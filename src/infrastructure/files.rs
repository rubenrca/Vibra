use std::cmp::Ordering;
use std::collections::BinaryHeap;
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
        self.list_directory_limited(project_root, directory, show_hidden, usize::MAX)
    }

    fn list_directory_limited(
        &self,
        project_root: &Path,
        directory: &Path,
        show_hidden: bool,
        limit: usize,
    ) -> Result<Vec<FileEntry>> {
        let root = canonical_root(project_root)?;
        let directory = canonical_directory(&root, directory)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut entries = BinaryHeap::new();
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
            let candidate = SortedEntry(FileEntry {
                path: entry.path(),
                name,
                kind,
            });
            if entries.len() < limit {
                entries.push(candidate);
            } else if entries.peek().is_some_and(|largest| candidate < *largest) {
                entries.pop();
                entries.push(candidate);
            }
        }
        Ok(entries
            .into_sorted_vec()
            .into_iter()
            .map(|entry| entry.0)
            .collect())
    }
}

struct SortedEntry(FileEntry);

impl PartialEq for SortedEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for SortedEntry {}

impl PartialOrd for SortedEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortedEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        entry_rank(self.0.kind)
            .cmp(&entry_rank(other.0.kind))
            .then_with(|| self.0.name.to_lowercase().cmp(&other.0.name.to_lowercase()))
            .then_with(|| self.0.name.cmp(&other.0.name))
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
        assert_eq!(
            port.list_directory_limited(&root, &root, true, 2)
                .unwrap()
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", ".secret"]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
