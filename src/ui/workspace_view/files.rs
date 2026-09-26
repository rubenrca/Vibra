use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use gpui::{AnyElement, Context, Div, Task, Timer, div, prelude::*, px, svg};
use notify::{EventKind, RecursiveMode, Watcher};

use super::{ProjectFileRow, RightSidebarMode};
use crate::ports::files::{FileEntryKind, FileSystemPort};
use crate::ports::git::GitFileStatus;
use crate::ui::theme::{MONO_FONT, colors};

pub(crate) fn collect_project_files(
    port: &dyn FileSystemPort,
    root: &Path,
    directory: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    show_hidden: bool,
    output: &mut Vec<ProjectFileRow>,
) -> anyhow::Result<()> {
    const MAX_VISIBLE_FILE_ROWS: usize = 5_000;
    if output.len() >= MAX_VISIBLE_FILE_ROWS {
        return Ok(());
    }
    for entry in port.list_directory_limited(
        root,
        directory,
        show_hidden,
        MAX_VISIBLE_FILE_ROWS - output.len(),
    )? {
        if output.len() >= MAX_VISIBLE_FILE_ROWS {
            break;
        }
        let is_expanded = entry.kind == FileEntryKind::Directory && expanded.contains(&entry.path);
        let child_path = entry.path.clone();
        let is_directory = entry.kind == FileEntryKind::Directory;
        output.push(ProjectFileRow {
            entry,
            depth,
            expanded: is_expanded,
        });
        if is_directory && is_expanded {
            collect_project_files(
                port,
                root,
                &child_path,
                depth + 1,
                expanded,
                show_hidden,
                output,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn collect_search_files(
    port: &dyn FileSystemPort,
    root: &Path,
    directory: &Path,
    output: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if directory == root && collect_git_search_files(port, root, output)? {
        return Ok(());
    }
    collect_search_files_from_disk(port, root, directory, output)
}

fn collect_search_files_from_disk(
    port: &dyn FileSystemPort,
    root: &Path,
    directory: &Path,
    output: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    const MAX_INDEXED_FILES: usize = 20_000;
    if output.len() >= MAX_INDEXED_FILES {
        return Ok(());
    }
    for entry in port.list_directory_limited(root, directory, false, MAX_INDEXED_FILES)? {
        if output.len() >= MAX_INDEXED_FILES {
            break;
        }
        match entry.kind {
            FileEntryKind::Directory
                if !matches!(
                    entry.name.as_str(),
                    "target"
                        | "node_modules"
                        | "dist"
                        | "build"
                        | ".next"
                        | "DerivedData"
                        | "Pods"
                        | ".venv"
                ) =>
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

/// Git applies nested .gitignore rules and excludes dependency/build trees.
/// This runs from the palette's background task, never during UI render.
fn collect_git_search_files(
    port: &dyn FileSystemPort,
    root: &Path,
    output: &mut Vec<PathBuf>,
) -> anyhow::Result<bool> {
    let mut visited = HashSet::new();
    collect_git_search_files_inner(port, root, output, &mut visited)
}

fn collect_git_search_files_inner(
    port: &dyn FileSystemPort,
    root: &Path,
    output: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
) -> anyhow::Result<bool> {
    const MAX_INDEXED_FILES: usize = 20_000;
    const MAX_GIT_PATH_BYTES: usize = 16 * 1024 * 1024;
    if output.len() >= MAX_INDEXED_FILES {
        return Ok(true);
    }
    let canonical = root.canonicalize()?;
    if !visited.insert(canonical) {
        return Ok(true);
    }
    let mut child = match Command::new(crate::infrastructure::git::git_program())
        .current_dir(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Ok(false),
    };
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("Git stdout was piped")
        .take((MAX_GIT_PATH_BYTES + 1) as u64)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > MAX_GIT_PATH_BYTES {
        let _ = child.kill();
    }
    let success = child.wait().is_ok_and(|status| status.success());
    if read.is_err() || !success {
        return Ok(false);
    }
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

struct FileIconStyle {
    glyph: Option<&'static str>,
    color: gpui::Rgba,
}

fn file_icon_style(name: &str) -> FileIconStyle {
    let lower = name.to_ascii_lowercase();
    let extension = Path::new(&lower)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    let (glyph, color) = match (lower.as_str(), extension) {
        ("cargo.toml" | "cargo.lock", _) | (_, "toml") => ("⚙", colors().muted),
        (_, "rs") => ("Rs", gpui::rgb(0xdea584)),
        (_, "md" | "mdx") => ("Md", colors().accent),
        (_, "json" | "jsonc") => ("{}", gpui::rgb(0xcbcb41)),
        (_, "lock") => ("L", colors().subtle),
        (_, "yml" | "yaml") => ("Y", colors().muted),
        (_, "gitignore" | "gitattributes") | (".gitignore", _) => ("⊘", gpui::rgb(0xf05033)),
        ("license" | "licence" | "notice" | "copying", _) => ("©", colors().subtle),
        (_, "ts" | "tsx") => ("Ts", gpui::rgb(0x519aba)),
        (_, "js" | "jsx" | "mjs" | "cjs") => ("Js", gpui::rgb(0xcbcb41)),
        (_, "css" | "scss") => ("#", gpui::rgb(0x9b7ed9)),
        (_, "html" | "htm" | "svg") => ("<>", gpui::rgb(0xe34c26)),
        (_, "py") => ("Py", gpui::rgb(0x3572a5)),
        (_, "go") => ("Go", gpui::rgb(0x00add8)),
        (_, "sh" | "bash" | "zsh") => ("$", colors().success),
        (_, "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico") => ("▣", gpui::rgb(0xa074c4)),
        _ => {
            return FileIconStyle {
                glyph: None,
                color: colors().subtle,
            };
        }
    };
    FileIconStyle {
        glyph: Some(glyph),
        color,
    }
}

pub(crate) fn file_tree_icon_color(kind: FileEntryKind, name: &str) -> gpui::Rgba {
    match kind {
        FileEntryKind::Directory => colors().folder,
        FileEntryKind::Symlink => colors().accent,
        FileEntryKind::File => file_icon_style(name).color,
    }
}

/// Folder / file glyph for the Files tree. Folders use bundled monochrome SVGs;
/// files keep a compact extension-aware letter/dot so the tree stays light.
pub(crate) fn file_tree_icon(
    kind: FileEntryKind,
    expanded: bool,
    name: &str,
    color: gpui::Rgba,
) -> AnyElement {
    match kind {
        FileEntryKind::Directory => {
            let path = if expanded {
                "file-icons/folder-open.svg"
            } else {
                "file-icons/folder.svg"
            };
            svg()
                .path(path)
                .size(px(14.0))
                .flex_none()
                .text_color(color)
                .into_any_element()
        }
        FileEntryKind::Symlink => div()
            .flex_none()
            .font_family(MONO_FONT)
            .text_size(px(11.0))
            .line_height(px(16.0))
            .text_color(color)
            .child("↗")
            .into_any_element(),
        FileEntryKind::File => {
            let Some(glyph) = file_icon_style(name).glyph else {
                return svg()
                    .path("file-icons/file.svg")
                    .size(px(13.0))
                    .flex_none()
                    .text_color(color)
                    .into_any_element();
            };
            div()
                .flex_none()
                .font_family(MONO_FONT)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_size(px(8.5))
                .line_height(px(16.0))
                .text_color(color)
                .child(glyph)
                .into_any_element()
        }
    }
}

pub(crate) fn relative_repo_path(path: &Path, root: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn git_status_color(status: GitFileStatus) -> gpui::Rgba {
    match status {
        GitFileStatus::Modified | GitFileStatus::TypeChanged => colors().git_modified,
        GitFileStatus::Added
        | GitFileStatus::Untracked
        | GitFileStatus::Renamed
        | GitFileStatus::Copied => colors().git_added,
        GitFileStatus::Deleted | GitFileStatus::Conflicted => colors().git_deleted,
    }
}

pub(crate) fn git_status_rank(status: GitFileStatus) -> u8 {
    match status {
        GitFileStatus::Conflicted => 0,
        GitFileStatus::Deleted => 1,
        GitFileStatus::Modified | GitFileStatus::TypeChanged => 2,
        GitFileStatus::Renamed | GitFileStatus::Copied => 3,
        GitFileStatus::Added => 4,
        GitFileStatus::Untracked => 5,
    }
}

/// Roll up changed paths once for all directory rows. Keys borrow the paths in
/// `statuses`, so this adds no string copies on each Files sidebar render.
pub(crate) fn aggregate_dir_statuses(
    statuses: &HashMap<String, GitFileStatus>,
) -> HashMap<&str, GitFileStatus> {
    let mut directories = HashMap::new();
    for (path, status) in statuses {
        // Include the path itself to preserve exact directory matches, then
        // every ancestor and the repository root (empty string).
        let prefixes = std::iter::once(path.len())
            .chain(
                path.char_indices()
                    .filter_map(|(index, ch)| (ch == '/' || ch == '\\').then_some(index)),
            )
            .chain(std::iter::once(0));
        for length in prefixes {
            let entry = directories.entry(&path[..length]).or_insert(*status);
            if git_status_rank(*status) < git_status_rank(*entry) {
                *entry = *status;
            }
        }
    }
    directories
}

/// Right-side indicator: letter for modified/renamed, colored dots for add/delete.
pub(crate) fn git_status_trailing(status: GitFileStatus) -> Div {
    let slot = div()
        .size(px(12.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    let label = match status {
        GitFileStatus::Modified | GitFileStatus::TypeChanged => "M",
        GitFileStatus::Renamed => "R",
        GitFileStatus::Copied => "C",
        GitFileStatus::Conflicted => "U",
        GitFileStatus::Added | GitFileStatus::Untracked | GitFileStatus::Deleted => {
            return slot.child(
                div()
                    .size(px(6.0))
                    .flex_none()
                    .rounded_full()
                    .bg(git_status_color(status)),
            );
        }
    };
    slot.font_family(MONO_FONT)
        .text_size(px(10.0))
        .line_height(px(12.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(git_status_color(status))
        .child(label)
}

const FILES_WATCH_DEBOUNCE: Duration = Duration::from_millis(200);

pub(super) struct FilesWatch {
    pub root: PathBuf,
    stop: mpsc::Sender<()>,
    _thread: thread::JoinHandle<()>,
    _task: Task<()>,
}

impl Drop for FilesWatch {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn event_should_refresh(root: &Path, event: &notify::Event) -> bool {
    if matches!(event.kind, EventKind::Access(_) | EventKind::Other) {
        return false;
    }
    event.paths.iter().any(|path| {
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        if relative
            .components()
            .any(|component| component.as_os_str() == ".git")
        {
            return false;
        }
        !relative.components().next().is_some_and(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some("target" | "node_modules" | ".next" | "DerivedData" | "Pods" | ".venv")
            )
        })
    })
}

fn run_files_watcher(root: PathBuf, events: async_channel::Sender<()>, stop: mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(watcher) => watcher,
        Err(_) => return,
    };
    if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
        return;
    }
    loop {
        // Check on every event, not only after a quiet 250 ms window. A busy
        // build can otherwise keep an obsolete watcher alive after root change.
        if !matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Ok(event)) => {
                if event_should_refresh(&root, &event) {
                    let _ = events.try_send(());
                }
            }
            Ok(Err(_)) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

impl super::WorkspaceView {
    fn files_sidebar_active(&self) -> bool {
        self.workspace_section == super::WorkspaceSection::Workspace
            && self.right_sidebar_visible
            && self.right_sidebar_mode == RightSidebarMode::Files
    }

    pub(super) fn sync_files_watcher(&mut self, cx: &mut Context<Self>) {
        if self.workspace_section != super::WorkspaceSection::Workspace
            || !self.right_sidebar_visible
            || !self.has_project_context()
        {
            self.files_watch = None;
            return;
        }
        let root = self.project_root();
        if !root.exists() {
            self.files_watch = None;
            return;
        }
        if self
            .files_watch
            .as_ref()
            .is_some_and(|watch| watch.root == root)
        {
            return;
        }
        self.start_files_watcher(root, cx);
    }

    fn start_files_watcher(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let (stop_tx, stop_rx) = mpsc::channel();
        let (event_tx, event_rx) = async_channel::bounded::<()>(8);
        let watch_root = root.clone();
        let thread = match thread::Builder::new()
            .name("vibra-files-watch".into())
            .spawn(move || run_files_watcher(watch_root, event_tx, stop_rx))
        {
            Ok(thread) => thread,
            Err(_) => {
                self.files_watch = None;
                return;
            }
        };
        let task = cx.spawn(async move |this, cx| {
            while let Ok(()) = event_rx.recv().await {
                Timer::after(FILES_WATCH_DEBOUNCE).await;
                while event_rx.try_recv().is_ok() {}
                if this
                    .update(cx, |this, cx| {
                        if !this.right_sidebar_visible {
                            return;
                        }
                        if this.files_sidebar_active() {
                            this.refresh_project_files(cx);
                        }
                        this.diff_view.update(cx, |diff_view, cx| {
                            diff_view.refresh_from_fs_event(cx);
                        });
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.files_watch = Some(FilesWatch {
            root,
            stop: stop_tx,
            _thread: thread,
            _task: task,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::files::LocalFileSystemPort;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn directory_status_index_uses_path_boundaries_and_priority() {
        let statuses = HashMap::from([
            ("src/main.rs".into(), GitFileStatus::Modified),
            ("src/nested/lib.rs".into(), GitFileStatus::Untracked),
            ("srcfile.rs".into(), GitFileStatus::Deleted),
            ("docs/readme.md".into(), GitFileStatus::Added),
            ("src/conflict.rs".into(), GitFileStatus::Conflicted),
        ]);

        let directories = aggregate_dir_statuses(&statuses);
        assert_eq!(directories.get(""), Some(&GitFileStatus::Conflicted));
        assert_eq!(directories.get("src"), Some(&GitFileStatus::Conflicted));
        assert_eq!(
            directories.get("src/nested"),
            Some(&GitFileStatus::Untracked)
        );
        assert_eq!(directories.get("docs"), Some(&GitFileStatus::Added));
        assert_eq!(directories.get("srcfile"), None);
    }

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
        collect_search_files(&LocalFileSystemPort, &root, &root, &mut files).unwrap();
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
        collect_search_files(&LocalFileSystemPort, &root, &root, &mut files).unwrap();
        assert!(files.contains(&root.join("vendor-lib/nested.rs")));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn watcher_ignores_generated_trees_but_refreshes_sources() {
        let event = |path: &str| notify::Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![PathBuf::from(path)],
            attrs: Default::default(),
        };
        assert!(!event_should_refresh(
            Path::new("/repo"),
            &event("/repo/target/debug/file")
        ));
        assert!(!event_should_refresh(
            Path::new("/repo"),
            &event("/repo/node_modules/pkg/file")
        ));
        assert!(event_should_refresh(
            Path::new("/repo"),
            &event("/repo/src/main.rs")
        ));
        assert!(event_should_refresh(
            Path::new("/repo/build"),
            &event("/repo/build/src/main.rs")
        ));
        assert!(event_should_refresh(
            Path::new("/repo"),
            &event("/repo/src/build/main.rs")
        ));
    }
}
