//! Spaces: a project follows the terminal it has selected. Its folder is the
//! repository that terminal is in (else the terminal's own folder) and its
//! name is that folder's, unless the user renamed it. ⌘N opens one with a
//! terminal in the home folder, and a space lives only while it has terminals.
//! The Agents list under the spaces shows every agent, wherever it runs.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, Context, Div, DragMoveEvent, MouseButton, MouseDownEvent, Pixels,
    SharedString, Timer, Window, div, prelude::*, px, relative, svg,
};
use uuid::Uuid;

use crate::NewSpace;
use crate::domain::agents::{AgentAttention, AgentRuntimeState};
use crate::infrastructure::settings::{MAX_AGENTS_PANEL_HEIGHT, MIN_AGENTS_PANEL_HEIGHT};
use crate::ports::files::FileSystemPort;
use crate::ui::agent_marks::{agent_compact_badge, agent_status_color};
use crate::ui::theme::{colors, surface_tint};

use super::chrome::{SIDEBAR_ROW_INSET, sidebar_row_width};
use super::navigation::section_label;
use super::{
    ContextMenuKind, PaneIdentity, SIDEBAR_CONTROL_SIZE, SidebarResizeDragView, WorkspaceSection,
    WorkspaceView, sidebar_tooltip,
};

/// Time-in-state labels count minutes, so this is often enough to keep them true.
const AGENT_CLOCK_TICK: Duration = Duration::from_secs(30);
/// An agent card and the gap below it.
const AGENT_CARD_PITCH: f32 = 64.0;
/// Room the spaces keep above Agents while it is dragged taller.
const MIN_SPACES_HEIGHT: f32 = 96.0;
/// Grab area of the line between the spaces and Agents.
const AGENTS_RESIZE_HANDLE: f32 = 7.0;

/// Drag payload of the line between the spaces and the Agents list.
#[derive(Clone, Copy)]
pub(super) struct AgentsResizeDrag;

/// A space's label: `~` for the home folder, else the folder's name.
pub(super) fn space_name(root: &Path, home: Option<&Path>) -> String {
    if Some(root) == home {
        return "~".into();
    }
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// Where a space whose terminal is in `cwd` belongs, and what it is called.
pub(super) fn locate_space(
    files: &dyn FileSystemPort,
    cwd: &Path,
    home: Option<&Path>,
) -> Option<(PathBuf, String)> {
    let root = files.space_root(cwd, home)?;
    let name = space_name(&root, home);
    Some((root, name))
}

/// The space's mark: its initial on a tint chosen by its name, so a
/// repository keeps its color in every space that visits it.
pub(super) fn space_avatar(name: &str) -> Div {
    let color = space_color(name);
    let initial: String = name
        .chars()
        .find(|character| character.is_alphanumeric())
        .or_else(|| name.chars().next())
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_default();
    div()
        .size(px(SIDEBAR_CONTROL_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .bg(gpui::Hsla::from(color).opacity(0.18))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(color)
        .child(initial)
}

fn space_color(name: &str) -> gpui::Rgba {
    // FNV-1a keeps the color stable across launches and Rust releases.
    let hash = name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    let palette = [
        colors().accent,
        colors().success,
        colors().warning,
        colors().folder,
        colors().git_deleted,
    ];
    palette[hash as usize % palette.len()]
}

/// What an agent needs, which orders the Agents list: most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum AgentGroup {
    NeedsYou,
    Working,
    Idle,
}

impl AgentGroup {
    fn of(state: AgentRuntimeState) -> Self {
        match state {
            AgentRuntimeState::Waiting => Self::NeedsYou,
            AgentRuntimeState::Working => Self::Working,
            AgentRuntimeState::Idle => Self::Idle,
        }
    }
}

pub(super) struct SidebarAgent {
    pub(super) session_id: Uuid,
    pub(super) space: String,
    pub(super) branch: Option<String>,
    pub(super) identity: PaneIdentity,
    pub(super) since: Option<Instant>,
}

impl WorkspaceView {
    /// Moves every space to where its selected terminal is. Returns whether
    /// one moved or was renamed; callers refresh what depends on its folder.
    pub(super) fn follow_spaces(&mut self) -> bool {
        let files = self.file_port.clone();
        let home = self.home_directory.clone();
        self.snapshot
            .follow_space_locations(|cwd| locate_space(files.as_ref(), cwd, home.as_deref()))
    }

    pub(super) fn new_space_action(
        &mut self,
        _: &NewSpace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_space(window, cx);
    }

    /// Where a new space's first terminal starts.
    pub(super) fn home_folder(&self) -> PathBuf {
        self.home_directory
            .clone()
            .filter(|home| home.is_dir())
            .unwrap_or_else(|| self.launch_directory.clone())
    }

    /// `⌘N`: a terminal in the home folder, in a space of its own that
    /// becomes wherever the terminal goes.
    pub(super) fn new_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.home_folder();
        let name = space_name(&root, self.home_directory.as_deref());
        let id = self.snapshot.add_space(&root, name);
        self.snapshot.open_tab_in_project(id, true);
        self.set_left_sidebar_visible(true, true, cx);
        self.reconcile_terminal_views(cx);
        self.show_terminal_tab(window, cx);
    }

    /// Spaces live while they have terminals. The selected one also stays
    /// while it shows a review or document. Returns whether any went away.
    pub(super) fn drop_empty_spaces(&mut self, cx: &mut Context<Self>) -> bool {
        let keep = self
            .snapshot
            .selected_project_id
            .filter(|_| self.diff_view.read(cx).review_expanded());
        let removed = self.snapshot.remove_empty_spaces(keep);
        if removed.is_empty() {
            return false;
        }
        let pinned = self.settings.pinned_project_ids.len();
        self.settings
            .pinned_project_ids
            .retain(|id| !removed.contains(id));
        if self.settings.pinned_project_ids.len() != pinned {
            self.persist_settings(cx);
        }
        true
    }

    /// The Agents list grows from its bottom edge up to the pointer, leaving
    /// the spaces room above it. The size is saved when the drag ends.
    pub(super) fn resize_agents_panel(
        &mut self,
        event: &DragMoveEvent<AgentsResizeDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bottom: f32 = event.bounds.bottom().into();
        let available: f32 = event.bounds.size.height.into();
        let pointer: f32 = event.event.position.y.into();
        let tallest =
            (available - MIN_SPACES_HEIGHT).clamp(MIN_AGENTS_PANEL_HEIGHT, MAX_AGENTS_PANEL_HEIGHT);
        let height = (bottom - pointer).clamp(MIN_AGENTS_PANEL_HEIGHT, tallest);
        if (self.settings.agents_panel_height - height).abs() < 0.5 {
            return;
        }
        self.settings.agents_panel_height = height;
        self.sidebar_resize_dirty = true;
        cx.notify();
    }

    /// Records when an agent entered its current state; the list shows how long ago.
    pub(super) fn track_agent_state_since(
        &mut self,
        pane_id: Uuid,
        state: Option<(AgentRuntimeState, Option<AgentAttention>)>,
    ) {
        let previous = self
            .agent_activity_seen
            .get(&pane_id)
            .map(|seen| (seen.state, seen.attention));
        match state {
            None => {
                self.agent_state_since.remove(&pane_id);
            }
            Some(state) if previous != Some(state) => {
                self.agent_state_since.insert(pane_id, Instant::now());
            }
            Some(_) => {}
        }
    }

    pub(super) fn start_agent_clock(&mut self, cx: &mut Context<Self>) {
        self._agent_clock_task = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(AGENT_CLOCK_TICK).await;
                let alive = this.update(cx, |this, cx| {
                    if !this.agent_state_since.is_empty() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    /// Every agent in any space, grouped by what it needs, the most recent
    /// change first within a group. A remembered task title alone is not an agent.
    pub(super) fn sidebar_agents(
        &self,
        cx: &Context<Self>,
    ) -> Vec<(AgentGroup, Vec<SidebarAgent>)> {
        let mut groups: Vec<(AgentGroup, Vec<SidebarAgent>)> = Vec::new();
        for project in &self.snapshot.projects {
            for (index, session) in project.terminal_sessions().enumerate() {
                let identity = self.pane_identity(session, index, cx);
                let (Some(_), Some(state)) = (&identity.agent_kind, identity.agent_state) else {
                    continue;
                };
                let group = AgentGroup::of(state);
                let agent = SidebarAgent {
                    session_id: session.id,
                    space: project.name.clone(),
                    branch: self.project_branch(project.id),
                    identity,
                    since: self.agent_state_since.get(&session.id).copied(),
                };
                match groups.iter_mut().find(|(existing, _)| *existing == group) {
                    Some((_, agents)) => agents.push(agent),
                    None => groups.push((group, vec![agent])),
                }
            }
        }
        groups.sort_by_key(|(group, _)| *group);
        for (_, agents) in &mut groups {
            // Stable, so agents with no recorded change keep sidebar order.
            agents.sort_by_key(|agent| std::cmp::Reverse(agent.since));
        }
        groups
    }

    pub(super) fn agents_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let groups = self.sidebar_agents(cx);
        let empty = groups.is_empty();
        // Cards carry their own status, so the groups only set the order.
        let cards: Vec<AnyElement> = groups
            .into_iter()
            .flat_map(|(_, agents)| agents)
            .map(|agent| self.sidebar_agent_card(agent, cx))
            .collect();
        // Shrinks before overflowing a short window; the spaces give way first.
        div()
            .relative()
            .h(px(self.settings.agents_panel_height))
            .min_h(px(MIN_AGENTS_PANEL_HEIGHT))
            .flex_shrink()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .id("agents-resize-handle")
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(-AGENTS_RESIZE_HANDLE / 2.0))
                    .h(px(AGENTS_RESIZE_HANDLE))
                    .cursor_ns_resize()
                    .on_drag(AgentsResizeDrag, |_, _, _, cx| {
                        cx.new(|_| SidebarResizeDragView)
                    }),
            )
            .child(
                div().px(px(SIDEBAR_ROW_INSET)).child(
                    section_label("Agents").child(
                        div()
                            .id("sidebar-agent-activity")
                            .group("sidebar-agent-activity")
                            .size(px(22.0))
                            .flex_none()
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|button| {
                                button.bg(surface_tint(colors().hover, colors().sidebar))
                            })
                            .tooltip(|_, cx| sidebar_tooltip("Agent activity", cx))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.work_inbox.activity = true;
                                this.select_section(WorkspaceSection::Inbox, window, cx);
                            }))
                            .child(
                                svg()
                                    .path("chrome-icons/activity.svg")
                                    .size(px(15.0))
                                    .flex_none()
                                    .text_color(colors().muted)
                                    .group_hover("sidebar-agent-activity", |icon| {
                                        icon.text_color(colors().foreground)
                                    }),
                            ),
                    ),
                ),
            )
            .child(
                div()
                    .id("sidebar-agents")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px(px(SIDEBAR_ROW_INSET))
                    .pb_2()
                    .when(empty, |list| {
                        list.child(
                            div()
                                .px(px(8.0))
                                .pt(px(2.0))
                                .text_size(px(12.0))
                                .line_height(px(17.0))
                                .text_color(colors().subtle)
                                .child(concat!(
                                    "No agents running. Agents show up here ",
                                    "when you start one in a terminal.",
                                )),
                        )
                    })
                    .children(cards),
            )
            .into_any_element()
    }

    /// The agent card: agent and status, task title, then its space and branch.
    fn sidebar_agent_card(&self, agent: SidebarAgent, cx: &mut Context<Self>) -> AnyElement {
        let SidebarAgent {
            session_id,
            space,
            branch,
            identity,
            since,
        } = agent;
        let selected = self.workspace_section == WorkspaceSection::Workspace
            && !self.review_covers_terminal(cx)
            && self
                .snapshot
                .selected_session()
                .is_some_and(|session| session.id == session_id);
        let kind = identity
            .agent_kind
            .clone()
            .unwrap_or_else(|| "Agent".into());
        let status = identity.agent_state.map(|state| {
            let label = agent_state_short_label(state, identity.agent_attention);
            (
                match since {
                    Some(since) => format!("{label} · {}", elapsed_label(since.elapsed())),
                    None => label.to_owned(),
                },
                agent_status_color(Some(state), identity.agent_attention)
                    .unwrap_or_else(|| colors().subtle),
            )
        });
        let tooltip = format!(
            "{}{}",
            identity.title,
            identity
                .detail
                .as_ref()
                .map(|detail| format!("\n{detail}"))
                .unwrap_or_default(),
        );
        let card_bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        div()
            .id(SharedString::from(format!("sidebar-agent-{session_id}")))
            .relative()
            .w(px(sidebar_row_width(self.left_sidebar_width())))
            .h(px(AGENT_CARD_PITCH - 2.0))
            .flex_none()
            .mb(px(2.0))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(7.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .cursor_pointer()
            .when(selected, |card| {
                card.bg(surface_tint(colors().selection, colors().sidebar))
            })
            .hover(move |card| {
                if selected {
                    card
                } else {
                    card.bg(surface_tint(colors().hover, colors().sidebar))
                }
            })
            .tooltip(move |_, cx| sidebar_tooltip(tooltip.clone(), cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_pane(session_id, window, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(MouseButton::Right, {
                let card_bounds = card_bounds.clone();
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    let card = card_bounds.get();
                    this.open_sidebar_context_menu(
                        ContextMenuKind::Pane { session_id },
                        card.top().into(),
                        card.bottom().into(),
                        cx,
                    );
                    cx.stop_propagation();
                })
            })
            .child(super::measure_bounds(card_bounds))
            .child(
                div()
                    .h(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(agent_compact_badge(
                        identity.agent_kind.as_deref(),
                        None,
                        None,
                        selected,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(colors().muted)
                            .child(kind),
                    )
                    .when_some(status, |line, (label, color)| {
                        line.child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .text_size(px(11.0))
                                .text_color(colors().subtle)
                                .child(div().size(px(6.0)).rounded_full().bg(color))
                                .child(label),
                        )
                    }),
            )
            .child(
                div()
                    .h(px(18.0))
                    .truncate()
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().foreground)
                    .child(identity.title),
            )
            .child(
                div()
                    .h(px(16.0))
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(11.5))
                    .line_height(px(16.0))
                    .text_color(colors().subtle)
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.5))
                            .truncate()
                            .child(space),
                    )
                    .when_some(branch, |line, branch| {
                        line.child(
                            svg()
                                .path("chrome-icons/git-branch.svg")
                                .size(px(12.0))
                                .flex_none()
                                .text_color(colors().subtle),
                        )
                        .child(div().min_w(px(0.0)).truncate().child(branch))
                    }),
            )
            .into_any_element()
    }
}

/// Fits the card's top line; the Inbox keeps the longer wording.
fn agent_state_short_label(
    state: AgentRuntimeState,
    attention: Option<AgentAttention>,
) -> &'static str {
    match (state, attention) {
        (AgentRuntimeState::Waiting, Some(AgentAttention::Permission)) => "Permission",
        (AgentRuntimeState::Waiting, _) => "Waiting",
        (AgentRuntimeState::Working, _) => "Working",
        (AgentRuntimeState::Idle, _) => "Idle",
    }
}

/// `<1m`, `5m`, `3h`, `2d`.
fn elapsed_label(elapsed: Duration) -> String {
    let minutes = elapsed.as_secs() / 60;
    match minutes {
        0 => "<1m".into(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_are_named_after_their_folder_and_home_is_a_tilde() {
        let home = Path::new("/Users/demo");
        assert_eq!(space_name(home, Some(home)), "~");
        assert_eq!(space_name(&home.join("Dev/Vibra"), Some(home)), "Vibra");
        assert_eq!(space_name(Path::new("/"), Some(home)), "/");
        assert_eq!(space_name(home, None), "demo");
    }

    #[test]
    fn agent_times_read_in_the_largest_whole_unit() {
        assert_eq!(elapsed_label(Duration::from_secs(59)), "<1m");
        assert_eq!(elapsed_label(Duration::from_secs(5 * 60 + 30)), "5m");
        assert_eq!(elapsed_label(Duration::from_secs(3 * 3600 + 59)), "3h");
        assert_eq!(elapsed_label(Duration::from_secs(2 * 86_400)), "2d");
    }
}
