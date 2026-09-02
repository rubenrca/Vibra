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
    pub size: u64,
}

/// Boundary for project-scoped file inspection.
pub trait FileSystemPort: Send + Sync {
    fn list_directory(
        &self,
        project_root: &Path,
        directory: &Path,
        show_hidden: bool,
    ) -> Result<Vec<FileEntry>>;
}
