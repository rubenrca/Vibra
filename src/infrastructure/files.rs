use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::ports::files::{FileEntry, FileEntryKind, FileSystemPort};

#[derive(Debug, Clone, Copy, Default)]
pub struct LocalFileSystemPort;

const MAX_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_INDEXED_FILES: usize = 20_000;

impl FileSystemPort for LocalFileSystemPort {
    fn search_files(&self, root: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
        if collect_git_search_files_inner(self, root, output, &mut HashSet::new())? {
            return Ok(());
        }
        collect_search_files_from_disk(self, root, root, output)
    }

    fn read_text_file(&self, project_root: &Path, path: &Path) -> Result<String> {
        let root = canonical_root(project_root)?;
        let path = path
            .canonicalize()
            .with_context(|| format!("Could not open {}", path.display()))?;
        if !path.starts_with(&root) {
            bail!("The file is outside the project.");
        }
        if !path.metadata()?.is_file() {
            bail!("This entry is not a regular file.");
        }
        let file = fs::File::open(&path)?;
        if file.metadata()?.len() > MAX_DOCUMENT_BYTES {
            bail!("The file exceeds the viewer's 2 MB limit. You can open it in its application.");
        }
        let mut bytes = Vec::new();
        file.take(MAX_DOCUMENT_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            bail!("The file exceeds the viewer's 2 MB limit. You can open it in its application.");
        }
        if bytes.contains(&0) {
            bail!("This is not a text document. You can open it in its application.");
        }
        let text = String::from_utf8(bytes).map_err(|_| {
            anyhow::anyhow!(
                "The viewer supports UTF-8 text. You can open this file in its application."
            )
        })?;
        if text.lines().any(|line| line.len() > 32_768) {
            bail!(concat!(
                "This file contains lines that are too long for the viewer. ",
                "You can open it in its application."
            ));
        }
        Ok(text)
    }

    fn create_entry(
        &self,
        root: &Path,
        directory: &Path,
        name: &str,
        folder: bool,
    ) -> Result<PathBuf> {
        create_project_entry(root, directory, name, folder).map_err(anyhow::Error::msg)
    }

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
            .with_context(|| format!("could not read {}", directory.display()))?
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
            let candidate = SortedEntry {
                lowercase_name: name.to_lowercase(),
                entry: FileEntry {
                    path: entry.path(),
                    name,
                    kind,
                },
            };
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
            .map(|entry| entry.entry)
            .collect())
    }
}

fn collect_search_files_from_disk(
    port: &dyn FileSystemPort,
    root: &Path,
    directory: &Path,
    output: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if output.len() >= MAX_INDEXED_FILES {
        return Ok(());
    }
    for entry in port.list_directory_limited(root, directory, false, MAX_INDEXED_FILES)? {
        if output.len() >= MAX_INDEXED_FILES {
            break;
        }
        match entry.kind {
            FileEntryKind::Directory
                if !crate::ports::files::SKIPPED_DIRECTORY_NAMES.contains(&entry.name.as_str()) =>
            {
                // One unreadable subdirectory must not discard the paths
                // already indexed from the rest of the project.
                let _ = collect_search_files_from_disk(port, root, &entry.path, output);
            }
            FileEntryKind::File => output.push(entry.path),
            FileEntryKind::Directory | FileEntryKind::Symlink => {}
        }
    }
    Ok(())
}

fn collect_git_search_files_inner(
    port: &dyn FileSystemPort,
    root: &Path,
    output: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
) -> anyhow::Result<bool> {
    if output.len() >= MAX_INDEXED_FILES {
        return Ok(true);
    }
    let canonical = root.canonicalize()?;
    if !visited.insert(canonical) {
        return Ok(true);
    }
    let bytes = match super::git::search_paths(root) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(false),
    };
    for path in bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        if output.len() >= MAX_INDEXED_FILES {
            break;
        }
        let relative = std::ffi::OsStr::from_bytes(path);
        let relative = Path::new(relative);
        if !relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            continue;
        }
        let absolute = root.join(relative);
        let Ok(metadata) = std::fs::symlink_metadata(&absolute) else {
            continue;
        };
        if metadata.is_file() {
            output.push(absolute);
        } else if metadata.is_dir() {
            // Gitlink entries are directories, not files. Search initialized
            // submodules with their own ignore rules and a shared file budget.
            if absolute.join(".git").exists()
                && collect_git_search_files_inner(port, &absolute, output, visited)?
            {
                continue;
            }
            let _ = collect_search_files_from_disk(port, &absolute, &absolute, output);
        }
    }
    Ok(true)
}

/// Creates `name` inside `directory`, never outside `root` and never over an
/// existing entry. Returns the new path.
fn create_project_entry(
    root: &Path,
    directory: &Path,
    name: &str,
    folder: bool,
) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name.".to_owned());
    }
    // Nested names (`src/lib.rs`) are allowed; escaping the project is not.
    let relative = Path::new(name);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("The name must stay within the project folder.".to_owned());
    }
    let canonical_root = canonical_root(root).map_err(|error| error.to_string())?;
    canonical_directory(&canonical_root, directory).map_err(|error| error.to_string())?;
    let path = directory.join(relative);
    // Nested names may cross a symlink even when their lexical path stays
    // inside the project. Resolve the closest existing parent before writing.
    let existing_parent = path
        .parent()
        .and_then(|parent| {
            parent
                .ancestors()
                .find(|ancestor| ancestor.symlink_metadata().is_ok())
        })
        .ok_or_else(|| "The folder is unavailable.".to_owned())?;
    canonical_directory(&canonical_root, existing_parent).map_err(|error| error.to_string())?;
    if path.symlink_metadata().is_ok() {
        return Err(format!("{} already exists.", path.display()));
    }
    let result = if folder {
        std::fs::create_dir_all(&path)
    } else {
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map(|_| ())
            })
    };
    result.map_err(|error| format!("Could not create {}: {error}", path.display()))?;
    Ok(path)
}

struct SortedEntry {
    entry: FileEntry,
    lowercase_name: String,
}

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
        entry_rank(self.entry.kind)
            .cmp(&entry_rank(other.entry.kind))
            .then_with(|| self.lowercase_name.cmp(&other.lowercase_name))
            .then_with(|| self.entry.name.cmp(&other.entry.name))
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
        .with_context(|| format!("could not resolve {}", root.display()))
}

fn canonical_directory(root: &Path, directory: &Path) -> Result<PathBuf> {
    let directory = directory
        .canonicalize()
        .with_context(|| format!("could not resolve {}", directory.display()))?;
    if !directory.starts_with(root) {
        bail!("{} is outside the project", directory.display());
    }
    if !directory.is_dir() {
        bail!("{} is not a directory", directory.display());
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use uuid::Uuid;

    #[test]
    fn quick_open_respects_nested_gitignore_rules() {
        let root = std::env::temp_dir().join(format!("vibra-quick-open-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        assert!(
            Command::new("git")
                .current_dir(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("src/.gitignore"), "generated.rs\n").unwrap();
        fs::write(root.join("src/generated.rs"), "ignored\n").unwrap();
        fs::write(root.join("src/main.rs"), "visible\n").unwrap();
        let mut files = Vec::new();
        LocalFileSystemPort.search_files(&root, &mut files).unwrap();
        assert!(files.contains(&root.join("src/main.rs")));
        assert!(!files.contains(&root.join("src/generated.rs")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quick_open_indexes_initialized_submodules() {
        let base = std::env::temp_dir().join(format!("vibra-submodule-{}", Uuid::new_v4()));
        let root = base.join("project");
        let library = base.join("library");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&library).unwrap();
        let git = |cwd: &Path, args: &[&str]| {
            let output = Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&library, &["init", "-q"]);
        git(&library, &["config", "user.name", "Vibra Test"]);
        git(&library, &["config", "user.email", "vibra@example.invalid"]);
        fs::write(library.join("nested.rs"), "pub fn nested() {}\n").unwrap();
        git(&library, &["add", "nested.rs"]);
        git(&library, &["commit", "-qm", "library"]);
        git(&root, &["init", "-q"]);
        git(
            &root,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                library.to_str().unwrap(),
                "vendor-lib",
            ],
        );
        let mut files = Vec::new();
        LocalFileSystemPort.search_files(&root, &mut files).unwrap();
        assert!(files.contains(&root.join("vendor-lib/nested.rs")));
        fs::remove_dir_all(base).unwrap();
    }

    fn temporary_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("vibra-files-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn documents_open_without_git_and_never_modify_the_source() {
        let root = temporary_root();
        let path = root.join("notas.md");
        let text = "# Documento\nTexto sin cambios ni repositorio.\n";
        fs::write(&path, text).unwrap();
        assert_eq!(
            LocalFileSystemPort.read_text_file(&root, &path).unwrap(),
            text
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        fs::write(root.join("empty.txt"), "").unwrap();
        assert_eq!(
            LocalFileSystemPort
                .read_text_file(&root, &root.join("empty.txt"))
                .unwrap(),
            ""
        );
        assert!(LocalFileSystemPort.read_text_file(&root, &root).is_err());
        assert!(
            LocalFileSystemPort
                .read_text_file(&root, &root.join("missing.txt"))
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_reads_bound_size_and_reject_binary_and_external_links() {
        use std::os::unix::fs::symlink;
        let root = temporary_root();
        let outside = temporary_root();
        let port = LocalFileSystemPort;
        let large = root.join("large.txt");
        fs::File::create(&large)
            .unwrap()
            .set_len(MAX_DOCUMENT_BYTES + 1)
            .unwrap();
        assert!(
            port.read_text_file(&root, &large)
                .unwrap_err()
                .to_string()
                .contains("2 MB")
        );
        let binary = root.join("binary.dat");
        fs::write(&binary, [0, 1, 2]).unwrap();
        assert!(port.read_text_file(&root, &binary).is_err());
        fs::write(&binary, [255, 254]).unwrap();
        assert!(port.read_text_file(&root, &binary).is_err());
        fs::write(outside.join("external.txt"), "outside").unwrap();
        symlink(outside.join("external.txt"), root.join("link.txt")).unwrap();
        assert!(port.read_text_file(&root, &root.join("link.txt")).is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
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

    #[test]
    fn limited_listing_matches_full_case_insensitive_order() {
        use std::os::unix::fs::symlink;
        let root = temporary_root();
        for name in [
            "Zulu",
            "alpha",
            "A-first.txt",
            "a-second.txt",
            "É-first.txt",
            "é-second.txt",
        ] {
            fs::write(root.join(name), "").unwrap();
        }
        fs::create_dir(root.join("folder")).unwrap();
        symlink(root.join("A-first.txt"), root.join("link")).unwrap();
        let full = LocalFileSystemPort
            .list_directory(&root, &root, true)
            .unwrap();
        assert_eq!(
            full.iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            [
                "folder",
                "A-first.txt",
                "a-second.txt",
                "alpha",
                "Zulu",
                "É-first.txt",
                "é-second.txt",
                "link"
            ]
        );
        for limit in 0..=full.len() + 1 {
            let limited = LocalFileSystemPort
                .list_directory_limited(&root, &root, true, limit)
                .unwrap();
            assert_eq!(limited, full[..limit.min(full.len())]);
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn entries_are_created_inside_the_project_only() {
        let root = std::env::temp_dir().join(format!("vibra-explorer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = create_project_entry(&root, &root, "src/lib.rs", false).unwrap();
        assert!(file.is_file());
        let folder = create_project_entry(&root, &root.join("src"), "nested", true).unwrap();
        assert!(folder.is_dir());
        assert!(create_project_entry(&root, &root, "src/lib.rs", false).is_err());
        assert!(create_project_entry(&root, &root, "../escape", false).is_err());
        assert!(create_project_entry(&root, &root, "/tmp/abs", true).is_err());
        assert!(create_project_entry(&root, &root, "  ", true).is_err());
        assert!(create_project_entry(&root, Path::new("/tmp"), "x", true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nested_entry_creation_resolves_symlinks_before_writing() {
        use std::os::unix::fs::symlink;

        let root = temporary_root();
        let outside = temporary_root();
        let port = LocalFileSystemPort;
        symlink(&outside, root.join("external")).unwrap();
        for folder in [false, true] {
            assert!(
                port.create_entry(&root, &root, "external/nested/new", folder)
                    .is_err()
            );
            assert!(
                port.create_entry(&root, &root.join("external"), "new", folder)
                    .is_err()
            );
        }
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::create_dir(root.join("internal")).unwrap();
        symlink(root.join("internal"), root.join("alias")).unwrap();
        port.create_entry(&root, &root, "alias/nested/file.txt", false)
            .unwrap();
        assert!(root.join("internal/nested/file.txt").is_file());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
