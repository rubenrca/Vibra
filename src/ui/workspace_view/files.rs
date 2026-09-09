use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use gpui::{AnyElement, Context, Div, Task, Timer, div, prelude::*, px, svg};
use notify::{EventKind, RecursiveMode, Watcher};

use super::{ProjectFileRow, RightSidebarMode};
use crate::ports::files::{FileEntryKind, FileSystemPort};
use crate::ports::git::GitFileStatus;
use crate::ui::theme::colors;

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
    for entry in port.list_directory(root, directory, show_hidden)? {
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
    const MAX_INDEXED_FILES: usize = 20_000;
    if output.len() >= MAX_INDEXED_FILES {
        return Ok(());
    }
    for entry in port.list_directory(root, directory, false)? {
        if output.len() >= MAX_INDEXED_FILES {
            break;
        }
        match entry.kind {
            FileEntryKind::Directory
                if !matches!(
                    entry.name.as_str(),
                    "target" | "node_modules" | "dist" | "build" | ".next" | "DerivedData"
                ) =>
            {
                collect_search_files(port, root, &entry.path, output)?;
            }
            FileEntryKind::File => output.push(entry.path),
            FileEntryKind::Directory | FileEntryKind::Symlink => {}
        }
    }
    Ok(())
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
                .text_color(color)
                .into_any_element()
        }
        FileEntryKind::Symlink => div()
            .font_family("JetBrains Mono")
            .text_size(px(11.0))
            .text_color(color)
            .child("↗")
            .into_any_element(),
        FileEntryKind::File => {
            let Some(glyph) = file_icon_style(name).glyph else {
                return svg()
                    .path("file-icons/file.svg")
                    .size(px(13.0))
                    .text_color(color)
                    .into_any_element();
            };
            div()
                .font_family("JetBrains Mono")
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_size(px(8.5))
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

/// Roll up git status of files under a directory (empty `rel` = repo root).
pub(crate) fn path_is_under_dir(path: &str, rel: &str) -> bool {
    path == rel
        || path
            .as_bytes()
            .get(rel.len())
            .is_some_and(|byte| *byte == b'/' || *byte == b'\\')
            && path.starts_with(rel)
}

pub(crate) fn aggregate_dir_status(
    rel: &str,
    statuses: &HashMap<String, GitFileStatus>,
) -> Option<GitFileStatus> {
    statuses
        .iter()
        .filter(|(path, _)| rel.is_empty() || path_is_under_dir(path, rel))
        .map(|(_, status)| *status)
        .min_by_key(|status| git_status_rank(*status))
}

/// Right-side indicator: letter for modified/renamed, colored dots for add/delete.
pub(crate) fn git_status_trailing(status: GitFileStatus) -> Div {
    let label = match status {
        GitFileStatus::Modified | GitFileStatus::TypeChanged => "M",
        GitFileStatus::Renamed => "R",
        GitFileStatus::Copied => "C",
        GitFileStatus::Conflicted => "U",
        GitFileStatus::Added | GitFileStatus::Untracked | GitFileStatus::Deleted => {
            return div()
                .size(px(6.0))
                .flex_none()
                .rounded_full()
                .bg(git_status_color(status));
        }
    };
    div()
        .flex_none()
        .font_family("JetBrains Mono")
        .text_size(px(10.0))
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

fn event_should_refresh(event: &notify::Event) -> bool {
    if matches!(event.kind, EventKind::Access(_) | EventKind::Other) {
        return false;
    }
    event.paths.iter().any(|path| {
        !path
            .components()
            .any(|component| component.as_os_str() == ".git")
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
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Ok(event)) => {
                if event_should_refresh(&event) {
                    let _ = events.try_send(());
                }
            }
            Ok(Err(_)) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stop.try_recv().is_ok() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

impl super::WorkspaceView {
    fn files_sidebar_active(&self) -> bool {
        self.right_sidebar_visible && self.right_sidebar_mode == RightSidebarMode::Files
    }

    pub(super) fn sync_files_watcher(&mut self, cx: &mut Context<Self>) {
        if !self.files_sidebar_active() {
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
                        if this.files_sidebar_active() {
                            this.refresh_project_files(cx);
                        }
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
