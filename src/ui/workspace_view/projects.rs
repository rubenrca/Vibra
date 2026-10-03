//! Folder selection and project navigation. Existing terminals keep their cwd.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Bounds, Context, DragMoveEvent, MouseButton, MouseDownEvent, PathPromptOptions,
    Pixels, Point, PromptLevel, SharedString, Window, div, linear_color_stop, linear_gradient,
    prelude::*, px, svg,
};
use uuid::Uuid;

use crate::domain::agents::{AgentAttention, AgentRuntimeState};
use crate::domain::workspace::ProjectSnapshot;
use crate::ui::agent_marks::{agent_compact_badge, agent_status_color};
use crate::ui::theme::{colors, surface_tint};
use crate::{AddProject, GoToProject};

use super::chrome::sidebar_row_width;
use super::inbox::agent_state_label;
use super::{
    ContextMenuKind, DragGhost, PaneIdentity, ProjectDrag, ReorderDrag, ReorderSlot,
    RightSidebarMode, SIDEBAR_CONTROL_SIZE, WorkspaceSection, WorkspaceView, sidebar_tooltip,
};

/// A project row and the space below it.
const PROJECT_ROW_PITCH: f32 = 34.0;
const PROJECT_AGENT_ROW_PITCH: f32 = 28.0;
/// Agent rows sit right of a guide line under the folder icon, like a tree.
const PROJECT_AGENT_INDENT: f32 = 24.0;
const PROJECT_AGENT_GUIDE_X: f32 = 15.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProjectDiffStats {
    pub additions: usize,
    pub deletions: usize,
}

/// Notes and automations cycle through projects, then the unassigned option.
pub(super) fn next_library_project(
    projects: &[ProjectSnapshot],
    current: Option<Uuid>,
) -> Option<Uuid> {
    let next = projects
        .iter()
        .position(|project| Some(project.id) == current)
        .map_or(0, |index| index + 1);
    projects.get(next).map(|project| project.id)
}

impl WorkspaceView {
    pub(super) fn apply_project_diff_stats(
        &mut self,
        root: PathBuf,
        stats: Option<ProjectDiffStats>,
        cx: &mut Context<Self>,
    ) {
        // A project can be removed or linked to another folder during the poll.
        if !self.snapshot.projects.iter().any(|project| {
            project
                .directory()
                .is_some_and(|path| Path::new(path) == root)
        }) {
            return;
        }
        if self.project_diff_stats.get(&root).copied() == stats {
            return;
        }
        if let Some(stats) = stats {
            self.project_diff_stats.insert(root, stats);
        } else {
            self.project_diff_stats.remove(&root);
        }
        cx.notify();
    }

    pub(super) fn project_diff_stats(&self, id: Uuid) -> Option<ProjectDiffStats> {
        let root = self
            .snapshot
            .projects
            .iter()
            .find(|project| project.id == id)?
            .directory()?;
        self.project_diff_stats.get(Path::new(root)).copied()
    }

    pub(super) fn add_project(
        &mut self,
        _: &AddProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choose_project_folder(None, false, window, cx);
    }

    pub(super) fn choose_project_folder(
        &mut self,
        project_id: Option<Uuid>,
        open_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                if project_id.is_some() {
                    "Link folder"
                } else {
                    "Add project"
                }
                .into(),
            ),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = picker.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let path = match result {
                    Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                        Some(path) => path,
                        None => return,
                    },
                    Ok(Ok(None)) | Err(_) => return,
                    Ok(Err(error)) => {
                        this.persistence_error =
                            Some(format!("Could not choose the folder: {error}").into());
                        cx.notify();
                        return;
                    }
                };
                let root = match path.canonicalize() {
                    Ok(root) if root.is_dir() => root,
                    _ => {
                        this.persistence_error =
                            Some("The selected folder is no longer available".into());
                        cx.notify();
                        return;
                    }
                };
                let id = match project_id {
                    Some(id) => {
                        if !this.snapshot.set_project_directory(id, &root) {
                            return;
                        }
                        this.snapshot.select_project(id);
                        id
                    }
                    None => {
                        let existing = this
                            .snapshot
                            .projects
                            .iter()
                            .find(|project| {
                                project.directory().is_some_and(|path| {
                                    Path::new(path)
                                        .canonicalize()
                                        .is_ok_and(|path| path == root)
                                })
                            })
                            .map(|project| project.id);
                        if let Some(id) = existing {
                            this.snapshot.select_project(id);
                            id
                        } else {
                            this.snapshot.add_project(&root)
                        }
                    }
                };
                let empty = this.snapshot.selected_workspace().is_none();
                if open_tab || (project_id.is_none() && empty) {
                    this.snapshot.open_tab_in_project(id, true);
                }
                this.persistence_error = None;
                this.right_sidebar_mode = RightSidebarMode::Files;
                this.set_right_sidebar_visible(true, true, cx);
                this.set_left_sidebar_visible(true, true, cx);
                this.reconcile_terminal_views(cx);
                this.show_terminal_tab(window, cx);
            });
        })
        .detach();
    }

    /// Opens a terminal tab in the project, asking for its folder first
    /// when a migrated project has none.
    pub(super) fn open_project_tab(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.snapshot.project(id) else {
            return;
        };
        if project
            .directory()
            .is_none_or(|root| !Path::new(root).is_dir())
        {
            self.choose_project_folder(Some(id), true, window, cx);
            return;
        }
        if self.snapshot.open_tab_in_project(id, true).is_some() {
            self.reconcile_terminal_views(cx);
            self.show_terminal_tab(window, cx);
        }
    }

    pub(super) fn select_project(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.select_project(id) {
            // Keep the right panel as the user left it.
            self.show_terminal_tab(window, cx);
        }
    }

    /// Sidebar order: pinned projects first, preserving the order within each group.
    pub(super) fn visible_project_order(&self) -> Vec<Uuid> {
        let (pinned, others): (Vec<Uuid>, Vec<Uuid>) = self
            .snapshot
            .projects
            .iter()
            .map(|project| project.id)
            .partition(|id| self.settings.pinned_project_ids.contains(id));
        pinned.into_iter().chain(others).collect()
    }

    /// `⌥1`–`⌥8` select by sidebar position; `⌥9` selects the last project.
    pub(super) fn go_to_project(
        &mut self,
        action: &GoToProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_mode.is_some() || self.rename_prompt.is_some() {
            return;
        }
        let order = self.visible_project_order();
        let Some(index) = super::tabs::numbered_navigation_index(action.index, order.len()) else {
            return;
        };
        self.select_project(order[index], window, cx);
    }

    pub(super) fn confirm_remove_project(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.snapshot.project(id) else {
            return;
        };
        let confirmation = window.prompt(
            PromptLevel::Warning,
            &format!("Remove {} from Vibra?", project.name),
            Some(concat!(
                "Its tabs and processes will be closed. ",
                "The folder and its files will remain on disk.",
            )),
            &["Cancel", "Remove project"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if confirmation.await.ok() != Some(1) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                if this.snapshot.remove_project(id) {
                    if this.library.detach_project(id) {
                        this.persist_library(cx);
                    }
                    this.reconcile_terminal_views(cx);
                    this.show_terminal_tab(window, cx);
                }
            });
        })
        .detach();
    }

    /// The most urgent agent state in a project, as a status color, and how
    /// many agents are running there.
    fn project_agent_activity(&self, project_id: Uuid) -> Option<(gpui::Rgba, usize)> {
        let project = self
            .snapshot
            .projects
            .iter()
            .find(|project| project.id == project_id)?;
        let presences: Vec<_> = project
            .terminal_sessions()
            .filter_map(|session| self.resolved_agent_presence(session.id))
            .filter(|presence| presence.state != AgentRuntimeState::Idle)
            .collect();
        // Permission beats waiting, which beats working.
        let urgent =
            presences
                .iter()
                .max_by_key(|presence| match (presence.state, presence.attention) {
                    (AgentRuntimeState::Waiting, Some(AgentAttention::Permission)) => 2,
                    (AgentRuntimeState::Waiting, _) => 1,
                    _ => 0,
                })?;
        let color = agent_status_color(Some(urgent.state), urgent.attention)?;
        Some((color, presences.len()))
    }

    pub(super) fn project_sidebar_header(
        &self,
        id: Uuid,
        name: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row_width = sidebar_row_width(self.left_sidebar_width());
        let controls_width = SIDEBAR_CONTROL_SIZE;
        let selected = self.snapshot.selected_project_id == Some(id)
            && self.workspace_section == WorkspaceSection::Workspace;
        let project_padding = 6.0;
        let icon_slot_size = 20.0;
        let diff_stats = self
            .project_diff_stats(id)
            .filter(|stats| stats.additions > 0 || stats.deletions > 0);
        let hover_background = if selected {
            colors().selection
        } else {
            colors().hover
        };
        let color = colors().muted;
        let activity = self.project_agent_activity(id);
        let drag = ProjectDrag { project_id: id };
        let ghost = {
            let name = name.clone();
            DragGhost::new(7.0, colors().sidebar, move || {
                project_drag_ghost(color, name.clone(), project_padding, icon_slot_size)
            })
        };
        div()
            .id(SharedString::from(format!("project-{id}")))
            .group("global-project")
            .relative()
            .w(px(row_width))
            .mb(px(2.0))
            .h(px(32.0))
            .pl(px(project_padding))
            .pr(px(project_padding))
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .when(selected, |row| {
                row.bg(surface_tint(colors().selection, colors().sidebar))
            })
            .hover(move |row| {
                if selected {
                    row
                } else {
                    row.bg(surface_tint(colors().hover, colors().sidebar))
                }
            })
            .cursor_move()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.reorder_drag = Some(ReorderDrag::Project(id));
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_project(id, window, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_context_menu(
                        ContextMenuKind::Project { project_id: id },
                        event.position.x.into(),
                        event.position.y.into(),
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .child(ghost.measure())
            .on_drag(drag, move |_, _, _, cx| ghost.preview(cx))
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<ProjectDrag>, _, cx| {
                    let source = event.drag(cx).project_id;
                    this.hover_project(source, id, event.bounds, event.event.position, cx);
                }),
            )
            .child(
                div()
                    .size(px(icon_slot_size))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path("chrome-icons/folder.svg")
                            .size(px(14.0))
                            .text_color(color),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .font_weight(if selected {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(if selected {
                        colors().foreground
                    } else {
                        colors().muted
                    })
                    .group_hover("global-project", |label| {
                        label.text_color(colors().foreground)
                    })
                    .child(name),
            )
            .child(
                div()
                    .min_w(px(controls_width))
                    .h(px(SIDEBAR_CONTROL_SIZE))
                    .flex_none()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    // Keep activity beside the counts; only replace it when there is no diff.
                    .when_some(activity, |slot, (dot, count)| {
                        slot.child(
                            div()
                                .min_w(px(controls_width))
                                .h_full()
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(3.0))
                                .when(diff_stats.is_none(), |activity| {
                                    activity
                                        .group_hover("global-project", |style| style.opacity(0.0))
                                })
                                .when(count > 1, |slot| {
                                    slot.child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(colors().subtle)
                                            .child(count.to_string()),
                                    )
                                })
                                .child(div().size(px(7.0)).rounded_full().bg(dot)),
                        )
                    })
                    .when_some(diff_stats, |slot, stats| {
                        slot.child(
                            div()
                                .id(SharedString::from(format!("project-diff-{id}")))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .text_size(px(10.5))
                                .tooltip(move |_, cx| {
                                    sidebar_tooltip(
                                        format!(
                                            "Uncommitted changes: {} lines added, {} lines deleted",
                                            stats.additions, stats.deletions
                                        ),
                                        cx,
                                    )
                                })
                                .when(stats.additions > 0, |counts| {
                                    counts.child(
                                        div()
                                            .text_color(colors().success)
                                            .child(format!("+{}", stats.additions)),
                                    )
                                })
                                .when(stats.deletions > 0, |counts| {
                                    counts.child(
                                        div()
                                            .text_color(colors().danger)
                                            .child(format!("−{}", stats.deletions)),
                                    )
                                }),
                        )
                    })
                    // Overlay the action so hovering never shifts the name or counts.
                    .child(
                        div()
                            .absolute()
                            .right(px(-project_padding))
                            .top(px(-project_padding))
                            .bottom(px(-project_padding))
                            .w(px(controls_width + 24.0 + project_padding))
                            .pr(px(project_padding))
                            .rounded_r(px(7.0))
                            .flex()
                            .items_center()
                            .justify_end()
                            .opacity(0.0)
                            .group_hover("global-project", |style| style.opacity(1.0))
                            .when(diff_stats.is_some(), |overlay| {
                                overlay.bg(linear_gradient(
                                    90.0,
                                    linear_color_stop(hover_background, 0.0).opacity(0.0),
                                    linear_color_stop(
                                        hover_background,
                                        24.0 / (controls_width + 24.0 + project_padding),
                                    ),
                                ))
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("project-new-tab-{id}")))
                                    .size(px(controls_width))
                                    .flex_none()
                                    .tooltip(|_, cx| sidebar_tooltip("New tab · ⌘T", cx))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(5.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors().hover))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_project_tab(id, window, cx);
                                        cx.stop_propagation();
                                    }))
                                    .child(
                                        svg()
                                            .path("chrome-icons/plus.svg")
                                            .size(px(12.0))
                                            .text_color(colors().muted),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Keep live agent sessions visible across tabs, including idle agents
    /// between turns. Ordinary shells and remembered task titles stay out.
    pub(super) fn project_agent_sessions(
        &self,
        project_id: Uuid,
        cx: &Context<Self>,
    ) -> Vec<(Uuid, PaneIdentity)> {
        self.snapshot
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .into_iter()
            .flat_map(|project| project.terminal_sessions())
            .enumerate()
            .filter_map(|(index, session)| {
                let identity = self.pane_identity(session, index, cx);
                identity
                    .agent_kind
                    .is_some()
                    .then_some((session.id, identity))
            })
            .collect()
    }

    fn project_agent_row(
        &self,
        session_id: Uuid,
        identity: PaneIdentity,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.workspace_section == WorkspaceSection::Workspace
            && !self.review_covers_terminal(cx)
            && self
                .snapshot
                .selected_session()
                .is_some_and(|session| session.id == session_id);
        let state = identity.agent_state.unwrap_or(AgentRuntimeState::Idle);
        let status = agent_state_label(state, identity.agent_attention);
        let tooltip = format!(
            "{} · {}\n{}{}",
            identity.agent_kind.as_deref().unwrap_or("Agent"),
            status,
            identity.title,
            identity
                .detail
                .as_ref()
                .map(|detail| format!("\n{detail}"))
                .unwrap_or_default(),
        );
        div()
            .id(SharedString::from(format!("project-agent-{session_id}")))
            .w(px(
                sidebar_row_width(self.left_sidebar_width()) - PROJECT_AGENT_INDENT
            ))
            .h(px(PROJECT_AGENT_ROW_PITCH - 2.0))
            .mb(px(2.0))
            .pl(px(6.0))
            .pr(px(8.0))
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(surface_tint(colors().selection, colors().sidebar))
            })
            .hover(move |row| {
                if selected {
                    row
                } else {
                    row.bg(surface_tint(colors().hover, colors().sidebar))
                }
            })
            .tooltip(move |_, cx| sidebar_tooltip(tooltip.clone(), cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_pane(session_id, window, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_context_menu(
                        ContextMenuKind::Pane { session_id },
                        event.position.x.into(),
                        event.position.y.into(),
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .child(agent_compact_badge(
                identity.agent_kind.as_deref(),
                identity.agent_state,
                identity.agent_attention,
                selected,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(if selected {
                        colors().foreground
                    } else {
                        colors().muted
                    })
                    .child(identity.title),
            )
            .into_any_element()
    }

    /// One sidebar section, with a gap where a project dragged within it lands.
    pub(super) fn project_rows(
        &self,
        projects: Vec<(Uuid, String)>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let order: Vec<_> = projects.iter().map(|(id, _)| *id).collect();
        let mut agents: std::collections::HashMap<_, _> = order
            .iter()
            .map(|id| (*id, self.project_agent_sessions(*id, cx)))
            .collect();
        let source = match self.reorder_drag {
            Some(ReorderDrag::Project(id)) if cx.has_active_drag() && order.contains(&id) => {
                Some(id)
            }
            _ => None,
        };
        let landing = source.and(self.project_drop);
        let slots = super::reorder_slots(&order, source, landing);
        let section = order
            .first()
            .is_some_and(|id| self.settings.pinned_project_ids.contains(id));
        let mut motion = self.project_motion.borrow_mut();
        let motion = &mut motion[usize::from(section)];
        let project_height = |id: Uuid| {
            PROJECT_ROW_PITCH + agents.get(&id).map_or(0, Vec::len) as f32 * PROJECT_AGENT_ROW_PITCH
        };
        let gap_height = source.map_or(PROJECT_ROW_PITCH, project_height);
        motion.update_with_sizes(&slots, |slot| {
            px(match slot {
                ReorderSlot::Item(id) => project_height(id),
                ReorderSlot::Gap => gap_height,
            })
        });
        let mut projects = projects;
        slots
            .into_iter()
            .map(|slot| match slot {
                ReorderSlot::Item(id) => {
                    let index = projects
                        .iter()
                        .position(|(project, _)| *project == id)
                        .expect("project in section");
                    let (_, name) = projects.swap_remove(index);
                    let agent_rows: Vec<_> = agents
                        .remove(&id)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(session_id, identity)| {
                            self.project_agent_row(session_id, identity, cx)
                        })
                        .collect();
                    super::slide_into_place(
                        div()
                            .relative()
                            .child(self.project_sidebar_header(id, name, cx))
                            .when(!agent_rows.is_empty(), |project| {
                                project.child(
                                    div()
                                        .relative()
                                        .pl(px(PROJECT_AGENT_INDENT))
                                        .child(
                                            div()
                                                .absolute()
                                                .left(px(PROJECT_AGENT_GUIDE_X))
                                                .top(px(0.0))
                                                .bottom(px(6.0))
                                                .w(px(1.0))
                                                .bg(colors().border_subtle),
                                        )
                                        .children(agent_rows),
                                )
                            }),
                        motion.slide(id),
                        "project",
                        true,
                    )
                }
                ReorderSlot::Gap => super::reorder_gap()
                    .w(px(sidebar_row_width(self.left_sidebar_width())))
                    .h(px(gap_height - 2.0))
                    .mb(px(2.0))
                    .rounded(px(7.0))
                    .into_any_element(),
            })
            .collect()
    }

    /// Projects in the same sidebar section as `project_id`, in order.
    fn project_section_order(&self, project_id: Uuid) -> Vec<Uuid> {
        let pinned = self.settings.pinned_project_ids.contains(&project_id);
        self.snapshot
            .projects
            .iter()
            .filter(|project| self.settings.pinned_project_ids.contains(&project.id) == pinned)
            .map(|project| project.id)
            .collect()
    }

    /// Crossing the middle of a row moves the gap past it, which slides it aside.
    fn hover_project(
        &mut self,
        source: Uuid,
        target: Uuid,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if source == target || !bounds.contains(&position) {
            return;
        }
        let order = self.project_section_order(source);
        if !order.contains(&target) {
            return;
        }
        let y = f32::from(position.y - bounds.top()) / f32::from(bounds.size.height).max(1.0);
        self.set_project_drop(
            Some(super::landing_beside(&order, Some(source), target, y > 0.5)),
            cx,
        );
    }

    fn set_project_drop(&mut self, drop: Option<Option<Uuid>>, cx: &mut Context<Self>) {
        if self.project_drop != drop {
            self.project_drop = drop;
            cx.notify();
        }
    }

    /// The list accepts the drop and forgets the landing once the pointer
    /// leaves it, so the dragged project returns to its place.
    pub(super) fn project_list_drop_area<E: InteractiveElement>(
        &self,
        list: E,
        cx: &mut Context<Self>,
    ) -> E {
        list.can_drop(|value, _, _| value.downcast_ref::<ProjectDrag>().is_some())
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ProjectDrag>, _, cx| {
                    if !event.bounds.contains(&event.event.position) {
                        this.set_project_drop(None, cx);
                    }
                }),
            )
            .on_drop(cx.listener(|this, drag: &ProjectDrag, _, cx| {
                if let Some(before) = this.project_drop.take() {
                    this.drop_project(drag, before, cx);
                }
            }))
    }

    /// The space below the last project moves a dragged project to the end.
    pub(super) fn project_list_end_target(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("global-project-drop-end")
            .h(px(28.0))
            .w_full()
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ProjectDrag>, _, cx| {
                    let source = event.drag(cx).project_id;
                    if event.bounds.contains(&event.event.position)
                        && !this.settings.pinned_project_ids.contains(&source)
                    {
                        this.set_project_drop(Some(None), cx);
                    }
                }),
            )
            .into_any_element()
    }

    fn drop_project(&mut self, drag: &ProjectDrag, before: Option<Uuid>, cx: &mut Context<Self>) {
        self.reorder_drag = None;
        if self.snapshot.move_project(drag.project_id, before) {
            self.persist(cx);
        }
        cx.notify();
    }

    pub(super) fn empty_project_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.snapshot.selected_project();
        let name = project
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Vibra".into());
        let path = project.and_then(|p| p.directory()).map(PathBuf::from);
        let button = if project.is_some() {
            "New terminal · ⌘T"
        } else {
            "Add project · ⇧⌘O"
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .text_size(px(16.0))
                    .text_color(colors().foreground)
                    .child(name),
            )
            .when_some(path, |content, path| {
                content.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors().subtle)
                        .child(path.display().to_string()),
                )
            })
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(colors().muted)
                    .child("Open a terminal in this folder to get started"),
            )
            .child(
                div()
                    .id("empty-project-new-tab")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(colors().elevated)
                    .text_size(px(12.0))
                    .text_color(colors().foreground)
                    .hover(|s| s.bg(colors().hover))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_terminal_tab_in_project(window, cx)
                    }))
                    .child(button),
            )
            .into_any_element()
    }
}

/// The row as it looks while selected, without controls that cannot be used mid-drag.
fn project_drag_ghost(
    color: gpui::Rgba,
    name: String,
    padding: f32,
    icon_slot_size: f32,
) -> AnyElement {
    div()
        .h(px(32.0))
        .px(px(padding))
        .rounded(px(7.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .bg(surface_tint(colors().selection, colors().sidebar))
        .child(
            div()
                .size(px(icon_slot_size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path("chrome-icons/folder.svg")
                        .size(px(14.0))
                        .text_color(color),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(13.0))
                .line_height(px(18.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(colors().foreground)
                .child(name),
        )
        .into_any_element()
}
