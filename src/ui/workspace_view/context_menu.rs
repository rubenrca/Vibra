//! Project and pane menus, rename prompts, and their actions.

use std::path::Path;

use gpui::{AnyElement, Context, MouseButton, SharedString, Window, div, prelude::*, px};

use crate::domain::workspace::PaneSplitDirection;
use crate::ui::agent_marks::agent_compact_badge;
use crate::ui::menu::{MenuRow, menu_panel, menu_separator};
use crate::ui::theme::{MONO_FONT, colors, popover_surface, surface_tint};

use super::{
    ContextMenuAction, ContextMenuKind, ContextMenuState, RenamePrompt, RenamePromptKind,
    WorkspaceView,
};

impl WorkspaceView {
    pub(super) fn open_context_menu(
        &mut self,
        kind: ContextMenuKind,
        x: f32,
        y: f32,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(ContextMenuState { kind, x, y });
        self.rename_prompt = None;
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn begin_rename_prompt(&mut self, kind: RenamePromptKind, cx: &mut Context<Self>) {
        let value = match kind {
            RenamePromptKind::Pane { session_id } => self
                .pane_names
                .get(&session_id)
                .cloned()
                .or_else(|| {
                    self.snapshot
                        .terminal_sessions()
                        .find(|session| session.id == session_id)
                        .map(|session| session.title.clone())
                })
                .unwrap_or_default(),
            RenamePromptKind::Project { project_id } => self
                .snapshot
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .map(|project| project.name.clone())
                .unwrap_or_default(),
            RenamePromptKind::NewFile { .. } | RenamePromptKind::NewFolder { .. } => String::new(),
        };
        self.context_menu = None;
        self.rename_prompt = Some(RenamePrompt { kind, value });
        self.close_palette(cx);
        self.settings_open = false;
        cx.notify();
    }

    pub(super) fn confirm_rename_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.rename_prompt.clone() else {
            return;
        };
        let name = prompt.value.trim().to_owned();
        match prompt.kind {
            RenamePromptKind::NewFile { directory } => {
                self.confirm_new_entry(directory, &name, false, cx);
                return;
            }
            RenamePromptKind::NewFolder { directory } => {
                self.confirm_new_entry(directory, &name, true, cx);
                return;
            }
            _ => {}
        }
        if name.is_empty() {
            self.persistence_error = Some("El nombre no puede estar vacío".into());
            cx.notify();
            return;
        }
        if name.chars().count() > crate::domain::workspace::MAX_NAME_CHARS {
            self.persistence_error = Some(
                format!(
                    "El nombre es demasiado largo (máx. {} caracteres)",
                    crate::domain::workspace::MAX_NAME_CHARS
                )
                .into(),
            );
            cx.notify();
            return;
        }
        match prompt.kind {
            RenamePromptKind::Pane { session_id } => {
                // Names are display labels, not CLI addressing aliases.
                if self.snapshot.project_for_session(session_id).is_none() {
                    self.rename_prompt = None;
                    return;
                }
                self.pane_names.insert(session_id, name);
                self.rename_prompt = None;
                self.persistence_error = None;
            }
            RenamePromptKind::Project { project_id } => {
                if self.snapshot.rename_project(project_id, &name) {
                    self.rename_prompt = None;
                    self.persistence_error = None;
                    self.persist(cx);
                }
            }
            RenamePromptKind::NewFile { .. } | RenamePromptKind::NewFolder { .. } => {}
        }
        cx.notify();
    }

    pub(super) fn run_context_menu_action(
        &mut self,
        action: ContextMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.context_menu.clone() else {
            return;
        };
        self.context_menu = None;
        match (menu.kind, action) {
            (ContextMenuKind::SidebarBackground, ContextMenuAction::AddProject) => {
                self.choose_project_folder(None, false, window, cx);
            }
            (ContextMenuKind::Project { project_id }, ContextMenuAction::Rename) => {
                self.begin_rename_prompt(RenamePromptKind::Project { project_id }, cx);
            }
            (ContextMenuKind::Project { project_id }, ContextMenuAction::NewTab) => {
                self.open_project_tab(project_id, window, cx);
            }
            (ContextMenuKind::Project { project_id }, ContextMenuAction::AssociateFolder) => {
                self.choose_project_folder(Some(project_id), false, window, cx);
            }
            (ContextMenuKind::Project { project_id }, ContextMenuAction::RevealProject) => {
                if let Some(path) = self
                    .snapshot
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .and_then(|p| p.directory())
                {
                    cx.reveal_path(Path::new(path));
                }
            }
            (ContextMenuKind::Project { project_id }, ContextMenuAction::ToggleProjectPin) => {
                if self.settings.pinned_project_ids.contains(&project_id) {
                    self.settings
                        .pinned_project_ids
                        .retain(|id| *id != project_id);
                } else {
                    self.settings.pinned_project_ids.push(project_id);
                }
                self.persist_settings(cx);
            }
            (ContextMenuKind::Project { project_id }, ContextMenuAction::RemoveProject) => {
                self.confirm_remove_project(project_id, window, cx);
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::Rename) => {
                self.begin_rename_prompt(RenamePromptKind::Pane { session_id }, cx);
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::ClosePane) => {
                self.close_pane(session_id, window, cx);
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::SplitRight) => {
                if self.snapshot.select_terminal_global(session_id) {
                    self.split_pane(PaneSplitDirection::Right, window, cx);
                }
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::SplitDown) => {
                if self.snapshot.select_terminal_global(session_id) {
                    self.split_pane(PaneSplitDirection::Down, window, cx);
                }
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::ToggleZoom) => {
                self.toggle_pane_zoom_for(session_id, window, cx);
            }
            _ => cx.notify(),
        }
    }

    pub(super) fn context_menu_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.context_menu.clone()?;
        let pane_identity = match &menu.kind {
            ContextMenuKind::Pane { session_id } => self.pane_identity_by_id(*session_id, cx),
            ContextMenuKind::Project { .. } | ContextMenuKind::SidebarBackground => None,
        };
        let row = |label: &'static str, icon: &'static str, action| {
            Some((MenuRow::new(label).icon(icon), action))
        };
        let entries: Vec<Option<(MenuRow, ContextMenuAction)>> = match &menu.kind {
            ContextMenuKind::Project { project_id } => {
                let pinned = self.settings.pinned_project_ids.contains(project_id);
                vec![
                    row(
                        "Nueva pestaña",
                        "chrome-icons/plus.svg",
                        ContextMenuAction::NewTab,
                    )
                    .map(|(item, action)| (item.shortcut("⌘T"), action)),
                    row(
                        "Renombrar proyecto",
                        "chrome-icons/pencil.svg",
                        ContextMenuAction::Rename,
                    ),
                    if pinned {
                        row(
                            "Desfijar proyecto",
                            "chrome-icons/pin-off.svg",
                            ContextMenuAction::ToggleProjectPin,
                        )
                    } else {
                        row(
                            "Fijar proyecto",
                            "chrome-icons/pin.svg",
                            ContextMenuAction::ToggleProjectPin,
                        )
                    },
                    None,
                    row(
                        "Mostrar en Finder",
                        "chrome-icons/folder-open.svg",
                        ContextMenuAction::RevealProject,
                    ),
                    row(
                        "Asociar carpeta…",
                        "chrome-icons/folder.svg",
                        ContextMenuAction::AssociateFolder,
                    ),
                    None,
                    row(
                        "Quitar proyecto…",
                        "chrome-icons/trash.svg",
                        ContextMenuAction::RemoveProject,
                    )
                    .map(|(item, action)| (item.danger(), action)),
                ]
            }
            ContextMenuKind::SidebarBackground => vec![
                row(
                    "Agregar proyecto…",
                    "chrome-icons/folder-plus.svg",
                    ContextMenuAction::AddProject,
                )
                .map(|(item, action)| (item.shortcut("⇧⌘O"), action)),
            ],
            ContextMenuKind::Pane { .. } => vec![
                row(
                    "Renombrar",
                    "chrome-icons/pencil.svg",
                    ContextMenuAction::Rename,
                ),
                None,
                row(
                    "Dividir a la derecha",
                    "chrome-icons/split-view.svg",
                    ContextMenuAction::SplitRight,
                )
                .map(|(item, action)| (item.shortcut("⌘D"), action)),
                row(
                    "Dividir abajo",
                    "chrome-icons/rows.svg",
                    ContextMenuAction::SplitDown,
                )
                .map(|(item, action)| (item.shortcut("⇧⌘D"), action)),
                row(
                    "Agrandar o restaurar",
                    "chrome-icons/maximize.svg",
                    ContextMenuAction::ToggleZoom,
                )
                .map(|(item, action)| (item.shortcut("⇧⌘↩"), action)),
                None,
                row(
                    "Cerrar pane",
                    "chrome-icons/close.svg",
                    ContextMenuAction::ClosePane,
                )
                .map(|(item, action)| (item.danger(), action)),
            ],
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_context_menu(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.close_context_menu(cx);
                    }),
                )
                .child(
                    gpui::anchored()
                        .position(gpui::point(px(menu.x + 2.0), px(menu.y + 2.0)))
                        .snap_to_window_with_margin(px(8.0))
                        .child(
                            menu_panel()
                                .id("context-menu")
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_mouse_down(MouseButton::Right, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .when_some(pane_identity, |menu, identity| {
                                    menu.child(
                                        div()
                                            .max_w(px(320.0))
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(10.0))
                                            .child(agent_compact_badge(
                                                identity.agent_kind.as_deref(),
                                                identity.agent_state,
                                                identity.agent_attention,
                                                true,
                                            ))
                                            .child(
                                                div()
                                                    .min_w(px(0.0))
                                                    .flex_1()
                                                    .flex()
                                                    .flex_col()
                                                    .child(
                                                        div()
                                                            .truncate()
                                                            .text_size(px(12.5))
                                                            .font_weight(gpui::FontWeight::MEDIUM)
                                                            .text_color(colors().foreground)
                                                            .child(identity.title),
                                                    )
                                                    .when_some(
                                                        identity.detail,
                                                        |column, detail| {
                                                            column.child(
                                                                div()
                                                                    .truncate()
                                                                    .font_family(MONO_FONT)
                                                                    .text_size(px(10.5))
                                                                    .text_color(colors().subtle)
                                                                    .child(detail),
                                                            )
                                                        },
                                                    ),
                                            ),
                                    )
                                    .child(menu_separator())
                                })
                                .children(entries.into_iter().enumerate().map(|(index, entry)| {
                                    match entry {
                                        Some((item, action)) => item
                                            .render(SharedString::from(format!(
                                                "context-menu-item-{index}"
                                            )))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.run_context_menu_action(action, window, cx);
                                            }))
                                            .into_any_element(),
                                        None => menu_separator().into_any_element(),
                                    }
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn rename_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prompt = self.rename_prompt.clone()?;
        let title = match prompt.kind {
            RenamePromptKind::Pane { .. } => "Renombrar pane",
            RenamePromptKind::Project { .. } => "Renombrar proyecto",
            RenamePromptKind::NewFile { .. } => "Nuevo archivo",
            RenamePromptKind::NewFolder { .. } => "Nueva carpeta",
        };
        let value = if prompt.value.is_empty() {
            "Escribe un nombre…".to_owned()
        } else {
            prompt.value
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors().overlay())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.rename_prompt = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(420.0))
                        .max_w_full()
                        .mx_4()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(popover_surface())
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors().foreground)
                                .child(title),
                        )
                        .child(
                            div()
                                .h(px(34.0))
                                .px_3()
                                .rounded(px(5.0))
                                .border_1()
                                .border_color(colors().border_subtle)
                                .bg(surface_tint(colors().terminal, colors().sidebar))
                                .flex()
                                .items_center()
                                .font_family(MONO_FONT)
                                .text_size(px(11.0))
                                .text_color(if value == "Escribe un nombre…" {
                                    colors().subtle
                                } else {
                                    colors().foreground
                                })
                                .child(div().min_w(px(0.0)).flex_1().truncate().child(value)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors().subtle)
                                        .child("↵ confirmar · esc cancelar"),
                                )
                                .child(
                                    div()
                                        .id("confirm-rename-prompt")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(5.0))
                                        .border_1()
                                        .border_color(colors().border_subtle)
                                        .cursor_pointer()
                                        .bg(surface_tint(colors().selection, colors().sidebar))
                                        .text_xs()
                                        .text_color(colors().foreground)
                                        .hover(|button| {
                                            button
                                                .bg(surface_tint(colors().hover, colors().sidebar))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_rename_prompt(cx);
                                        }))
                                        .child("Confirmar"),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
