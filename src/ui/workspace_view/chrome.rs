use std::path::Path;

use gpui::{Div, div, prelude::*, px};

use crate::infrastructure::automation::{AgentAttention, AgentRuntimeState};
use crate::ui::theme::{MONO_FONT, colors};

use super::SidebarWorkspaceMeta;

pub(crate) const PANEL_GAP: f32 = 4.0;
pub(crate) const PANEL_RADIUS: f32 = 10.0;

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
        row = row.font_family(MONO_FONT);
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
    let title = title.trim();
    if title.starts_with('/') || title == "~" || title.starts_with("~/") {
        return Some(title);
    }
    title
        .split_once(':')
        .filter(|(prefix, _)| prefix.contains('@') || is_generic_tab_title(prefix))
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
    Some(title.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Compact tab label: alias first, then the complete live command, then the directory.
pub(crate) fn tab_display_title(
    alias: Option<&str>,
    title: Option<&str>,
    working_directory: Option<&str>,
    index: usize,
) -> String {
    if let Some(alias) = alias.map(str::trim).filter(|alias| !alias.is_empty()) {
        return alias.to_owned();
    }
    if let Some(command) = live_command_title(title) {
        return command;
    }
    if let Some(path) = working_directory {
        let name = directory_basename(path);
        if name != "—" {
            return name;
        }
    }
    // OSC prompt titles may lag behind `cd`; the live cwd above is authoritative.
    if let Some(path) = title.and_then(title_path_suffix) {
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
    let command = live_command_title(title).map(|command| compact_chrome_label(&command, 42));
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

/// Outer clip + inner full-width column so sidebar content does not reflow while animating.
pub(crate) fn clipped_width_panel(
    width: f32,
    full_width: f32,
    background: gpui::Rgba,
    content: impl IntoElement,
) -> Div {
    div()
        .w(px(width))
        .h_full()
        .flex_none()
        .relative()
        .overflow_hidden()
        .rounded(px(PANEL_RADIUS))
        .bg(background)
        .border_1()
        .border_color(colors().border_subtle)
        .child(
            div()
                .w(px(full_width))
                .h_full()
                .flex()
                .flex_col()
                .child(content),
        )
}

pub(crate) fn sidebar_agent_priority(
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
) -> u8 {
    match (state, attention) {
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Permission)) => 50,
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Question)) => 45,
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Plan)) => 40,
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Notification)) => 35,
        (Some(AgentRuntimeState::Working), _) => 30,
        (Some(AgentRuntimeState::Waiting), _) => 20,
        (Some(AgentRuntimeState::Idle), _) => 10,
        (None, _) => 0,
    }
}

pub(crate) fn sidebar_agent_line(
    kind: Option<&str>,
    model: Option<&str>,
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
) -> String {
    let kind = kind.unwrap_or("Terminal");
    let activity = match (state, attention) {
        (_, Some(AgentAttention::Permission)) => "pide permiso",
        (_, Some(AgentAttention::Question)) => "tiene una pregunta",
        (_, Some(AgentAttention::Plan)) => "tiene un plan",
        (_, Some(AgentAttention::Notification)) => "necesita atención",
        (Some(AgentRuntimeState::Working), _) => "trabajando",
        (Some(AgentRuntimeState::Waiting), _) => "esperando",
        (Some(AgentRuntimeState::Idle), _) => "listo",
        (None, _) => "shell",
    };
    match model.map(str::trim).filter(|model| !model.is_empty()) {
        Some(model) => format!("{kind} · {} · {activity}", compact_chrome_label(model, 20)),
        None => format!("{kind} · {activity}"),
    }
}

pub(crate) fn sidebar_location_line(branch: Option<&str>, path: &str) -> String {
    match branch {
        Some(branch) => format!("{branch}  ·  {path}"),
        None => path.to_owned(),
    }
}

pub(crate) struct SidebarWorkspaceAppearance {
    pub title: gpui::Rgba,
    pub path: gpui::Rgba,
    pub branch: gpui::Rgba,
    pub agent_fallback: gpui::Rgba,
    pub background: gpui::Rgba,
    pub border: gpui::Rgba,
}

pub(crate) fn sidebar_workspace_appearance(
    selected: bool,
    dirty: bool,
    behind: usize,
) -> SidebarWorkspaceAppearance {
    SidebarWorkspaceAppearance {
        title: if selected {
            colors().foreground
        } else {
            colors().muted
        },
        path: if selected {
            colors().muted
        } else {
            colors().subtle
        },
        branch: match (dirty, behind > 0) {
            (true, _) => colors().warning,
            (_, true) => colors().accent,
            _ if selected => colors().muted,
            _ => colors().subtle,
        },
        agent_fallback: if selected {
            colors().muted
        } else {
            colors().subtle
        },
        background: if selected {
            colors().elevated
        } else {
            colors().sidebar
        },
        border: if selected {
            colors().border_subtle
        } else {
            gpui::rgba(0x00000000)
        },
    }
}

pub(crate) fn sidebar_workspace_text_column(
    text_width: f32,
    title: &str,
    title_color: gpui::Rgba,
    agent_line: &str,
    agent_color: gpui::Rgba,
    location_line: &str,
    location_color: gpui::Rgba,
) -> Div {
    div()
        .w(px(text_width))
        .flex_none()
        .overflow_hidden()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(1.0))
        .child(sidebar_tab_line(title, title_color, 11.5, true, false))
        .child(sidebar_tab_line(agent_line, agent_color, 9.5, true, false))
        .child(sidebar_tab_line(
            location_line,
            location_color,
            8.5,
            false,
            true,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_agent_line_includes_model_only_when_reported() {
        assert_eq!(
            sidebar_agent_line(Some("Codex"), None, Some(AgentRuntimeState::Working), None),
            "Codex · trabajando"
        );
        assert_eq!(
            sidebar_agent_line(
                Some("Codex"),
                Some("gpt-5"),
                Some(AgentRuntimeState::Working),
                None
            ),
            "Codex · gpt-5 · trabajando"
        );
        assert_eq!(
            sidebar_location_line(Some("main"), "~/Dev/Vibra"),
            "main  ·  ~/Dev/Vibra"
        );
    }
}
