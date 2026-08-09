use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::ports::files::{FileEntry, FileEntryKind, FileSystemPort, TextFileSnapshot};

const MAX_TEXT_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default)]
pub struct LocalFileSystemPort;

impl FileSystemPort for LocalFileSystemPort {
    fn list_directory(
        &self,
        project_root: &Path,
        directory: &Path,
        show_hidden: bool,
    ) -> Result<Vec<FileEntry>> {
        let root = canonical_root(project_root)?;
        let directory = canonical_directory(&root, directory)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("no se pudo leer {}", directory.display()))?
        {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            let kind = if metadata.file_type().is_symlink() {
                FileEntryKind::Symlink
            } else if metadata.is_dir() {
                FileEntryKind::Directory
            } else {
                FileEntryKind::File
            };
            entries.push(FileEntry {
                path: entry.path(),
                name,
                kind,
                size: metadata.len(),
            });
        }
        entries.sort_by(|left, right| {
            entry_rank(left.kind)
                .cmp(&entry_rank(right.kind))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(entries)
    }

    fn create_file(&self, project_root: &Path, directory: &Path, name: &str) -> Result<PathBuf> {
        let root = canonical_root(project_root)?;
        let directory = canonical_directory(&root, directory)?;
        let target = unused_child(&directory, name)?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .with_context(|| format!("no se pudo crear {}", target.display()))?;
        Ok(target)
    }

    fn create_directory(
        &self,
        project_root: &Path,
        directory: &Path,
        name: &str,
    ) -> Result<PathBuf> {
        let root = canonical_root(project_root)?;
        let directory = canonical_directory(&root, directory)?;
        let target = unused_child(&directory, name)?;
        fs::create_dir(&target)
            .with_context(|| format!("no se pudo crear {}", target.display()))?;
        Ok(target)
    }

    fn rename(&self, project_root: &Path, path: &Path, new_name: &str) -> Result<PathBuf> {
        let root = canonical_root(project_root)?;
        let path = existing_entry(&root, path)?;
        let parent = path
            .parent()
            .context("la entrada no tiene directorio padre")?;
        let target = unused_child(parent, new_name)?;
        fs::rename(&path, &target).with_context(|| {
            format!(
                "no se pudo renombrar {} a {}",
                path.display(),
                target.display()
            )
        })?;
        Ok(target)
    }

    fn move_to_trash(&self, project_root: &Path, path: &Path) -> Result<()> {
        let root = canonical_root(project_root)?;
        let path = existing_entry(&root, path)?;
        #[cfg(target_os = "macos")]
        {
            const SCRIPT: &str = r#"
                on run argv
                    tell application "Finder"
                        delete POSIX file (item 1 of argv)
                    end tell
                end run
            "#;
            let status = Command::new("/usr/bin/osascript")
                .arg("-e")
                .arg(SCRIPT)
                .arg(path.as_os_str())
                .status()
                .with_context(|| format!("no se pudo abrir la Papelera para {}", path.display()))?;
            if !status.success() {
                bail!("Finder no pudo mover {} a la Papelera", path.display());
            }
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = path;
            bail!("mover a la Papelera aún no está soportado en esta plataforma")
        }
    }

    fn read_text_file(&self, project_root: &Path, path: &Path) -> Result<TextFileSnapshot> {
        let root = canonical_root(project_root)?;
        let path = canonical_regular_file(&root, path)?;
        if path.metadata()?.len() > MAX_TEXT_FILE_BYTES {
            bail!("{} supera el límite de 8 MiB del editor", path.display());
        }
        let bytes = fs::read(&path)?;
        if bytes.contains(&0) {
            bail!("{} parece ser binario", path.display());
        }
        let fingerprint = text_fingerprint(&bytes);
        let contents = String::from_utf8(bytes)
            .with_context(|| format!("{} no contiene UTF-8 válido", path.display()))?;
        Ok(TextFileSnapshot {
            path,
            contents,
            fingerprint,
        })
    }

    fn save_text_file(
        &self,
        project_root: &Path,
        path: &Path,
        contents: &str,
        expected_fingerprint: &str,
    ) -> Result<String> {
        let root = canonical_root(project_root)?;
        let path = canonical_regular_file(&root, path)?;
        let current = fs::read(&path)?;
        if text_fingerprint(&current) != expected_fingerprint {
            bail!("el archivo cambió fuera de VibraGPUI; recárgalo antes de guardar");
        }
        let parent = path
            .parent()
            .context("el archivo no tiene directorio padre")?;
        let file_name = path
            .file_name()
            .context("el archivo no tiene nombre")?
            .to_string_lossy();
        let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        fs::set_permissions(&temporary, fs::metadata(&path)?.permissions())?;
        if let Err(error) = output.write_all(contents.as_bytes()) {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        output.sync_all()?;
        drop(output);
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(text_fingerprint(contents.as_bytes()))
    }
}

fn entry_rank(kind: FileEntryKind) -> u8 {
    match kind {
        FileEntryKind::Directory => 0,
        FileEntryKind::File => 1,
        FileEntryKind::Symlink => 2,
    }
}

fn canonical_root(root: &Path) -> Result<PathBuf> {
    root.canonicalize()
        .with_context(|| format!("no se pudo resolver {}", root.display()))
}

fn canonical_directory(root: &Path, directory: &Path) -> Result<PathBuf> {
    let directory = directory
        .canonicalize()
        .with_context(|| format!("no se pudo resolver {}", directory.display()))?;
    if !directory.starts_with(root) {
        bail!("{} está fuera del proyecto", directory.display());
    }
    if !directory.is_dir() {
        bail!("{} no es un directorio", directory.display());
    }
    Ok(directory)
}

fn canonical_regular_file(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("no se pudo resolver {}", path.display()))?;
    if !path.starts_with(root) {
        bail!("{} está fuera del proyecto", path.display());
    }
    if !path.is_file() {
        bail!("{} no es un archivo regular", path.display());
    }
    Ok(path)
}

fn text_fingerprint(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}-{:x}", bytes.len())
}

fn existing_entry(root: &Path, path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .context("la entrada no tiene directorio padre")?;
    let parent = canonical_directory(root, parent)?;
    let name = path.file_name().context("la entrada no tiene nombre")?;
    let path = parent.join(name);
    if path == root {
        bail!("no se puede modificar la raíz del proyecto");
    }
    fs::symlink_metadata(&path).with_context(|| format!("{} ya no existe", path.display()))?;
    Ok(path)
}

fn unused_child(directory: &Path, name: &str) -> Result<PathBuf> {
    validate_file_name(name)?;
    let target = directory.join(name);
    if fs::symlink_metadata(&target).is_ok() {
        bail!("{} ya existe", target.display());
    }
    Ok(target)
}

fn validate_file_name(name: &str) -> Result<()> {
    if name.trim() != name || name.is_empty() || name.as_bytes().contains(&0) {
        bail!("el nombre no puede estar vacío ni tener espacios externos");
    }
    let mut components = Path::new(name).components();
    let valid = matches!(components.next(), Some(Component::Normal(value)) if value != OsStr::new("."))
        && components.next().is_none();
    if !valid {
        bail!("usa un nombre simple, sin /, . ni ..");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("vibra-files-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn file_operations_stay_inside_the_project_and_never_overwrite() {
        let root = temporary_root();
        let port = LocalFileSystemPort;
        let directory = port.create_directory(&root, &root, "src").unwrap();
        let file = port.create_file(&root, &directory, "main.rs").unwrap();

        assert!(port.create_file(&root, &directory, "main.rs").is_err());
        assert!(port.create_file(&root, &directory, "../escape").is_err());
        assert!(port.rename(&root, &file, "../escape").is_err());
        assert!(port.list_directory(&root, Path::new("/"), false).is_err());

        let renamed = port.rename(&root, &file, "lib.rs").unwrap();
        assert!(renamed.exists());
        assert!(!file.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listing_sorts_directories_first_and_hides_dotfiles() {
        let root = temporary_root();
        let port = LocalFileSystemPort;
        port.create_file(&root, &root, "z.txt").unwrap();
        port.create_file(&root, &root, ".secret").unwrap();
        port.create_directory(&root, &root, "alpha").unwrap();

        let visible = port.list_directory(&root, &root, false).unwrap();
        assert_eq!(
            visible
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "z.txt"]
        );
        assert_eq!(port.list_directory(&root, &root, true).unwrap().len(), 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn text_saves_are_atomic_and_reject_external_changes() {
        let root = temporary_root();
        let path = root.join("note.txt");
        fs::write(&path, "first\n").unwrap();
        let port = LocalFileSystemPort;
        let opened = port.read_text_file(&root, &path).unwrap();

        let fingerprint = port
            .save_text_file(&root, &path, "second\n", &opened.fingerprint)
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
        fs::write(&path, "external\n").unwrap();
        assert!(
            port.save_text_file(&root, &path, "third\n", &fingerprint)
                .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "external\n");
        fs::remove_dir_all(root).unwrap();
    }
}
