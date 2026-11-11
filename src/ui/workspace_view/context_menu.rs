//! Space and pane menus, rename prompts, and their actions.

use std::path::Path;

use gpui::{
    AnyElement, Context, Corner, MouseButton, SharedString, Window, div, point, prelude::*, px,
};

use crate::domain::workspace::PaneSplitDirection;
use crate::ui::agent_marks::agent_compact_badge;
use crate::ui::dialog::{
    DialogButton, dialog_button, dialog_field, dialog_footer, dialog_header, dialog_panel,
    dialog_scrim, fade_in, rise_in,
};
use crate::ui::menu::{MenuRow, menu_panel, menu_panel_height, menu_separator};
use crate::ui::theme::{MONO_FONT, colors};

use super::{
    ContextMenuAction, ContextMenuAnchor, ContextMenuKind, ContextMenuState, RenamePrompt,
    RenamePromptKind, WorkspaceView,
};

/// Space between the sidebar's edge and a menu opened beside it.
const SIDEBAR_MENU_GAP: f32 = 4.0;
/// Keep menus this far from the window edges.
pub(super) const MENU_EDGE_MARGIN: f32 = 8.0;
/// A pane menu's identity header (agent and title) and its separator.
const PANE_MENU_HEADER_HEIGHT: f32 = 55.0;

/// Where a menu of `height` goes: the point to anchor and which of its corners
/// sits there. Beside the sidebar it hangs from the row's top edge, or rises
/// from the row's bottom edge when it would not fit below.
fn menu_placement(
    anchor: ContextMenuAnchor,
    height: f32,
    window_height: f32,
) -> (gpui::Point<gpui::Pixels>, Corner) {
    match anchor {
        ContextMenuAnchor::Pointer { x, y } => (point(px(x + 2.0), px(y + 2.0)), Corner::TopLeft),
        ContextMenuAnchor::Sidebar { left, top, bottom } => {
            if top + height + MENU_EDGE_MARGIN <= window_height {
                (point(px(left), px(top)), Corner::TopLeft)
            } else {
                (point(px(left), px(bottom)), Corner::BottomLeft)
            }
        }
    }
}

impl WorkspaceView {
    /// Opens a menu with its corner at the pointer.
    pub(super) fn open_context_menu(
        &mut self,
        kind: ContextMenuKind,
        x: f32,
        y: f32,
        cx: &mut Context<Self>,
    ) {
        self.show_context_menu(kind, ContextMenuAnchor::Pointer { x, y }, cx);
    }

    /// Opens a menu beside the sidebar, level with the row spanning `top` to
    /// `bottom` (window coordinates).
    pub(super) fn open_sidebar_context_menu(
        &mut self,
        kind: ContextMenuKind,
        top: f32,
        bottom: f32,
        cx: &mut Context<Self>,
    ) {
        let left = self.sidebar_popover_left();
        self.show_context_menu(kind, ContextMenuAnchor::Sidebar { left, top, bottom }, cx);
    }

    /// Window x where a menu or popover opened beside the left sidebar starts.
    pub(super) fn sidebar_popover_left(&self) -> f32 {
        self.left_sidebar_width() * self.left_sidebar_progress + SIDEBAR_MENU_GAP
    }

    fn show_context_menu(
        &mut self,
        kind: ContextMenuKind,
        anchor: ContextMenuAnchor,
        cx: &mut Context<Self>,
    ) {
        self.usage.open = false;
        self.context_menu = Some(ContextMenuState { kind, anchor });
        self.rename_prompt = None;
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn begin_rename_prompt(&mut self, kind: RenamePromptKind, cx: &mut Context<Self>) {
        self.usage.open = false;
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
                .project(project_id)
                .map(|project| project.name.clone())
                .unwrap_or_default(),
            RenamePromptKind::NewFile { .. } | RenamePromptKind::NewFolder { .. } => String::new(),
        };
        self.context_menu = None;
        self.rename_prompt = Some(RenamePrompt { kind, value });
        self.close_palette(cx);
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
            // An empty name gives the space back its folder's name, which it follows.
            RenamePromptKind::Project { project_id } if name.is_empty() => {
                let folder = self
                    .snapshot
                    .project(project_id)
                    .and_then(|project| project.directory())
                    .map(|root| {
                        super::spaces::space_name(Path::new(root), self.home_directory.as_deref())
                    });
                if let Some(folder) = folder
                    && self.snapshot.name_space_after_folder(project_id, folder)
                {
                    self.persist(cx);
                }
                self.rename_prompt = None;
                self.persistence_error = None;
                cx.notify();
                return;
            }
            _ => {}
        }
        if name.is_empty() {
            self.persistence_error = Some("The name cannot be empty".into());
            cx.notify();
            return;
        }
        if name.chars().count() > crate::domain::workspace::MAX_NAME_CHARS {
            self.persistence_error = Some(
                format!(
                    "The name is too long (max. {} characters)",
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
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        match (menu.kind, action) {
            (ContextMenuKind::SidebarBackground, ContextMenuAction::NewSpace) => {
                self.new_space(window, cx);
            }
            (ContextMenuKind::SidebarBackground, ContextMenuAction::AddProject) => {
                self.open_folder_space(window, cx);
            }
            (ContextMenuKind::Project { project_id }, ContextMenuAction::Rename) => {
                self.begin_rename_prompt(RenamePromptKind::Project { project_id }, cx);
            }
            (ContextMenuKind::Project { project_id }, ContextMenuAction::NewTab) => {
                self.open_project_tab(project_id, window, cx);
            }
            (ContextMenuKind::Project { project_id }, ContextMenuAction::RevealProject) => {
                if let Some(path) = self
                    .snapshot
                    .project(project_id)
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
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::CloseTab) => {
                if let Some(tab_id) = self.tab_of_session(session_id).map(|tab| tab.id) {
                    self.close_tab(tab_id, window, cx);
                }
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
            (ContextMenuKind::Review, ContextMenuAction::CloseReview) => {
                self.close_review(window, cx);
            }
            _ => cx.notify(),
        }
    }

    pub(super) fn context_menu_overlay(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.context_menu.clone()?;
        let pane_identity = match &menu.kind {
            ContextMenuKind::Pane { session_id } => self.pane_identity_by_id(*session_id, cx),
            ContextMenuKind::Project { .. }
            | ContextMenuKind::SidebarBackground
            | ContextMenuKind::Review => None,
        };
        let row = |label: &'static str, icon: &'static str, action| {
            Some((MenuRow::new(label).icon(icon), action))
        };
        let entries: Vec<Option<(MenuRow, ContextMenuAction)>> = match &menu.kind {
            ContextMenuKind::Project { project_id } => {
                let pinned = self.settings.pinned_project_ids.contains(project_id);
                // Only a space with terminals asks before closing.
                let asks = self
                    .snapshot
                    .project(*project_id)
                    .is_some_and(|project| project.terminal_sessions().next().is_some());
                vec![
                    row(
                        "New tab",
                        "chrome-icons/plus.svg",
                        ContextMenuAction::NewTab,
                    )
                    .map(|(item, action)| (item.shortcut("⌘T"), action)),
                    row(
                        "Rename space",
                        "chrome-icons/pencil.svg",
                        ContextMenuAction::Rename,
                    ),
                    if pinned {
                        row(
                            "Unpin space",
                            "chrome-icons/pin-off.svg",
                            ContextMenuAction::ToggleProjectPin,
                        )
                    } else {
                        row(
                            "Pin space",
                            "chrome-icons/pin.svg",
                            ContextMenuAction::ToggleProjectPin,
                        )
                    },
                    None,
                    row(
                        "Reveal in Finder",
                        "chrome-icons/folder-open.svg",
                        ContextMenuAction::RevealProject,
                    ),
                    None,
                    row(
                        if asks {
                            "Close space…"
                        } else {
                            "Close space"
                        },
                        "chrome-icons/trash.svg",
                        ContextMenuAction::RemoveProject,
                    )
                    .map(|(item, action)| (item.danger(), action)),
                ]
            }
            ContextMenuKind::Review => {
                let label = if self.diff_view.read(cx).preview_path().is_some() {
                    "Close file"
                } else {
                    "Close review"
                };
                vec![
                    row(
                        label,
                        "chrome-icons/close.svg",
                        ContextMenuAction::CloseReview,
                    )
                    .map(|(item, action)| (item.shortcut("⌘W"), action)),
                ]
            }
            ContextMenuKind::SidebarBackground => vec![
                row(
                    "New space",
                    "chrome-icons/plus.svg",
                    ContextMenuAction::NewSpace,
                )
                .map(|(item, action)| (item.shortcut("⌘N"), action)),
                row(
                    "Open folder…",
                    "chrome-icons/folder-plus.svg",
                    ContextMenuAction::AddProject,
                )
                .map(|(item, action)| (item.shortcut("⇧⌘O"), action)),
            ],
            ContextMenuKind::Pane { session_id } => {
                let mut entries = vec![
                    row(
                        "Rename",
                        "chrome-icons/pencil.svg",
                        ContextMenuAction::Rename,
                    ),
                    None,
                    row(
                        "Split right",
                        "chrome-icons/split-view.svg",
                        ContextMenuAction::SplitRight,
                    )
                    .map(|(item, action)| (item.shortcut("⌘D"), action)),
                    row(
                        "Split down",
                        "chrome-icons/rows.svg",
                        ContextMenuAction::SplitDown,
                    )
                    .map(|(item, action)| (item.shortcut("⇧⌘D"), action)),
                    row(
                        "Zoom or restore",
                        "chrome-icons/maximize.svg",
                        ContextMenuAction::ToggleZoom,
                    )
                    .map(|(item, action)| (item.shortcut("⇧⌘↩"), action)),
                    None,
                    row(
                        "Close pane",
                        "chrome-icons/close.svg",
                        ContextMenuAction::ClosePane,
                    )
                    .map(|(item, action)| (item.danger(), action)),
                ];
                // The selected tab shows this menu instead of a close button,
                // so a tab of several panes can still close as a whole.
                if self
                    .tab_of_session(*session_id)
                    .is_some_and(|tab| tab.sessions.len() > 1)
                {
                    entries.push(
                        row(
                            "Close tab",
                            "chrome-icons/close.svg",
                            ContextMenuAction::CloseTab,
                        )
                        .map(|(item, action)| (item.danger(), action)),
                    );
                }
                entries
            }
        };
        let rows = entries.iter().flatten().count();
        let separators = entries.len() - rows;
        let height = menu_panel_height(rows, separators)
            + if pane_identity.is_some() {
                PANE_MENU_HEADER_HEIGHT
            } else {
                0.0
            };
        let window_height: f32 = window.bounds().size.height.into();
        let (position, corner) = menu_placement(menu.anchor, height, window_height);
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
                        .anchor(corner)
                        .position(position)
                        .snap_to_window_with_margin(px(MENU_EDGE_MARGIN))
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
        let (icon, title, placeholder, action, detail): (_, _, _, _, Option<SharedString>) =
            match &prompt.kind {
                RenamePromptKind::Pane { .. } => (
                    "chrome-icons/pencil.svg",
                    "Rename pane",
                    "Pane name",
                    "Rename",
                    None,
                ),
                RenamePromptKind::Project { .. } => (
                    "chrome-icons/pencil.svg",
                    "Rename space",
                    "Space name",
                    "Rename",
                    Some("Leave empty to use the folder's name".into()),
                ),
                RenamePromptKind::NewFile { directory } => (
                    "chrome-icons/file-plus.svg",
                    "New file",
                    "File name",
                    "Create",
                    Some(format!("In {}", self.entry_location(directory)).into()),
                ),
                RenamePromptKind::NewFolder { directory } => (
                    "chrome-icons/folder-plus.svg",
                    "New folder",
                    "Folder name",
                    "Create",
                    Some(format!("In {}", self.entry_location(directory)).into()),
                ),
            };
        let mono = matches!(
            prompt.kind,
            RenamePromptKind::NewFile { .. } | RenamePromptKind::NewFolder { .. }
        );
        let panel = dialog_panel()
            .w(px(440.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .p(px(20.0))
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(dialog_header(icon, colors().accent, title, detail))
                    .child(dialog_field(&prompt.value, placeholder, mono)),
            )
            .child(
                dialog_footer()
                    .justify_end()
                    .child(
                        dialog_button(
                            "cancel-rename-prompt",
                            "Cancel",
                            "esc",
                            DialogButton::Secondary,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.rename_prompt = None;
                            cx.notify();
                        })),
                    )
                    .child(
                        dialog_button("confirm-rename-prompt", action, "↵", DialogButton::Primary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_rename_prompt(cx);
                            })),
                    ),
            );
        Some(
            fade_in(
                "rename-scrim",
                dialog_scrim()
                    .items_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.rename_prompt = None;
                            cx.notify();
                        }),
                    )
                    .child(rise_in("rename-panel", panel)),
            )
            .into_any_element(),
        )
    }

    /// Where a new file or folder will land, relative to the space when it can be.
    fn entry_location(&self, directory: &Path) -> String {
        let root = self.project_root();
        match directory.strip_prefix(&root) {
            Ok(relative) if !relative.as_os_str().is_empty() => relative.display().to_string(),
            _ => directory
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| directory.display().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_menus_hang_from_the_row_or_rise_from_its_bottom_edge() {
        let anchor = ContextMenuAnchor::Sidebar {
            left: 224.0,
            top: 180.0,
            bottom: 212.0,
        };
        // Room below: the menu's top edge meets the row's top edge.
        assert_eq!(
            menu_placement(anchor, 200.0, 700.0),
            (point(px(224.0), px(180.0)), Corner::TopLeft)
        );
        // Exactly enough room, edge margin included, still hangs.
        assert_eq!(
            menu_placement(anchor, 200.0, 180.0 + 200.0 + MENU_EDGE_MARGIN).1,
            Corner::TopLeft
        );
        // Not enough room: the menu's bottom edge meets the row's bottom edge.
        assert_eq!(
            menu_placement(anchor, 200.0, 380.0),
            (point(px(224.0), px(212.0)), Corner::BottomLeft)
        );
    }

    #[test]
    fn pointer_menus_keep_their_corner_just_off_the_pointer() {
        assert_eq!(
            menu_placement(
                ContextMenuAnchor::Pointer { x: 10.0, y: 20.0 },
                400.0,
                100.0
            ),
            (point(px(12.0), px(22.0)), Corner::TopLeft)
        );
    }

    #[gpui::test]
    fn sidebar_menus_open_beside_the_sidebar_and_pointer_menus_at_the_pointer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, _, _, window) =
            super::super::tests::open_recording_workspace(cx, "sidebar-menu");
        window
            .update(cx, |view, window, cx| {
                let edge = view.left_sidebar_width() * view.left_sidebar_progress;
                let session_id = view.snapshot.selected_session().unwrap().id;
                for kind in [
                    ContextMenuKind::SidebarBackground,
                    ContextMenuKind::Pane { session_id },
                ] {
                    view.open_sidebar_context_menu(kind.clone(), 300.0, 332.0, cx);
                    match view.context_menu.as_ref().unwrap().anchor {
                        ContextMenuAnchor::Sidebar { left, top, bottom } => {
                            assert_eq!(left, edge + SIDEBAR_MENU_GAP);
                            assert_eq!((top, bottom), (300.0, 332.0));
                        }
                        other => panic!("expected a sidebar anchor, got {other:?}"),
                    }
                    assert!(view.context_menu_overlay(window, cx).is_some());

                    view.open_context_menu(kind, 10.0, 20.0, cx);
                    assert!(matches!(
                        view.context_menu.as_ref().unwrap().anchor,
                        ContextMenuAnchor::Pointer { x: 10.0, y: 20.0 }
                    ));
                    assert!(view.context_menu_overlay(window, cx).is_some());
                }
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
