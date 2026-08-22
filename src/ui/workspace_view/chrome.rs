use std::path::Path;

use gpui::{Div, div, prelude::*, px};

use super::SidebarWorkspaceMeta;

pub(crate) fn sidebar_tab_line(
    text: &str,
    color: gpui::Rgba,
    size: f32,
    medium: bool,
    mono: bool,
) -> Div {
    let mut row = div()
        .w_full()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(px(size))
        .text_color(color)
        .child(text.to_owned());
    if medium {
        row = row.font_weight(gpui::FontWeight::MEDIUM);
    }
    if mono {
        row = row.font_family("JetBrains Mono");
    }
    row
}

pub(crate) fn is_generic_tab_title(title: &str) -> bool {
    let title = title.trim();
    if title.is_empty() {
        return true;
    }
    let head = title
        .split([' ', '—', '–', '-', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or(title);
    matches!(
        head,
        "Terminal"
            | "terminal"
            | "zsh"
            | "bash"
            | "fish"
            | "sh"
            | "nu"
            | "dash"
            | "login"
            | "pwsh"
            | "ksh"
    )
}

pub(crate) fn title_path_suffix(title: &str) -> Option<&str> {
    title
        .split_once(": ")
        .map(|(_, rest)| rest.trim())
        .filter(|rest| rest.starts_with('/') || rest.starts_with('~'))
}

/// Trim an OSC title without throwing away the command arguments that distinguish panes.
pub(crate) fn compact_chrome_label(label: &str, max_chars: usize) -> String {
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.chars().count() <= max_chars {
        return label;
    }
    let visible = max_chars.saturating_sub(1);
    format!("{}…", label.chars().take(visible).collect::<String>())
}

/// A meaningful live command from OSC, excluding shell-only and prompt-path titles.
pub(crate) fn live_command_title(title: Option<&str>) -> Option<String> {
    let title = title?.trim();
    if title.is_empty() || is_generic_tab_title(title) || title_path_suffix(title).is_some() {
        return None;
    }
    Some(compact_chrome_label(title, 42))
}

/// Compact tab label: alias first, then the complete live command, then the directory.
pub(crate) fn tab_display_title(
    alias: Option<&str>,
    title: Option<&str>,
    working_directory: Option<&str>,
    index: usize,
) -> String {
    if let Some(alias) = alias.map(str::trim).filter(|alias| !alias.is_empty()) {
        return compact_chrome_label(alias, 22);
    }
    if let Some(path) = title.and_then(title_path_suffix) {
        let name = directory_basename(path);
        if name != "—" {
            return name;
        }
    }
    if let Some(command) = live_command_title(title) {
        return compact_chrome_label(&command, 22);
    }
    if let Some(path) = working_directory {
        let name = directory_basename(path);
        if name != "—" {
            return name;
        }
    }
    format!("Terminal {}", index + 1)
}

/// Secondary pane-header label. Commands keep the cwd visible; aliases keep both command and cwd.
pub(crate) fn pane_detail_title(
    alias: Option<&str>,
    title: Option<&str>,
    working_directory: Option<&str>,
    home: Option<&Path>,
) -> Option<String> {
    let command = live_command_title(title);
    let path = working_directory
        .map(|path| format_sidebar_path(path, home))
        .filter(|path| path != "—");
    match (
        alias.map(str::trim).filter(|alias| !alias.is_empty()),
        command,
        path,
    ) {
        (Some(_), Some(command), Some(path)) => Some(format!("{command}  ·  {path}")),
        (Some(_), Some(command), None) => Some(command),
        (_, _, Some(path)) => Some(path),
        _ => None,
    }
}

/// Directory basename for chrome labels (`/Users/me/Dev/Vibra` → `Vibra`).
pub(crate) fn directory_basename(path: &str) -> String {
    let path = path.trim().trim_end_matches(['/', '\\']);
    if path.is_empty() {
        return "—".to_owned();
    }
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_owned())
}

/// Short path for sidebar tabs: `~/…`, and collapse long intermediate segments.
pub(crate) fn format_sidebar_path(path: &str, home: Option<&Path>) -> String {
    let path = path.trim();
    if path.is_empty() {
        return "—".to_owned();
    }
    let display = if let Some(home) = home {
        let home_str = home.to_string_lossy();
        if path == home_str.as_ref() {
            "~".to_owned()
        } else if let Some(rest) = path
            .strip_prefix(home_str.as_ref())
            .and_then(|rest| rest.strip_prefix('/').or_else(|| rest.strip_prefix('\\')))
        {
            format!("~/{rest}")
        } else {
            path.to_owned()
        }
    } else {
        path.to_owned()
    };

    let components: Vec<&str> = display
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    if components.len() <= 3 {
        return display;
    }
    // Keep root marker + last two segments: ~/…/src/app
    let head = components[0];
    let tail = &components[components.len() - 2..];
    if head == "~" {
        format!("~/…/{}/{}", tail[0], tail[1])
    } else if display.starts_with('/') {
        format!("/…/{}/{}", tail[0], tail[1])
    } else {
        format!("…/{}/{}", tail[0], tail[1])
    }
}

/// Branch + dirty/ahead/behind for sidebar tabs (compact cmux-style).
pub(crate) fn format_sidebar_branch(meta: &SidebarWorkspaceMeta) -> Option<String> {
    let branch = meta.branch.as_ref()?;
    let mut label = branch.clone();
    if meta.dirty {
        label.push('*');
    }
    if meta.ahead > 0 {
        label.push_str(&format!(" ↑{}", meta.ahead));
    }
    if meta.behind > 0 {
        label.push_str(&format!(" ↓{}", meta.behind));
    }
    Some(label)
}

/// Smooth ease-out for sidebar width (`t` in `0.0..=1.0`).
pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}
