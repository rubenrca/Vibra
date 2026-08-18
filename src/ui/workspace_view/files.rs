use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{AnyElement, Div, div, prelude::*, px, svg};

use super::ProjectFileRow;
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

pub(crate) fn file_tree_icon_color(kind: FileEntryKind, name: &str) -> gpui::Rgba {
    match kind {
        FileEntryKind::Directory => colors().folder,
        FileEntryKind::Symlink => colors().accent,
        FileEntryKind::File => {
            let lower = name.to_ascii_lowercase();
            let ext = std::path::Path::new(name)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match (lower.as_str(), ext.as_str()) {
                ("cargo.toml" | "cargo.lock", _) | (_, "toml") => colors().muted,
                (_, "rs") => gpui::rgb(0xdea584),
                (_, "md" | "mdx") => colors().accent,
                (_, "json" | "jsonc") => gpui::rgb(0xcbcb41),
                (_, "lock") => colors().subtle,
                (_, "yml" | "yaml") => colors().muted,
                (_, "gitignore" | "gitattributes") | (".gitignore", _) => gpui::rgb(0xf05033),
                ("license" | "licence" | "notice" | "copying", _) => colors().subtle,
                (_, "ts" | "tsx") => gpui::rgb(0x519aba),
                (_, "js" | "jsx" | "mjs" | "cjs") => gpui::rgb(0xcbcb41),
                (_, "css" | "scss") => gpui::rgb(0x9b7ed9),
                (_, "html" | "htm" | "svg") => gpui::rgb(0xe34c26),
                (_, "py") => gpui::rgb(0x3572a5),
                (_, "go") => gpui::rgb(0x00add8),
                (_, "sh" | "bash" | "zsh") => colors().success,
                (_, "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico") => gpui::rgb(0xa074c4),
                _ if name.starts_with('.') => colors().subtle,
                _ => colors().subtle,
            }
        }
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
            let lower = name.to_ascii_lowercase();
            let ext = std::path::Path::new(name)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let glyph = match (lower.as_str(), ext.as_str()) {
                ("cargo.toml" | "cargo.lock", _) | (_, "toml") => "⚙",
                (_, "rs") => "Rs",
                (_, "md" | "mdx") => "Md",
                (_, "json" | "jsonc") => "{}",
                (_, "lock") => "L",
                (_, "yml" | "yaml") => "Y",
                (_, "gitignore" | "gitattributes") | (".gitignore", _) => "⊘",
                ("license" | "licence" | "notice" | "copying", _) => "©",
                (_, "ts" | "tsx") => "Ts",
                (_, "js" | "jsx" | "mjs" | "cjs") => "Js",
                (_, "css" | "scss") => "#",
                (_, "html" | "htm" | "svg") => "<>",
                (_, "py") => "Py",
                (_, "go") => "Go",
                (_, "sh" | "bash" | "zsh") => "$",
                (_, "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico") => "▣",
                _ => {
                    return svg()
                        .path("file-icons/file.svg")
                        .size(px(13.0))
                        .text_color(color)
                        .into_any_element();
                }
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
    let mut best: Option<GitFileStatus> = None;
    for (path, status) in statuses {
        let under = rel.is_empty() || path_is_under_dir(path, rel);
        if !under {
            continue;
        }
        best = Some(match best {
            None => *status,
            Some(current) if git_status_rank(*status) < git_status_rank(current) => *status,
            Some(current) => current,
        });
    }
    best
}

/// Right-side indicator: letter for modified/renamed, colored dots for add/delete.
pub(crate) fn git_status_trailing(status: GitFileStatus) -> Div {
    match status {
        GitFileStatus::Modified | GitFileStatus::TypeChanged => div()
            .flex_none()
            .font_family("JetBrains Mono")
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(colors().git_modified)
            .child("M"),
        GitFileStatus::Renamed => div()
            .flex_none()
            .font_family("JetBrains Mono")
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(colors().git_added)
            .child("R"),
        GitFileStatus::Copied => div()
            .flex_none()
            .font_family("JetBrains Mono")
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(colors().git_added)
            .child("C"),
        GitFileStatus::Conflicted => div()
            .flex_none()
            .font_family("JetBrains Mono")
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(colors().git_deleted)
            .child("U"),
        GitFileStatus::Added | GitFileStatus::Untracked => div()
            .size(px(6.0))
            .flex_none()
            .rounded_full()
            .bg(colors().git_added),
        GitFileStatus::Deleted => div()
            .size(px(6.0))
            .flex_none()
            .rounded_full()
            .bg(colors().git_deleted),
    }
}
