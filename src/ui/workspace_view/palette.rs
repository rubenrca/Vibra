//! Command palette, project picker, sidebar tab search and quick-open.

use gpui::{AnyElement, Context, MouseButton, SharedString, Window, div, prelude::*, px, svg};

use crate::domain::workspace::{PaneSplitDirection, WorkspaceTitleSource};
use crate::ui::theme::{MONO_FONT, colors, popover_surface, surface_tint};
use crate::{OpenIde, QuickOpen, ToggleCommandPalette};

use super::{
    LeftSidebarMode, PaletteAction, PaletteItem, PaletteMode, RightSidebarMode,
    collect_search_files,
};

impl super::WorkspaceView {
    pub(super) fn open_palette(&mut self, mode: PaletteMode, cx: &mut Context<Self>) {
        self.palette_mode = Some(mode);
        self.palette_query.clear();
        self.palette_selected = 0;
        self.settings_open = false;
        self.context_menu = None;
        self.ide_menu_open = false;
        self.rename_prompt = None;
        self.palette_files.clear();
        if mode == PaletteMode::Files && self.has_project_context() {
            self.palette_request_id = self.palette_request_id.wrapping_add(1);
            let request_id = self.palette_request_id;
            let root = self.project_root();
            let port = self.file_port.clone();
            let task = cx.background_spawn(async move {
                let mut files = Vec::new();
                let _ = collect_search_files(port.as_ref(), &root, &root, &mut files);
                files
            });
            self._palette_task = Some(cx.spawn(async move |this, cx| {
                let files = task.await;
                let _ = this.update(cx, |this, cx| {
                    if request_id != this.palette_request_id {
                        return;
                    }
                    this.palette_files = files;
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    pub(super) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_mode.is_some() {
            self.palette_mode = None;
            self.palette_files.clear();
            cx.notify();
        } else {
            self.open_palette(PaletteMode::Commands, cx);
        }
    }

    pub(super) fn quick_open(&mut self, _: &QuickOpen, _: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(PaletteMode::Files, cx);
    }

    pub(super) fn palette_items(&self, cx: &Context<Self>) -> Vec<PaletteItem> {
        let Some(mode) = self.palette_mode else {
            return Vec::new();
        };
        let mut items = match mode {
            PaletteMode::Commands => vec![
                PaletteItem {
                    label: "Proyecto: Anterior".into(),
                    detail: "⌃⇧⌘[".into(),
                    action: PaletteAction::CycleProject(-1),
                },
                PaletteItem {
                    label: "Proyecto: Siguiente".into(),
                    detail: "⌃⇧⌘]".into(),
                    action: PaletteAction::CycleProject(1),
                },
                PaletteItem {
                    label: "Proyecto: Agregar carpeta…".into(),
                    detail: "⇧⌘O".into(),
                    action: PaletteAction::AddProject,
                },
                PaletteItem {
                    label: "Terminal: New tab".into(),
                    detail: "⌘T".into(),
                    action: PaletteAction::NewTerminalTab,
                },
                PaletteItem {
                    label: "Workspace: Open current folder in IDE".into(),
                    detail: "⇧⌘E".into(),
                    action: PaletteAction::OpenIde,
                },
                PaletteItem {
                    label: "Sesión: Nueva en este proyecto".into(),
                    detail: "⌘N".into(),
                    action: PaletteAction::NewWorkspace,
                },
                PaletteItem {
                    label: "Pane: Split right".into(),
                    detail: "⌘D".into(),
                    action: PaletteAction::Split(PaneSplitDirection::Right),
                },
                PaletteItem {
                    label: "Pane: Split down".into(),
                    detail: "⇧⌘D".into(),
                    action: PaletteAction::Split(PaneSplitDirection::Down),
                },
                PaletteItem {
                    label: "Pane: Split left".into(),
                    detail: String::new(),
                    action: PaletteAction::Split(PaneSplitDirection::Left),
                },
                PaletteItem {
                    label: "Pane: Split up".into(),
                    detail: String::new(),
                    action: PaletteAction::Split(PaneSplitDirection::Up),
                },
                PaletteItem {
                    label: "Pane: Equalize".into(),
                    detail: "⌃⌥E".into(),
                    action: PaletteAction::EqualizePanes,
                },
                PaletteItem {
                    label: "Pane: Toggle zoom".into(),
                    detail: "⇧⌘↵".into(),
                    action: PaletteAction::TogglePaneZoom,
                },
                PaletteItem {
                    label: "Sidebar: Toggle Sessions".into(),
                    detail: "⌘B".into(),
                    action: PaletteAction::ShowSessions,
                },
                PaletteItem {
                    label: "Sidebar: Toggle Files / Git".into(),
                    detail: "⌥⌘B".into(),
                    action: PaletteAction::ToggleGit,
                },
                PaletteItem {
                    label: "Sidebar: Files".into(),
                    detail: String::new(),
                    action: PaletteAction::ShowFiles,
                },
                PaletteItem {
                    label: "Sidebar: Info".into(),
                    detail: String::new(),
                    action: PaletteAction::ShowInfo,
                },
                PaletteItem {
                    label: "Settings: Open".into(),
                    detail: "⌘,".into(),
                    action: PaletteAction::ShowSettings,
                },
            ],
            PaletteMode::Projects => self
                .snapshot
                .projects
                .iter()
                .map(|project| PaletteItem {
                    label: project.name.clone(),
                    detail: project.root_path.clone(),
                    action: PaletteAction::SelectProject(project.id),
                })
                .collect(),
            PaletteMode::Tabs => self
                .snapshot
                .projects
                .iter()
                .flat_map(|project| {
                    project
                        .workspaces
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(|workspace| {
                            // Use the same manual/task/live title shown on the sidebar tab.
                            let label =
                                if workspace.title_source == Some(WorkspaceTitleSource::Manual) {
                                    workspace.name.clone()
                                } else {
                                    workspace
                                        .primary_session()
                                        .map(|session| self.pane_identity(session, 0, cx).title)
                                        .unwrap_or_else(|| workspace.name.clone())
                                };
                            PaletteItem {
                                label,
                                detail: project.name.clone(),
                                action: PaletteAction::SelectWorkspace {
                                    project_id: project.id,
                                    workspace_id: workspace.id,
                                },
                            }
                        })
                })
                .collect(),
            PaletteMode::Files => {
                let root = self.project_root();
                let query = self.palette_query.to_lowercase();
                let tokens: Vec<_> = query.split_whitespace().filter(|t| !t.is_empty()).collect();
                self.palette_files
                    .iter()
                    .filter_map(|path| {
                        let label = path
                            .strip_prefix(&root)
                            .unwrap_or(path)
                            .display()
                            .to_string();
                        if !tokens.is_empty() {
                            let haystack = label.to_lowercase();
                            if !tokens.iter().all(|token| haystack.contains(token)) {
                                return None;
                            }
                        }
                        Some(PaletteItem {
                            label,
                            detail: path
                                .extension()
                                .map(|extension| extension.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            action: PaletteAction::OpenFile(path.clone()),
                        })
                    })
                    .take(100)
                    .collect()
            }
        };
        if mode == PaletteMode::Commands {
            items.extend(self.snapshot.projects.iter().map(|project| PaletteItem {
                label: format!("Proyecto: {}", project.name),
                detail: project.root_path.clone(),
                action: PaletteAction::SelectProject(project.id),
            }));
            items.extend(
                self.snapshot
                    .workspace_entries()
                    .into_iter()
                    .map(|entry| PaletteItem {
                        label: format!("Sesión: {}", entry.workspace_name),
                        detail: entry.project_name,
                        action: PaletteAction::SelectWorkspace {
                            project_id: entry.project_id,
                            workspace_id: entry.workspace_id,
                        },
                    }),
            );
        }
        if matches!(
            mode,
            PaletteMode::Commands | PaletteMode::Tabs | PaletteMode::Projects
        ) {
            let query = self.palette_query.to_lowercase();
            if !query.is_empty() {
                let tokens: Vec<_> = query.split_whitespace().collect();
                items.retain(|item| {
                    let haystack = format!("{} {}", item.label, item.detail).to_lowercase();
                    tokens.iter().all(|token| haystack.contains(token))
                });
            }
            if mode != PaletteMode::Projects {
                items.truncate(100);
            }
        }
        items
    }

    pub(super) fn execute_palette_action(
        &mut self,
        action: PaletteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette_mode = None;
        match action {
            PaletteAction::AddProject => self.choose_project_folder(None, false, window, cx),
            PaletteAction::SelectProject(id) => self.select_project(id, window, cx),
            PaletteAction::CycleProject(offset) => self.cycle_project(offset, window, cx),
            PaletteAction::NewTerminalTab => {
                self.open_terminal_tab_in_project(window, cx);
            }
            PaletteAction::OpenIde => self.open_ide(&OpenIde, window, cx),
            PaletteAction::NewWorkspace => {
                self.open_workspace_in_project(window, cx);
            }
            PaletteAction::Split(direction) => self.split_pane(direction, window, cx),
            PaletteAction::EqualizePanes => {
                if self.snapshot.equalize_selected_panes() {
                    self.persist(cx);
                }
            }
            PaletteAction::TogglePaneZoom => {
                if self.snapshot.toggle_selected_pane_zoom() {
                    self.sync_terminal_surface_visibility(cx);
                    self.persist(cx);
                }
            }
            PaletteAction::ToggleGit => self.toggle_diff_panel(cx),
            PaletteAction::ShowSessions => {
                if self.left_sidebar_visible && self.left_sidebar_mode == LeftSidebarMode::Sessions
                {
                    self.set_left_sidebar_visible(false, true, cx);
                } else {
                    self.left_sidebar_mode = LeftSidebarMode::Sessions;
                    self.set_left_sidebar_visible(true, true, cx);
                }
            }
            PaletteAction::ShowFiles => {
                self.right_sidebar_mode = RightSidebarMode::Files;
                self.refresh_project_files(cx);
                self.set_right_sidebar_visible(true, true, cx);
            }
            PaletteAction::ShowInfo => {
                self.left_sidebar_mode = LeftSidebarMode::Info;
                self.set_left_sidebar_visible(true, true, cx);
            }
            PaletteAction::ShowSettings => {
                self.open_settings(cx);
            }
            PaletteAction::SelectWorkspace {
                project_id,
                workspace_id,
            } => self.select_workspace(project_id, workspace_id, window, cx),
            PaletteAction::OpenFile(path) => {
                self.select_file_path(path, cx);
                self.right_sidebar_mode = RightSidebarMode::Files;
                self.set_right_sidebar_visible(true, true, cx);
            }
        }
    }

    pub(super) fn palette_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.palette_mode?;
        let items = self.palette_items(cx);
        let empty = items.is_empty();
        let selected = self.palette_selected.min(items.len().saturating_sub(1));
        let query = self.palette_query.clone();
        let placeholder = match mode {
            PaletteMode::Commands => "Buscar proyectos, sesiones y comandos…",
            PaletteMode::Files => "Open file…",
            PaletteMode::Tabs => "Buscar tabs…",
            PaletteMode::Projects => "Buscar proyectos…",
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(86.0))
                .bg(colors().overlay())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.palette_mode = None;
                        this.palette_files.clear();
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(if mode == PaletteMode::Projects {
                            460.0
                        } else {
                            560.0
                        }))
                        .max_w_full()
                        .max_h(px(460.0))
                        .mx_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(popover_surface())
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h(px(48.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_4()
                                .border_b_1()
                                .border_color(colors().border_subtle)
                                .child(if mode == PaletteMode::Projects {
                                    svg()
                                        .path("chrome-icons/folder.svg")
                                        .size(px(16.0))
                                        .flex_none()
                                        .text_color(colors().muted)
                                        .into_any_element()
                                } else {
                                    div()
                                        .font_family(MONO_FONT)
                                        .text_color(colors().subtle)
                                        .child(">")
                                        .into_any_element()
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .font_family(MONO_FONT)
                                        .text_size(px(12.0))
                                        .text_color(if query.is_empty() {
                                            colors().subtle
                                        } else {
                                            colors().foreground
                                        })
                                        .child(if query.is_empty() {
                                            placeholder.to_owned()
                                        } else {
                                            query
                                        }),
                                )
                                .child(div().text_xs().text_color(colors().subtle).child("esc")),
                        )
                        .child(
                            div()
                                .id("palette-results")
                                .flex_1()
                                .min_h(px(0.0))
                                .overflow_y_scroll()
                                .py_2()
                                .children(items.into_iter().enumerate().map(|(index, item)| {
                                    let active = index == selected;
                                    let action = item.action.clone();
                                    let current_project = matches!(
                                        &item.action,
                                        PaletteAction::SelectProject(id)
                                            if Some(*id) == self.snapshot.selected_project_id
                                    );
                                    div()
                                        .id(SharedString::from(format!("palette-item-{index}")))
                                        .h(px(if mode == PaletteMode::Projects {
                                            48.0
                                        } else {
                                            38.0
                                        }))
                                        .mx_2()
                                        .px_3()
                                        .rounded(px(6.0))
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .cursor_pointer()
                                        .when(active, |row| {
                                            row.bg(surface_tint(
                                                colors().selection,
                                                colors().sidebar,
                                            ))
                                        })
                                        .hover(|row| {
                                            row.bg(surface_tint(colors().hover, colors().sidebar))
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.execute_palette_action(action.clone(), window, cx);
                                        }))
                                        .child(
                                            div()
                                                .w(px(18.0))
                                                .text_center()
                                                .font_family(MONO_FONT)
                                                .text_color(if active {
                                                    colors().muted
                                                } else {
                                                    colors().subtle
                                                })
                                                .child(if mode == PaletteMode::Projects {
                                                    if current_project { "✓" } else { "" }
                                                } else if active {
                                                    "›"
                                                } else {
                                                    "·"
                                                }),
                                        )
                                        .child(
                                            div()
                                                .min_w(px(0.0))
                                                .flex_1()
                                                .flex()
                                                .flex_col()
                                                .gap(px(2.0))
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(11.0))
                                                        .text_color(if active {
                                                            colors().foreground
                                                        } else {
                                                            colors().muted
                                                        })
                                                        .child(item.label),
                                                )
                                                .when(mode == PaletteMode::Projects, |label| {
                                                    label.child(
                                                        div()
                                                            .truncate()
                                                            .font_family(MONO_FONT)
                                                            .text_size(px(9.0))
                                                            .text_color(colors().subtle)
                                                            .child(item.detail.clone()),
                                                    )
                                                }),
                                        )
                                        .when(mode != PaletteMode::Projects, |row| {
                                            row.child(
                                                div()
                                                    .font_family(MONO_FONT)
                                                    .text_size(px(8.5))
                                                    .text_color(colors().subtle)
                                                    .child(item.detail),
                                            )
                                        })
                                })),
                        )
                        .when(empty, |palette| {
                            palette.child(
                                div()
                                    .h(px(80.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(colors().subtle)
                                    .child(match mode {
                                        PaletteMode::Tabs => "No se encontraron tabs",
                                        PaletteMode::Projects => "No se encontraron proyectos",
                                        _ => "No results",
                                    }),
                            )
                        })
                        .child(
                            div()
                                .h(px(if mode == PaletteMode::Projects {
                                    44.0
                                } else {
                                    28.0
                                }))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_3()
                                .px_3()
                                .border_t_1()
                                .border_color(colors().border_subtle)
                                .text_xs()
                                .text_color(colors().subtle)
                                .when(mode == PaletteMode::Projects, |footer| {
                                    footer.justify_between().child(
                                        div()
                                            .id("project-picker-new-project")
                                            .h(px(28.0))
                                            .flex_none()
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .rounded(px(5.0))
                                            .border_1()
                                            .border_color(colors().border_subtle)
                                            .bg(surface_tint(colors().selection, colors().sidebar))
                                            .text_size(px(11.0))
                                            .text_color(colors().foreground)
                                            .cursor_pointer()
                                            .hover(|button| {
                                                button.bg(surface_tint(
                                                    colors().hover,
                                                    colors().sidebar,
                                                ))
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.execute_palette_action(
                                                    PaletteAction::AddProject,
                                                    window,
                                                    cx,
                                                );
                                                cx.stop_propagation();
                                                cx.notify();
                                            }))
                                            .child(
                                                svg()
                                                    .path("chrome-icons/plus.svg")
                                                    .size(px(12.0))
                                                    .flex_none()
                                                    .text_color(colors().foreground),
                                            )
                                            .child("Nuevo proyecto"),
                                    )
                                })
                                .child(match mode {
                                    PaletteMode::Tabs => "↑↓ navegar · ↵ abrir tab",
                                    PaletteMode::Projects => "↑↓ navegar · ↵ cambiar proyecto",
                                    _ => "↑↓ navigate · ↵ run",
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}
