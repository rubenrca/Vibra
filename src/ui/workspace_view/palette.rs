//! Command palette and quick-open.

use gpui::{
    AnyElement, Context, MouseButton, SharedString, Window, div, prelude::*, px, relative, svg,
};

use crate::ui::dialog::{
    CONTROL_RADIUS, caret, dialog_footer, dialog_panel, dialog_scrim, fade_in, key_hint, keycap,
    rise_in, shortcut_keys,
};

use crate::domain::workspace::PaneSplitDirection;
use crate::ui::menu::{menu_border, menu_hover};
use crate::ui::theme::{colors, surface_tint};
use crate::{OpenIde, QuickOpen, ToggleCommandPalette};

use super::{PaletteAction, PaletteItem, PaletteMode, RightSidebarMode, WorkspaceSection};

impl super::WorkspaceView {
    pub(super) fn open_palette(&mut self, mode: PaletteMode, cx: &mut Context<Self>) {
        self.usage.open = false;
        self.close_palette(cx);
        self.palette_mode = Some(mode);
        self.context_menu = None;
        self.ide_menu_open = false;
        self.rename_prompt = None;
        if mode == PaletteMode::Files && self.has_project_context() {
            self.palette_loading = true;
            let request_id = self.palette_request_id;
            let root = self.project_root();
            let port = self.file_port.clone();
            let task = cx.background_spawn(async move {
                let mut files = Vec::new();
                let error = port
                    .search_files(&root, &mut files)
                    .err()
                    .map(|error| format!("Could not search files: {error}"));
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

    /// Every dismissal cancels its file search, including Escape and clicks
    /// outside the modal. Late results cannot repopulate a closed palette.
    pub(super) fn close_palette(&mut self, cx: &mut Context<Self>) {
        let was_open = self.palette_mode.take().is_some();
        self.palette_request_id = self.palette_request_id.wrapping_add(1);
        self._palette_task = None;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.palette_files.clear();
        self.palette_loading = false;
        self.palette_error = None;
        if was_open {
            cx.notify();
        }
    }

    pub(super) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_mode.is_some() {
            self.close_palette(cx);
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
        let mut items: Vec<_> = match mode {
            PaletteMode::Commands => [
                ("Terminal: New tab", "⌘T", PaletteAction::NewTerminalTab),
                (
                    "Pane: Split right",
                    "⌘D",
                    PaletteAction::Split(PaneSplitDirection::Right),
                ),
                (
                    "Pane: Split down",
                    "⇧⌘D",
                    PaletteAction::Split(PaneSplitDirection::Down),
                ),
                (
                    "Pane: Split left",
                    "",
                    PaletteAction::Split(PaneSplitDirection::Left),
                ),
                (
                    "Pane: Split up",
                    "",
                    PaletteAction::Split(PaneSplitDirection::Up),
                ),
                ("Pane: Equalize sizes", "⌃⌥E", PaletteAction::EqualizePanes),
                (
                    "Pane: Zoom or restore",
                    "⇧⌘↵",
                    PaletteAction::TogglePaneZoom,
                ),
                ("Space: New", "⌘N", PaletteAction::NewSpace),
                ("Space: Open folder…", "⇧⌘O", PaletteAction::AddProject),
                ("Space: Open in IDE", "⇧⌘E", PaletteAction::OpenIde),
                (
                    "Workspace: Toggle panel",
                    "⌥⌘B",
                    PaletteAction::ToggleWorkspacePanel,
                ),
                ("Workspace: Explorer", "", PaletteAction::ShowFiles),
                (
                    "Repository: Open",
                    "",
                    PaletteAction::ShowSection(WorkspaceSection::Inbox),
                ),
                ("Settings: Open", "⌘,", PaletteAction::ShowSettings),
            ]
            .into_iter()
            .map(|(label, shortcut, action)| PaletteItem {
                label: label.into(),
                detail: match action {
                    PaletteAction::ShowSection(WorkspaceSection::Inbox) => {
                        "History, pull requests, issues, and agent activity".into()
                    }
                    _ => String::new(),
                },
                shortcut,
                action,
            })
            .collect(),
            PaletteMode::Files => {
                let root = self.project_root();
                let query = self.palette_query.to_lowercase();
                let tokens: Vec<_> = query.split_whitespace().collect();
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
                            detail: String::new(),
                            shortcut: "",
                            action: PaletteAction::OpenFile(path.clone()),
                        })
                    })
                    .take(100)
                    .collect()
            }
        };
        if mode == PaletteMode::Commands {
            items.extend(self.snapshot.projects.iter().map(|project| PaletteItem {
                label: format!("Space: {}", project.name),
                detail: project.root_path.clone(),
                shortcut: "",
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
        self.close_palette(cx);
        match action {
            PaletteAction::NewSpace => self.new_space(window, cx),
            PaletteAction::AddProject => self.open_folder_space(window, cx),
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
            PaletteAction::ToggleWorkspacePanel => self.toggle_workspace_panel(window, cx),
            PaletteAction::ShowFiles => {
                self.set_workspace_mode(RightSidebarMode::Files, cx);
                self.focus_selected_terminal(window, cx);
            }
            PaletteAction::ShowSection(section) => self.select_section(section, window, cx),
            PaletteAction::ShowSettings => {
                self.open_settings(window, cx);
            }
            PaletteAction::OpenFile(path) => {
                self.set_workspace_mode(RightSidebarMode::Files, cx);
                self.open_project_file(path, window, cx);
            }
        }
    }

    pub(super) fn palette_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.palette_mode?;
        let items = self.palette_items();
        let empty = items.is_empty();
        let empty_message = if self.palette_loading {
            "Searching files…".into()
        } else if let Some(error) = &self.palette_error {
            error.to_string()
        } else {
            "No results".into()
        };
        let selected = self.palette_selected.min(items.len().saturating_sub(1));
        let query = self.palette_query.clone();
        let (placeholder, run_label) = match mode {
            PaletteMode::Commands => ("Search commands…", "Run"),
            PaletteMode::Files => ("Open file…", "Open"),
        };
        let search = div()
            .h(px(54.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(18.0))
            .border_b_1()
            .border_color(menu_border())
            .child(
                svg()
                    .path("chrome-icons/search.svg")
                    .size(px(17.0))
                    .flex_none()
                    .text_color(colors().muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .text_size(px(15.0))
                    .when(!query.is_empty(), |field| {
                        field.child(
                            div()
                                .min_w(px(0.0))
                                .truncate()
                                .mr(px(1.0))
                                .text_color(colors().foreground)
                                .child(query.clone()),
                        )
                    })
                    .child(caret(18.0))
                    .when(query.is_empty(), |field| {
                        field.child(
                            div()
                                .min_w(px(0.0))
                                .truncate()
                                .ml(px(1.0))
                                .text_color(colors().subtle)
                                .child(placeholder),
                        )
                    }),
            )
            .child(keycap("esc"));
        let results = div()
            .id("palette-results")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p(px(6.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .children(items.into_iter().enumerate().map(|(index, item)| {
                let active = index == selected;
                let icon = palette_icon(&item.action);
                let action = item.action.clone();
                div()
                    .id(SharedString::from(format!("palette-item-{index}")))
                    .h(px(38.0))
                    .flex_none()
                    .px(px(10.0))
                    .rounded(px(CONTROL_RADIUS))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .cursor_pointer()
                    .when(active, |row| {
                        row.bg(surface_tint(colors().selection, colors().elevated))
                    })
                    .when(!active, |row| row.hover(|row| row.bg(menu_hover())))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.execute_palette_action(action.clone(), window, cx);
                    }))
                    .child(
                        svg()
                            .path(icon)
                            .size(px(15.0))
                            .flex_none()
                            .text_color(if active {
                                colors().foreground
                            } else {
                                colors().muted
                            }),
                    )
                    .child(palette_label(mode, &item.label))
                    .when(!item.detail.is_empty(), |row| {
                        row.child(
                            div()
                                .max_w(relative(0.42))
                                .flex_none()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(colors().subtle)
                                .child(item.detail),
                        )
                    })
                    .when(!item.shortcut.is_empty(), |row| {
                        row.child(shortcut_keys(item.shortcut))
                    })
            }));
        let panel = dialog_panel()
            .w(px(620.0))
            .max_h(px(480.0))
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(search)
            .when(!empty, |panel| panel.child(results))
            .when(empty, |panel| {
                panel.child(
                    div()
                        .h(px(120.0))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(10.0))
                        .child(
                            svg()
                                .path("chrome-icons/search.svg")
                                .size(px(20.0))
                                .text_color(colors().subtle),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(colors().muted)
                                .child(empty_message),
                        ),
                )
            })
            .child(
                dialog_footer()
                    .py(px(9.0))
                    .justify_end()
                    .gap(px(16.0))
                    .child(key_hint(&["↑", "↓"], "Navigate"))
                    .child(key_hint(&["↵"], run_label)),
            );
        Some(
            fade_in(
                "palette-scrim",
                dialog_scrim()
                    .items_start()
                    .pt(px(86.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_palette(cx);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(rise_in("palette-panel", panel)),
            )
            .into_any_element(),
        )
    }
}

fn palette_icon(action: &PaletteAction) -> &'static str {
    match action {
        PaletteAction::NewSpace => "chrome-icons/plus.svg",
        PaletteAction::AddProject => "chrome-icons/folder-open.svg",
        PaletteAction::SelectProject(_) => "chrome-icons/folder.svg",
        PaletteAction::NewTerminalTab => "chrome-icons/terminal.svg",
        PaletteAction::OpenIde => "chrome-icons/open-external.svg",
        PaletteAction::Split(_) => "chrome-icons/split-view.svg",
        PaletteAction::EqualizePanes => "chrome-icons/rows.svg",
        PaletteAction::TogglePaneZoom => "chrome-icons/maximize.svg",
        PaletteAction::ToggleWorkspacePanel => "chrome-icons/panel-right.svg",
        PaletteAction::ShowFiles => "chrome-icons/files.svg",
        PaletteAction::ShowSection(_) => "chrome-icons/repository.svg",
        PaletteAction::ShowSettings => "chrome-icons/settings.svg",
        PaletteAction::OpenFile(_) => "file-icons/file.svg",
    }
}

/// Commands lead with a muted category (`Pane:`) before what they do; files
/// lead with their name and trail the folder that holds them.
fn palette_label(mode: PaletteMode, label: &str) -> gpui::Div {
    let (lead, lead_color, rest, rest_color) = match mode {
        PaletteMode::Commands => match label.split_once(": ") {
            Some((category, title)) => (
                format!("{category}:"),
                colors().subtle,
                title.to_owned(),
                colors().foreground,
            ),
            None => (
                label.to_owned(),
                colors().foreground,
                String::new(),
                colors().subtle,
            ),
        },
        PaletteMode::Files => match label.rsplit_once('/') {
            Some((folder, name)) => (
                name.to_owned(),
                colors().foreground,
                folder.to_owned(),
                colors().subtle,
            ),
            None => (
                label.to_owned(),
                colors().foreground,
                String::new(),
                colors().subtle,
            ),
        },
    };
    let files = mode == PaletteMode::Files;
    div()
        .min_w(px(0.0))
        .flex_1()
        .flex()
        .items_center()
        .gap(px(if files { 8.0 } else { 5.0 }))
        .overflow_hidden()
        .text_size(px(13.0))
        .child(div().flex_none().text_color(lead_color).child(lead))
        .when(!rest.is_empty(), |label| {
            label.child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .when(files, |folder| folder.text_size(px(12.0)))
                    .text_color(rest_color)
                    .child(rest),
            )
        })
}
