//! Command palette and quick-open.

use gpui::{AnyElement, Context, MouseButton, SharedString, Window, div, prelude::*, px, relative};

use crate::domain::workspace::PaneSplitDirection;
use crate::ui::theme::{MONO_FONT, colors, popover_surface, surface_tint};
use crate::{OpenIde, QuickOpen, ToggleCommandPalette};

use super::{
    PaletteAction, PaletteItem, PaletteMode, RightSidebarMode, WorkspaceSection,
    collect_search_files,
};

impl super::WorkspaceView {
    pub(super) fn open_palette(&mut self, mode: PaletteMode, cx: &mut Context<Self>) {
        self.palette_mode = Some(mode);
        self.palette_request_id = self.palette_request_id.wrapping_add(1);
        self.palette_query.clear();
        self.palette_selected = 0;
        self.settings_open = false;
        self.context_menu = None;
        self.ide_menu_open = false;
        self.rename_prompt = None;
        self.palette_files.clear();
        self.palette_loading = false;
        self.palette_error = None;
        if mode == PaletteMode::Files && self.has_project_context() {
            self.palette_loading = true;
            let request_id = self.palette_request_id;
            let root = self.project_root();
            let port = self.file_port.clone();
            let task = cx.background_spawn(async move {
                let mut files = Vec::new();
                let error = collect_search_files(port.as_ref(), &root, &root, &mut files)
                    .err()
                    .map(|error| format!("No se pudieron buscar archivos: {error}"));
                (files, error)
            });
            self._palette_task = Some(cx.spawn(async move |this, cx| {
                let (files, error) = task.await;
                let _ = this.update(cx, |this, cx| {
                    if request_id != this.palette_request_id {
                        return;
                    }
                    this.palette_files = files;
                    this.palette_loading = false;
                    this.palette_error = error.map(Into::into);
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
            self.palette_request_id = self.palette_request_id.wrapping_add(1);
            self.palette_files.clear();
            cx.notify();
        } else {
            self.open_palette(PaletteMode::Commands, cx);
        }
    }

    pub(super) fn quick_open(&mut self, _: &QuickOpen, _: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(PaletteMode::Files, cx);
    }

    pub(super) fn palette_items(&self) -> Vec<PaletteItem> {
        let Some(mode) = self.palette_mode else {
            return Vec::new();
        };
        let mut items = match mode {
            PaletteMode::Commands => vec![
                PaletteItem {
                    label: "Terminal: Nueva pestaña".into(),
                    detail: "⌘T".into(),
                    action: PaletteAction::NewTerminalTab,
                },
                PaletteItem {
                    label: "Pane: Dividir a la derecha".into(),
                    detail: "⌘D".into(),
                    action: PaletteAction::Split(PaneSplitDirection::Right),
                },
                PaletteItem {
                    label: "Pane: Dividir hacia abajo".into(),
                    detail: "⇧⌘D".into(),
                    action: PaletteAction::Split(PaneSplitDirection::Down),
                },
                PaletteItem {
                    label: "Pane: Dividir a la izquierda".into(),
                    detail: String::new(),
                    action: PaletteAction::Split(PaneSplitDirection::Left),
                },
                PaletteItem {
                    label: "Pane: Dividir hacia arriba".into(),
                    detail: String::new(),
                    action: PaletteAction::Split(PaneSplitDirection::Up),
                },
                PaletteItem {
                    label: "Pane: Igualar tamaños".into(),
                    detail: "⌃⌥E".into(),
                    action: PaletteAction::EqualizePanes,
                },
                PaletteItem {
                    label: "Pane: Agrandar o restaurar".into(),
                    detail: "⇧⌘↵".into(),
                    action: PaletteAction::TogglePaneZoom,
                },
                PaletteItem {
                    label: "Proyecto: Agregar carpeta…".into(),
                    detail: "⇧⌘O".into(),
                    action: PaletteAction::AddProject,
                },
                PaletteItem {
                    label: "Proyecto: Abrir en el IDE".into(),
                    detail: "⇧⌘E".into(),
                    action: PaletteAction::OpenIde,
                },
                PaletteItem {
                    label: "Workspace: Mostrar u ocultar panel".into(),
                    detail: "⌥⌘B".into(),
                    action: PaletteAction::ToggleGit,
                },
                PaletteItem {
                    label: "Workspace: Explorer".into(),
                    detail: String::new(),
                    action: PaletteAction::ShowFiles,
                },
                PaletteItem {
                    label: "Inbox: Abrir".into(),
                    detail: "Actividad de los agentes".into(),
                    action: PaletteAction::ShowSection(WorkspaceSection::Inbox),
                },
                PaletteItem {
                    label: "Notes: Abrir".into(),
                    detail: "Notas por proyecto".into(),
                    action: PaletteAction::ShowSection(WorkspaceSection::Notes),
                },
                PaletteItem {
                    label: "Notes: Nueva nota".into(),
                    detail: String::new(),
                    action: PaletteAction::NewNote,
                },
                PaletteItem {
                    label: "Automations: Abrir".into(),
                    detail: "Comandos guardados y programados".into(),
                    action: PaletteAction::ShowSection(WorkspaceSection::Automations),
                },
                PaletteItem {
                    label: "Automations: Nueva automatización".into(),
                    detail: String::new(),
                    action: PaletteAction::NewAutomation,
                },
                PaletteItem {
                    label: "Settings: Abrir".into(),
                    detail: "⌘,".into(),
                    action: PaletteAction::ShowSettings,
                },
            ],
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
            items.extend(
                self.library
                    .automations
                    .iter()
                    .map(|automation| PaletteItem {
                        label: format!("Automations: Ejecutar {}", automation.name),
                        detail: automation.command.clone(),
                        action: PaletteAction::RunAutomation(automation.id),
                    }),
            );
            items.extend(self.snapshot.projects.iter().map(|project| PaletteItem {
                label: format!("Proyecto: {}", project.name),
                detail: project.root_path.clone(),
                action: PaletteAction::SelectProject(project.id),
            }));
        }
        if mode != PaletteMode::Files {
            let query = self.palette_query.to_lowercase();
            if !query.is_empty() {
                let tokens: Vec<_> = query.split_whitespace().collect();
                items.retain(|item| {
                    let haystack = format!("{} {}", item.label, item.detail).to_lowercase();
                    tokens.iter().all(|token| haystack.contains(token))
                });
            }
            items.truncate(100);
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
            PaletteAction::NewTerminalTab => {
                self.open_terminal_tab_in_project(window, cx);
            }
            PaletteAction::OpenIde => self.open_ide(&OpenIde, window, cx),
            PaletteAction::Split(direction) => self.split_pane(direction, window, cx),
            PaletteAction::EqualizePanes => {
                self.select_section(WorkspaceSection::Workspace, window, cx);
                self.equalize_panes(&crate::EqualizePanes, window, cx);
            }
            PaletteAction::TogglePaneZoom => {
                self.toggle_pane_zoom(&crate::TogglePaneZoom, window, cx);
            }
            PaletteAction::ToggleGit => self.toggle_diff_panel(window, cx),
            PaletteAction::ShowFiles => {
                self.set_workspace_mode(RightSidebarMode::Files, cx);
                self.focus_selected_terminal(window, cx);
            }
            PaletteAction::ShowSection(section) => self.select_section(section, window, cx),
            PaletteAction::NewNote => self.create_note(window, cx),
            PaletteAction::NewAutomation => self.open_automation_form(None, window, cx),
            PaletteAction::RunAutomation(id) => {
                self.run_automation(id, None, true, cx);
            }
            PaletteAction::ShowSettings => {
                self.open_settings(cx);
            }
            PaletteAction::OpenFile(path) => {
                self.select_file_path(path, cx);
                self.set_workspace_mode(RightSidebarMode::Files, cx);
                self.focus_selected_terminal(window, cx);
            }
        }
    }

    pub(super) fn palette_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.palette_mode?;
        let items = self.palette_items();
        let empty = items.is_empty();
        let empty_message = if self.palette_loading {
            "Buscando archivos…".into()
        } else if let Some(error) = &self.palette_error {
            error.to_string()
        } else {
            "No results".into()
        };
        let selected = self.palette_selected.min(items.len().saturating_sub(1));
        let query = self.palette_query.clone();
        let placeholder = match mode {
            PaletteMode::Commands => "Search commands…",
            PaletteMode::Files => "Open file…",
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
                        .w(px(560.0))
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
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(MONO_FONT)
                                        .text_color(colors().subtle)
                                        .child(">"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
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
                                .child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(colors().subtle)
                                        .child("esc"),
                                ),
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
                                    div()
                                        .id(SharedString::from(format!("palette-item-{index}")))
                                        .h(px(38.0))
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
                                                .size(px(18.0))
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .font_family(MONO_FONT)
                                                .text_color(if active {
                                                    colors().muted
                                                } else {
                                                    colors().subtle
                                                })
                                                .child(if active { "›" } else { "·" }),
                                        )
                                        .child(
                                            div()
                                                .min_w(px(0.0))
                                                .flex_1()
                                                .truncate()
                                                .text_size(px(11.0))
                                                .text_color(if active {
                                                    colors().foreground
                                                } else {
                                                    colors().muted
                                                })
                                                .child(item.label),
                                        )
                                        .child(
                                            div()
                                                .max_w(relative(0.4))
                                                .flex_none()
                                                .truncate()
                                                .font_family(MONO_FONT)
                                                .text_size(px(8.5))
                                                .text_color(colors().subtle)
                                                .child(item.detail),
                                        )
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
                                    .child(empty_message),
                            )
                        })
                        .child(
                            div()
                                .h(px(28.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_end()
                                .px_3()
                                .border_t_1()
                                .border_color(colors().border_subtle)
                                .text_xs()
                                .text_color(colors().subtle)
                                .child("↑↓ navigate · ↵ run"),
                        ),
                )
                .into_any_element(),
        )
    }
}
