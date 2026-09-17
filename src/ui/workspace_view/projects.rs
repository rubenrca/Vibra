//! Folder selection and project navigation. Existing terminals keep their cwd.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, Div, MouseButton, MouseDownEvent, PathPromptOptions, PromptLevel,
    SharedString, Stateful, Window, div, prelude::*, px, svg,
};
use uuid::Uuid;

use crate::AddProject;
use crate::domain::workspace::{ProjectSnapshot, SidebarEntry};
use crate::ui::theme::{colors, mix, surface_tint};

use super::{
    ContextMenuKind, LeftSidebarMode, PaletteMode, ProjectDrag, SIDEBAR_CONTROL_SIZE,
    SIDEBAR_ROW_END_PADDING, SIDEBAR_ROW_INSET, SIDEBAR_ROW_PADDING, SIDEBAR_ROW_RADIUS,
    SidebarWorkspaceDrag, WorkspaceView, sidebar_tooltip,
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
        create_session: bool,
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
                if create_session || (project_id.is_none() && empty) {
                    this.snapshot.create_workspace_in_project(id);
                }
                this.persistence_error = None;
                this.left_sidebar_mode = LeftSidebarMode::Sessions;
                this.set_left_sidebar_visible(true, true, cx);
                this.reconcile_terminal_views(cx);
                this.apply_workspace_selection_change(window, cx);
            });
        })
        .detach();
    }

    pub(super) fn create_project_session(
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
        if self.snapshot.create_workspace_in_project(id).is_some() {
            self.reconcile_terminal_views(cx);
            self.apply_workspace_selection_change(window, cx);
        }
    }

    pub(super) fn select_project(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.select_project(id) {
            self.apply_workspace_selection_change(window, cx);
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
            Some("Se cerrarán sus sesiones y procesos. La carpeta y sus archivos se conservarán en el disco."),
            &["Cancelar", "Quitar proyecto"], cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if confirmation.await.ok() != Some(1) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                if this.snapshot.remove_project(id) {
                    this.reconcile_terminal_views(cx);
                    this.apply_workspace_selection_change(window, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn project_sidebar_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_none()
            .px(px(SIDEBAR_ROW_INSET))
            .py_2()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                sidebar_action("sidebar-search", "chrome-icons/search.svg", "Buscar", true)
                    .tooltip(|_, cx| sidebar_tooltip("Buscar tabs", cx))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_palette(PaletteMode::Tabs, cx);
                    })),
            )
            .child(
                sidebar_action("sidebar-kanban", "chrome-icons/kanban.svg", "Kanban", false)
                    .tooltip(|_, cx| {
                        sidebar_tooltip("Próximamente: issues de GitHub de este proyecto", cx)
                    }),
            )
            .into_any_element()
    }

    pub(super) fn project_sidebar_header(
        &self,
        entry: SidebarEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let SidebarEntry::Project { id, name, .. } = entry else {
            unreachable!()
        };
        let row_width = self.left_sidebar_width() - 2.0 * SIDEBAR_ROW_INSET;
        let label_color = mix(colors().muted, colors().foreground, 0.6);
        // Reserve the folder, project picker, and session button before truncating the name.
        let label_width = row_width
            - SIDEBAR_ROW_PADDING
            - SIDEBAR_ROW_END_PADDING
            - 14.0
            - 2.0 * SIDEBAR_CONTROL_SIZE
            - 3.0 * 6.0;
        div()
            .w(px(row_width))
            .mx(px(SIDEBAR_ROW_INSET))
            .flex_none()
            .pt(px(6.0))
            .pb(px(2.0))
            .border_t_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .id(SharedString::from(format!("project-{id}")))
                    .h(px(24.0))
                    .w_full()
                    .pl(px(SIDEBAR_ROW_PADDING))
                    .pr(px(SIDEBAR_ROW_END_PADDING))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_default()
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
                    .child(
                        svg()
                            .path("chrome-icons/folder.svg")
                            .size(px(14.0))
                            .flex_none()
                            .text_color(label_color),
                    )
                    .child(
                        div()
                            .w(px(label_width))
                            .flex_none()
                            .truncate()
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(label_color)
                            .child(name),
                    )
                    .child(
                        project_icon_button(
                            format!("project-new-session-{id}"),
                            "chrome-icons/plus.svg",
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.create_project_session(id, window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    )
                    .child(
                        project_icon_button("project-picker", "chrome-icons/chevron-down.svg")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_palette(PaletteMode::Projects, cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn cycle_project(
        &mut self,
        offset: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.cycle_project(offset) {
            self.close_context_menu(cx);
            self.apply_workspace_selection_change(window, cx);
        }
    }

    pub(super) fn project_sidebar_navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.snapshot.projects.len();
        let selected = self
            .snapshot
            .projects
            .iter()
            .position(|project| Some(project.id) == self.snapshot.selected_project_id)
            .unwrap_or(0);
        let row_width = self.left_sidebar_width() - 2.0 * SIDEBAR_ROW_INSET;
        // Leave room for both arrow buttons and the same inset as the project row.
        let visible_count = (((row_width - 2.0 * SIDEBAR_ROW_PADDING - 2.0 * SIDEBAR_CONTROL_SIZE)
            / 20.0) as usize)
            .max(1);
        let start = selected
            .saturating_sub(visible_count / 2)
            .min(count.saturating_sub(visible_count));
        let dots = self
            .snapshot
            .projects
            .iter()
            .skip(start)
            .take(visible_count)
            .map(|project| self.project_navigation_dot(project, cx))
            .collect::<Vec<_>>();
        let previous_name = count.checked_sub(1).map(|last| {
            &self.snapshot.projects[if selected == 0 { last } else { selected - 1 }].name
        });
        let next_name = (count > 0).then(|| &self.snapshot.projects[(selected + 1) % count].name);
        let previous_hint = previous_name.map_or_else(
            || "Proyecto anterior".to_owned(),
            |name| format!("{name} · ⌃⇧⌘["),
        );
        let next_hint = next_name.map_or_else(
            || "Proyecto siguiente".to_owned(),
            |name| format!("{name} · ⌃⇧⌘]"),
        );
        div()
            .w(px(row_width))
            .mx(px(SIDEBAR_ROW_INSET))
            .flex_none()
            .py_2()
            .px(px(SIDEBAR_ROW_PADDING))
            .flex()
            .items_center()
            .child(
                project_icon_button("previous-project", "chrome-icons/arrow-left.svg")
                    .when(count < 2, |button| button.opacity(0.3).cursor_default())
                    .tooltip(move |_, cx| sidebar_tooltip(previous_hint.clone(), cx))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.cycle_project(-1, window, cx)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(dots),
            )
            .child(
                project_icon_button("next-project", "chrome-icons/arrow-right.svg")
                    .when(count < 2, |button| button.opacity(0.3).cursor_default())
                    .tooltip(move |_, cx| sidebar_tooltip(next_hint.clone(), cx))
                    .on_click(cx.listener(|this, _, window, cx| this.cycle_project(1, window, cx))),
            )
            .into_any_element()
    }

    fn project_navigation_dot(
        &self,
        project: &ProjectSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = project.id;
        let active = Some(id) == self.snapshot.selected_project_id;
        let detail = format!(
            "{}\n{}\nArrastra una sesión aquí para moverla a este proyecto",
            project.name, project.root_path
        );
        div()
            .id(SharedString::from(format!("project-dot-{id}")))
            .w(px(20.0))
            .h(px(20.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(|style| style.bg(surface_tint(colors().hover, colors().sidebar)))
            .tooltip(move |_, cx| sidebar_tooltip(detail.clone(), cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.close_context_menu(cx);
                this.select_project(id, window, cx);
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
            .on_drag(
                ProjectDrag {
                    project_id: id,
                    name: project.name.clone(),
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            )
            .can_drop(move |value, _, _| {
                value
                    .downcast_ref::<SidebarWorkspaceDrag>()
                    .is_some_and(|drag| drag.project_id != id)
                    || value
                        .downcast_ref::<ProjectDrag>()
                        .is_some_and(|drag| drag.project_id != id)
            })
            .drag_over::<SidebarWorkspaceDrag>(|style, _, _, _| {
                style.bg(surface_tint(colors().selection, colors().sidebar))
            })
            .drag_over::<ProjectDrag>(|style, _, _, _| {
                style.bg(surface_tint(colors().selection, colors().sidebar))
            })
            .on_drop(
                cx.listener(move |this, drag: &SidebarWorkspaceDrag, window, cx| {
                    this.reorder_drag = None;
                    if this
                        .snapshot
                        .move_workspace_to_project(drag.workspace_id, id)
                    {
                        this.apply_workspace_selection_change(window, cx);
                    }
                }),
            )
            .on_drop(cx.listener(move |this, drag: &ProjectDrag, _, cx| {
                let projects = &this.snapshot.projects;
                let source = projects.iter().position(|p| p.id == drag.project_id);
                let target = projects.iter().position(|p| p.id == id);
                if let (Some(source), Some(target)) = (source, target) {
                    let before = if source < target {
                        projects.get(target + 1).map(|p| p.id)
                    } else {
                        Some(id)
                    };
                    if this.snapshot.move_project(drag.project_id, before) {
                        this.persist(cx);
                    }
                }
            }))
            .child(
                div()
                    .size(px(if active { 5.0 } else { 3.0 }))
                    .rounded_full()
                    .bg(if active {
                        colors().foreground
                    } else {
                        colors().subtle
                    }),
            )
            .into_any_element()
    }

    pub(super) fn sidebar_empty_state(
        &self,
        has_project: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .px(px(SIDEBAR_ROW_PADDING))
            .py_3()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(colors().subtle)
                    .child(if has_project {
                        "Sin sesiones"
                    } else {
                        "Sin proyectos"
                    }),
            )
            .child(
                div()
                    .id("sidebar-empty-action")
                    .text_size(px(11.5))
                    .text_color(colors().muted)
                    .cursor_pointer()
                    .hover(|style| style.text_color(colors().foreground))
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_workspace_in_project(window, cx)
                        }),
                    )
                    .child(if has_project {
                        "Nueva sesión…"
                    } else {
                        "Agregar proyecto…"
                    }),
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
            "Crear sesión · ⌘N"
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
                    .child("Organiza tu trabajo en sesiones"),
            )
            .child(
                div()
                    .id("empty-project-new-session")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(colors().elevated)
                    .text_size(px(12.0))
                    .text_color(colors().foreground)
                    .hover(|s| s.bg(colors().hover))
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_workspace_in_project(window, cx)
                        }),
                    )
                    .child(button),
            )
            .into_any_element()
    }
}

fn sidebar_action(
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.0))
        .w_full()
        .flex_none()
        .px(px(SIDEBAR_ROW_PADDING))
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded(px(SIDEBAR_ROW_RADIUS))
        .text_size(px(11.5))
        .line_height(px(16.0))
        .text_color(if enabled {
            colors().muted
        } else {
            colors().subtle
        })
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(surface_tint(colors().hover, colors().sidebar)))
        })
        .child(
            svg()
                .path(icon)
                .size(px(16.0))
                .flex_none()
                .text_color(if enabled {
                    colors().muted
                } else {
                    colors().subtle
                }),
        )
        .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
}

fn project_icon_button(id: impl Into<SharedString>, icon: &'static str) -> Stateful<Div> {
    div()
        .id(id.into())
        .size(px(SIDEBAR_CONTROL_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .text_color(colors().subtle)
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(surface_tint(colors().hover, colors().sidebar))
                .text_color(colors().foreground)
        })
        .child(svg().path(icon).size(px(12.0)).text_color(colors().muted))
}
