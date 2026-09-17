//! Folder selection and project navigation. Existing terminals keep their cwd.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, MouseButton, MouseDownEvent, PathPromptOptions, PromptLevel, SharedString,
    Window, div, prelude::*, px, svg,
};
use uuid::Uuid;

use crate::AddProject;
use crate::domain::workspace::SidebarEntry;
use crate::ui::theme::{colors, surface_tint};

use super::chrome::sidebar_row_width;
use super::{
    ContextMenuKind, LeftSidebarMode, ProjectDrag, SIDEBAR_CONTROL_SIZE, SIDEBAR_ROW_END_PADDING,
    SIDEBAR_ROW_PADDING, SIDEBAR_ROW_RADIUS, SidebarWorkspaceDrag, WorkspaceView, sidebar_tooltip,
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

    pub(super) fn project_sidebar_header(
        &self,
        entry: SidebarEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let SidebarEntry::Project {
            id,
            name,
            collapsed,
            is_selected,
            ..
        } = entry
        else {
            unreachable!()
        };
        let row_width = sidebar_row_width(self.left_sidebar_width());
        let controls_width = 2.0 * SIDEBAR_CONTROL_SIZE + 2.0;
        // Reserve the folder icon, gaps, and controls within the shared row bounds.
        let label_width = (row_width
            - SIDEBAR_ROW_PADDING
            - SIDEBAR_ROW_END_PADDING
            - 16.0
            - 12.0
            - controls_width)
            .max(48.0);
        let drag = ProjectDrag {
            project_id: id,
            name: name.clone(),
        };
        div()
            .id(SharedString::from(format!("project-{id}")))
            .w(px(row_width))
            .mt(px(4.0))
            .mb(px(3.0))
            .h(px(30.0))
            .pl(px(SIDEBAR_ROW_PADDING))
            .pr(px(SIDEBAR_ROW_END_PADDING))
            .rounded(px(SIDEBAR_ROW_RADIUS))
            .flex()
            .items_center()
            .gap(px(6.0))
            .hover(|s| s.bg(surface_tint(colors().hover, colors().sidebar)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.snapshot.toggle_project(id) {
                    this.persist(cx);
                }
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
                    .downcast_ref::<SidebarWorkspaceDrag>()
                    .is_some_and(|drag| drag.project_id != id)
                    || value
                        .downcast_ref::<ProjectDrag>()
                        .is_some_and(|drag| drag.project_id != id)
            })
            .drag_over::<ProjectDrag>(|style, _, _, _| {
                style.border_t_2().border_color(colors().accent)
            })
            .drag_over::<SidebarWorkspaceDrag>(|style, _, _, _| {
                style.bg(surface_tint(colors().selection, colors().sidebar))
            })
            .on_drop(cx.listener(move |this, drag: &ProjectDrag, _, cx| {
                if this.snapshot.move_project(drag.project_id, Some(id)) {
                    this.persist(cx);
                }
            }))
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
            .child(
                svg()
                    .path("chrome-icons/folder.svg")
                    .size(px(16.0))
                    .flex_none()
                    .text_color(colors().muted),
            )
            .child(
                div()
                    .w(px(label_width))
                    .flex_none()
                    .truncate()
                    .text_size(px(11.5))
                    .line_height(px(16.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(if is_selected {
                        colors().foreground
                    } else {
                        colors().muted
                    })
                    .child(name),
            )
            .child(
                div()
                    .w(px(controls_width))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("project-new-session-{id}")))
                            .tooltip(|_, cx| sidebar_tooltip("Nueva sesión en este proyecto", cx))
                            .size(px(SIDEBAR_CONTROL_SIZE))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .text_color(colors().muted)
                            .hover(|s| s.bg(colors().hover).text_color(colors().foreground))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.create_project_session(id, window, cx);
                                cx.stop_propagation();
                            }))
                            .child(svg().path("chrome-icons/plus.svg").size(px(12.0))),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("project-collapse-{id}")))
                            .size(px(SIDEBAR_CONTROL_SIZE))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .hover(|s| s.bg(colors().hover).text_color(colors().foreground))
                            .child(
                                svg()
                                    .path(if collapsed {
                                        "chrome-icons/chevron-right.svg"
                                    } else {
                                        "chrome-icons/chevron-down.svg"
                                    })
                                    .size(px(9.0))
                                    .text_color(colors().subtle),
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
