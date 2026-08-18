use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::infrastructure::paths::{AtomicWriteOptions, atomic_write_with};

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
            bail!("el archivo cambió fuera de Vibra; recárgalo antes de guardar");
        }
        atomic_write_with(
            &path,
            contents.as_bytes(),
            AtomicWriteOptions {
                unix_mode: None,
                sync: true,
                preserve_permissions_from: Some(&path),
            },
        )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temporary_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("vibra-files-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn listing_sorts_directories_first_and_hides_dotfiles() {
        let root = temporary_root();
        let port = LocalFileSystemPort;
        fs::write(root.join("z.txt"), "").unwrap();
        fs::write(root.join(".secret"), "").unwrap();
        fs::create_dir(root.join("alpha")).unwrap();

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
