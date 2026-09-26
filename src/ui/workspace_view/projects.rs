//! Folder selection and project navigation. Existing terminals keep their cwd.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, MouseButton, MouseDownEvent, PathPromptOptions, PromptLevel, SharedString,
    Window, div, prelude::*, px, svg,
};
use uuid::Uuid;

use crate::AddProject;
use crate::domain::agents::{AgentAttention, AgentRuntimeState};
use crate::ui::agent_marks::agent_status_color;
use crate::ui::theme::{colors, surface_tint};

use super::chrome::sidebar_row_width;
use super::{
    ContextMenuKind, ProjectDrag, RightSidebarMode, SIDEBAR_CONTROL_SIZE, SIDEBAR_ROW_END_PADDING,
    WorkspaceSection, WorkspaceView, sidebar_tooltip,
};

impl WorkspaceView {
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
                    "Asociar carpeta"
                } else {
                    "Agregar proyecto"
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
                            Some(format!("No se pudo elegir la carpeta: {error}").into());
                        cx.notify();
                        return;
                    }
                };
                let root = match path.canonicalize() {
                    Ok(root) if root.is_dir() => root,
                    _ => {
                        this.persistence_error =
                            Some("La carpeta seleccionada ya no está disponible".into());
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
        let Some(project) = self.snapshot.projects.iter().find(|p| p.id == id) else {
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

    pub(super) fn confirm_remove_project(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.snapshot.projects.iter().find(|p| p.id == id) else {
            return;
        };
        let confirmation = window.prompt(
            PromptLevel::Warning,
            &format!("¿Quitar {} de Vibra?", project.name),
            Some("Se cerrarán sus pestañas y procesos. La carpeta y sus archivos se conservarán en el disco."),
            &["Cancelar", "Quitar proyecto"], cx,
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
        let avatar_size = 20.0;
        // Avatar, two 10 px gaps, and the status/new-tab slot.
        let label_width = (row_width
            - project_padding
            - SIDEBAR_ROW_END_PADDING
            - avatar_size
            - 20.0
            - controls_width)
            .max(48.0);
        let color = super::navigation::project_color(id);
        let initial = name
            .chars()
            .find(|character| character.is_alphanumeric())
            .map(|character| character.to_uppercase().collect::<String>())
            .unwrap_or_else(|| "·".to_owned());
        let activity = self.project_agent_activity(id);
        let drag = ProjectDrag {
            project_id: id,
            name: name.clone(),
        };
        div()
            .id(SharedString::from(format!("project-{id}")))
            .group("global-project")
            .w(px(row_width))
            .mb(px(2.0))
            .h(px(30.0))
            .pl(px(project_padding))
            .pr(px(SIDEBAR_ROW_END_PADDING))
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
            .cursor_pointer()
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
            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
            .can_drop(move |value, _, _| {
                value
                    .downcast_ref::<ProjectDrag>()
                    .is_some_and(|drag| drag.project_id != id)
            })
            .drag_over::<ProjectDrag>(|style, _, _, _| {
                style.border_t_2().border_color(colors().accent)
            })
            .on_drop(cx.listener(move |this, drag: &ProjectDrag, _, cx| {
                if this.snapshot.move_project(drag.project_id, Some(id)) {
                    this.persist(cx);
                }
            }))
            .child(
                div()
                    .size(px(avatar_size))
                    .flex_none()
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::Rgba { a: 0.18, ..color })
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(color)
                    .child(initial),
            )
            .child(
                div()
                    .w(px(label_width))
                    .flex_none()
                    .truncate()
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .font_weight(if selected {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::MEDIUM
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
                    .w(px(controls_width))
                    .h(px(SIDEBAR_CONTROL_SIZE))
                    .flex_none()
                    .relative()
                    // Agent activity, replaced by the new-tab button on hover.
                    .when_some(activity, |slot, (dot, count)| {
                        slot.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap(px(3.0))
                                .group_hover("global-project", |style| style.opacity(0.0))
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
                    .child(
                        div()
                            .id(SharedString::from(format!("project-new-tab-{id}")))
                            .absolute()
                            .inset_0()
                            .tooltip(|_, cx| sidebar_tooltip("Nueva pestaña · ⌘T", cx))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .opacity(0.0)
                            .group_hover("global-project", |style| style.opacity(1.0))
                            .hover(|s| s.bg(colors().hover))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
            )
            .into_any_element()
    }

    pub(super) fn empty_project_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.snapshot.selected_project();
        let name = project
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Vibra".into());
        let path = project.and_then(|p| p.directory()).map(PathBuf::from);
        let button = if project.is_some() {
            "Nueva terminal · ⌘T"
        } else {
            "Agregar proyecto · ⇧⌘O"
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
                    .child("Abre una terminal en esta carpeta para empezar"),
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
