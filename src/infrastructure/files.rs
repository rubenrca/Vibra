use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::ports::files::{FileEntry, FileEntryKind, FileSystemPort};

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
}
