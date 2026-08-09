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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFileSnapshot {
    pub path: PathBuf,
    pub contents: String,
    pub fingerprint: String,
}

/// Boundary for project-scoped filesystem operations.
pub trait FileSystemPort: Send + Sync {
    fn list_directory(
        &self,
        project_root: &Path,
        directory: &Path,
        show_hidden: bool,
    ) -> Result<Vec<FileEntry>>;

    fn create_file(&self, project_root: &Path, directory: &Path, name: &str) -> Result<PathBuf>;

    fn create_directory(
        &self,
        project_root: &Path,
        directory: &Path,
        name: &str,
    ) -> Result<PathBuf>;

    fn rename(&self, project_root: &Path, path: &Path, new_name: &str) -> Result<PathBuf>;

    /// Moves an entry to the operating system's recoverable trash.
    fn move_to_trash(&self, project_root: &Path, path: &Path) -> Result<()>;

    fn read_text_file(&self, project_root: &Path, path: &Path) -> Result<TextFileSnapshot>;

    /// Atomically saves only if the file still matches the fingerprint that was read.
    fn save_text_file(
        &self,
        project_root: &Path,
        path: &Path,
        contents: &str,
        expected_fingerprint: &str,
    ) -> Result<String>;
}
