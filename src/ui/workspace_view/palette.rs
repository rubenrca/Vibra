//! Command palette and quick-open.

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px};

use crate::domain::workspace::PaneSplitDirection;
use crate::ui::theme::colors;
use crate::{OpenIde, QuickOpen, ToggleCommandPalette, ToggleDevTerminal};

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
        self.rename_prompt = None;
        self.palette_files.clear();
        if mode == PaletteMode::Files {
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
                    label: "Terminal: Terminal de desarrollo".into(),
                    detail: "⌘J".into(),
                    action: PaletteAction::ToggleDevTerminal,
                },
                PaletteItem {
                    label: "Workspace: Abrir carpeta actual en el IDE".into(),
                    detail: "⇧⌘E".into(),
                    action: PaletteAction::OpenIde,
                },
                PaletteItem {
                    label: "Workspace: Nuevo".into(),
                    detail: "⌘N".into(),
                    action: PaletteAction::NewWorkspace,
                },
                PaletteItem {
                    label: "Panel: Dividir a la derecha".into(),
                    detail: "⌘D".into(),
                    action: PaletteAction::Split(PaneSplitDirection::Right),
                },
                PaletteItem {
                    label: "Panel: Dividir abajo".into(),
                    detail: "⇧⌘D".into(),
                    action: PaletteAction::Split(PaneSplitDirection::Down),
                },
                PaletteItem {
                    label: "Panel: Dividir a la izquierda".into(),
                    detail: String::new(),
                    action: PaletteAction::Split(PaneSplitDirection::Left),
                },
                PaletteItem {
                    label: "Panel: Dividir arriba".into(),
                    detail: String::new(),
                    action: PaletteAction::Split(PaneSplitDirection::Up),
                },
                PaletteItem {
                    label: "Panel: Igualar".into(),
                    detail: "⌃⌥E".into(),
                    action: PaletteAction::EqualizePanes,
                },
                PaletteItem {
                    label: "Panel: Alternar zoom".into(),
                    detail: "⇧⌘↵".into(),
                    action: PaletteAction::TogglePaneZoom,
                },
                PaletteItem {
                    label: "Barra: Alternar sesiones".into(),
                    detail: "⌘B".into(),
                    action: PaletteAction::ShowSessions,
                },
                PaletteItem {
                    label: "Barra: Alternar Archivos / Git".into(),
                    detail: "⌥⌘B".into(),
                    action: PaletteAction::ToggleGit,
                },
                PaletteItem {
                    label: "Barra: Archivos".into(),
                    detail: String::new(),
                    action: PaletteAction::ShowFiles,
                },
                PaletteItem {
                    label: "Barra: Info".into(),
                    detail: String::new(),
                    action: PaletteAction::ShowInfo,
                },
                PaletteItem {
                    label: "Ajustes: Abrir".into(),
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
                self.snapshot
                    .workspace_entries()
                    .into_iter()
                    .map(|entry| PaletteItem {
                        label: format!("Workspace: {}", entry.workspace_name),
                        detail: entry.project_name,
                        action: PaletteAction::SelectWorkspace {
                            project_id: entry.project_id,
                            workspace_id: entry.workspace_id,
                        },
                    }),
            );
        }
        if mode == PaletteMode::Commands {
            let query = self.palette_query.to_lowercase();
            if !query.is_empty() {
                let tokens: Vec<_> = query.split_whitespace().collect();
                items.retain(|item| {
                    let haystack = item.label.to_lowercase();
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
            PaletteAction::NewTerminalTab => {
                self.open_terminal_tab_in_current_directory(window, cx);
            }
            PaletteAction::ToggleDevTerminal => {
                self.toggle_dev_terminal(&ToggleDevTerminal, window, cx);
            }
            PaletteAction::OpenIde => self.open_ide(&OpenIde, window, cx),
            PaletteAction::NewWorkspace => {
                self.open_workspace_in_current_directory(window, cx);
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
        let items = self.palette_items();
        let selected = self.palette_selected.min(items.len().saturating_sub(1));
        let query = self.palette_query.clone();
        let placeholder = match mode {
            PaletteMode::Commands => "Buscar comandos…",
            PaletteMode::Files => "Abrir archivo…",
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
                .child(
                    div()
                        .w(px(560.0))
                        .max_w_full()
                        .max_h(px(460.0))
                        .mx_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(colors().elevated)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
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
                                        .font_family("JetBrains Mono")
                                        .text_color(colors().subtle)
                                        .child(">"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .font_family("JetBrains Mono")
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
                                        .bg(if active {
                                            colors().selection
                                        } else {
                                            colors().elevated
                                        })
                                        .hover(|row| row.bg(colors().hover))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.execute_palette_action(action.clone(), window, cx);
                                        }))
                                        .child(
                                            div()
                                                .w(px(18.0))
                                                .text_center()
                                                .font_family("JetBrains Mono")
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
                                                .font_family("JetBrains Mono")
                                                .text_size(px(8.5))
                                                .text_color(colors().subtle)
                                                .child(item.detail),
                                        )
                                })),
                        )
                        .when(self.palette_items().is_empty(), |palette| {
                            palette.child(
                                div()
                                    .h(px(80.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(colors().subtle)
                                    .child("Sin resultados"),
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
                                .child("↑↓ navegar · ↵ ejecutar"),
                        ),
                )
                .into_any_element(),
        )
    }
}
