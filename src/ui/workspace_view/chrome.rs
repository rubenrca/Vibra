use std::path::Path;

use gpui::{Div, SharedString, Stateful, div, prelude::*, px, svg};

use crate::domain::workspace::WorkspaceSplitAxis;
use crate::infrastructure::automation::{AgentAttention, AgentRuntimeState};
use crate::ui::agent_marks::{SIDEBAR_AGENT_MARK_SIZE, agent_sidebar_badge, agent_status_color};
use crate::ui::theme::{MONO_FONT, colors, floating_surface, mix, surface, surface_tint};

use super::SidebarWorkspaceMeta;

pub(crate) const PANEL_GAP: f32 = 4.0;
pub(crate) const PANEL_RADIUS: f32 = 10.0;
// Projects and sessions share the same horizontal bounds and leading edge.
pub(crate) const SIDEBAR_ROW_INSET: f32 = 8.0;
pub(crate) const SIDEBAR_ROW_PADDING: f32 = 6.0;
pub(crate) const SIDEBAR_ROW_END_PADDING: f32 = 2.0;
pub(crate) const SIDEBAR_ROW_RADIUS: f32 = 6.0;
pub(crate) const SIDEBAR_CONTROL_SIZE: f32 = 20.0;
pub(crate) const SIDEBAR_SESSION_MENU_SPACE: f32 = SIDEBAR_CONTROL_SIZE + 4.0;

/// Gutter between bento tiles. The window supplies its background; tiles own the borders.
pub(crate) fn split_gutter(id: impl Into<SharedString>, axis: WorkspaceSplitAxis) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .hover(|divider| divider.bg(surface(colors().hover)))
        .when(axis == WorkspaceSplitAxis::Horizontal, |divider| {
            divider.w(px(PANEL_GAP)).h_full().cursor_ew_resize()
        })
        .when(axis == WorkspaceSplitAxis::Vertical, |divider| {
            divider.h(px(PANEL_GAP)).w_full().cursor_ns_resize()
        })
}

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
        .bg(surface(background))
        .border_1()
        .border_color(colors().border_subtle)
        .child(
            div()
                .w(px(full_width))
                .h_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(PANEL_RADIUS))
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
    let activity = sidebar_agent_activity(state, attention);
    match model.map(str::trim).filter(|model| !model.is_empty()) {
        Some(model) => format!("{kind} · {model} · {activity}"),
        None => format!("{kind} · {activity}"),
    }
}

fn sidebar_agent_activity(
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
) -> &'static str {
    match (state, attention) {
        (_, Some(AgentAttention::Permission)) => "pide permiso",
        (_, Some(AgentAttention::Question)) => "tiene una pregunta",
        (_, Some(AgentAttention::Plan)) => "tiene un plan",
        (_, Some(AgentAttention::Notification)) => "necesita atención",
        (Some(AgentRuntimeState::Working), _) => "trabajando",
        (Some(AgentRuntimeState::Waiting), _) => "esperando",
        (Some(AgentRuntimeState::Idle), _) => "listo",
        (None, _) => "shell",
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
    pub branch: gpui::Rgba,
    pub background: gpui::Rgba,
    pub hover: gpui::Rgba,
}

pub(crate) fn sidebar_workspace_appearance(
    selected: bool,
    dirty: bool,
    behind: usize,
) -> SidebarWorkspaceAppearance {
    let theme = colors();
    let selected_surface = mix(theme.sidebar, theme.foreground, 0.115);
    SidebarWorkspaceAppearance {
        title: if selected {
            theme.foreground
        } else {
            mix(theme.foreground, theme.muted, 0.35)
        },
        branch: match (dirty, behind > 0) {
            (true, _) => theme.warning,
            (_, true) => theme.accent,
            _ if selected => theme.muted,
            _ => mix(theme.muted, theme.subtle, 0.6),
        },
        background: if selected {
            surface_tint(selected_surface, theme.sidebar)
        } else {
            gpui::rgba(0x00000000)
        },
        hover: if selected {
            surface_tint(
                mix(selected_surface, theme.foreground, 0.025),
                theme.sidebar,
            )
        } else {
            surface_tint(mix(theme.sidebar, theme.foreground, 0.045), theme.sidebar)
        },
    }
}

/// The session row and its drag preview share the same typography and content.
#[derive(Clone)]
pub(crate) struct SidebarSessionCard {
    pub context: String,
    pub title: String,
    pub branch: Option<String>,
    pub path: String,
    pub selected: bool,
    pub dirty: bool,
    pub behind: usize,
    pub agent_kind: Option<String>,
    pub agent_state: Option<AgentRuntimeState>,
    pub agent_attention: Option<AgentAttention>,
    pub agent_model: Option<String>,
    pub width: f32,
}

struct SidebarTooltip(SharedString);

impl gpui::Render for SidebarTooltip {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(420.0))
            .px_3()
            .py_2()
            .rounded(px(7.0))
            .bg(floating_surface(colors().elevated))
            .border_1()
            .border_color(colors().border_subtle)
            .shadow_sm()
            .text_size(px(11.0))
            .text_color(colors().foreground)
            .child(self.0.clone())
    }
}

pub(crate) fn sidebar_tooltip(label: impl Into<SharedString>, cx: &mut gpui::App) -> gpui::AnyView {
    cx.new(|_| SidebarTooltip(label.into())).into()
}

pub(crate) fn sidebar_session_detail(card: &SidebarSessionCard, full_path: &str) -> String {
    format!(
        "{}\n{}\n{}",
        card.title,
        sidebar_agent_line(
            card.agent_kind.as_deref(),
            card.agent_model.as_deref(),
            card.agent_state,
            card.agent_attention,
        ),
        sidebar_location_line(card.branch.as_deref(), full_path),
    )
}

pub(crate) fn sidebar_workspace_content(card: &SidebarSessionCard) -> Div {
    let appearance = sidebar_workspace_appearance(card.selected, card.dirty, card.behind);
    // Match the row padding, including in the drag preview.
    let width = (card.width - 2.0 * SIDEBAR_ROW_PADDING).max(80.0);
    let activity = match (card.agent_state, card.agent_attention) {
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Permission)) => Some("Permiso"),
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Question)) => Some("Pregunta"),
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Plan)) => Some("Plan"),
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Notification)) => Some("Atención"),
        (Some(AgentRuntimeState::Waiting), _) => Some("En espera"),
        (Some(AgentRuntimeState::Working), _) => Some("Trabajando"),
        _ => None,
    };
    let status_width = activity.map_or(0.0, |label| label.chars().count() as f32 * 5.0 + 6.0);
    let status_color =
        agent_status_color(card.agent_state, card.agent_attention).unwrap_or(colors().muted);
    let context_width = width
        - SIDEBAR_AGENT_MARK_SIZE
        - 6.0
        - status_width
        - if activity.is_some() { 6.0 } else { 0.0 };
    let location = card.branch.as_deref().unwrap_or(&card.path);
    let metadata_chrome = 10.0 + 3.0;
    div()
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(1.5))
        .overflow_hidden()
        .child(
            div()
                .h(px(13.0))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(agent_sidebar_badge(
                    card.agent_kind.as_deref(),
                    card.selected,
                ))
                .child(
                    div()
                        .w(px(context_width))
                        .flex_none()
                        .truncate()
                        .text_size(px(9.5))
                        .line_height(px(13.0))
                        .text_color(colors().subtle)
                        .child(card.context.clone()),
                )
                .when_some(activity, |row, activity| {
                    row.child(
                        div()
                            .w(px(status_width))
                            .h(px(13.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(3.0))
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(status_color)
                            .child(
                                div()
                                    .size(px(3.0))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(status_color),
                            )
                            .child(activity),
                    )
                }),
        )
        .child(
            sidebar_tab_line(&card.title, appearance.title, 11.5, true, false)
                .h(px(17.0))
                .flex_none()
                .line_height(px(17.0)),
        )
        .child(
            div()
                .h(px(13.0))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(
                    svg()
                        .path(if card.branch.is_some() {
                            "chrome-icons/git-branch.svg"
                        } else {
                            "chrome-icons/folder.svg"
                        })
                        .size(px(10.0))
                        .flex_none()
                        .text_color(colors().subtle),
                )
                .child(
                    div()
                        .w(px(width - metadata_chrome))
                        .group_hover("sidebar-session", move |style| {
                            style.w(px(width - metadata_chrome - SIDEBAR_SESSION_MENU_SPACE))
                        })
                        .flex_none()
                        .truncate()
                        .text_size(px(9.5))
                        .line_height(px(13.0))
                        .text_color(appearance.branch)
                        .child(location.to_owned()),
                ),
        )
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
