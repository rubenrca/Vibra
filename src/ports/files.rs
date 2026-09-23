use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEntryKind {
    Directory,
    File,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: FileEntryKind,
}

/// Boundary for project-scoped file inspection.
pub trait FileSystemPort: Send + Sync {
    fn list_directory(
        &self,
        project_root: &Path,
        directory: &Path,
        show_hidden: bool,
    ) -> Result<Vec<FileEntry>>;

    /// Return at most `limit` entries in the same order as `list_directory`.
    /// The default keeps test adapters simple; the local adapter bounds memory
    /// while enumerating very large directories.
    fn list_directory_limited(
        &self,
        project_root: &Path,
        directory: &Path,
        show_hidden: bool,
        limit: usize,
    ) -> Result<Vec<FileEntry>> {
        let mut entries = self.list_directory(project_root, directory, show_hidden)?;
        entries.truncate(limit);
        Ok(entries)
    }
}
