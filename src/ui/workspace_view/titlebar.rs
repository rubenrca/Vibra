//! Titlebar chrome, utility tabs, and the IDE launcher.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Div, MouseButton, SharedString, Stateful, Window, WindowControlArea, div,
    prelude::*, px, svg,
};

use crate::OpenIde;
use crate::infrastructure::editor::InstalledEditor;
use crate::ui::menu::{menu_heading, menu_hover, menu_panel};
use crate::ui::tab_style::lifted_tab;
use crate::ui::theme::{colors, frame, frame_hover, frame_surface};

use super::titlebar_reveal::TAB_RISE;
use super::{
    RightSidebarMode, TITLEBAR_CHROME_COLLAPSED, TITLEBAR_HEIGHT, TITLEBAR_RIGHT_CHROME_COLLAPSED,
    WorkspaceSection, sidebar_tooltip,
};

/// Room for the traffic lights: 14 px inset, three 14 px buttons, 20 px pitch.
const TRAFFIC_LIGHTS_INSET: f32 = 80.0;
pub(super) const TITLEBAR_BUTTON_SIZE: f32 = 28.0;
pub(super) const TITLEBAR_BUTTON_GAP: f32 = 2.0;
const TITLEBAR_ICON_SIZE: f32 = 17.0;
/// The IDE split button: the remembered IDE's icon, which opens it, and a
/// chevron that lists the installed ones.
const IDE_LABEL_WIDTH: f32 = 34.0;
const IDE_ICON_SIZE: f32 = 16.0;
const IDE_CHEVRON_WIDTH: f32 = 22.0;
const IDE_BUTTON_HEIGHT: f32 = 24.0;
/// Both segments, the divider between them, and the outline on either side.
const IDE_BUTTON_WIDTH: f32 = IDE_LABEL_WIDTH + 1.0 + IDE_CHEVRON_WIDTH + 2.0;
/// Breathing room so the outlined button does not crowd the icon buttons.
const IDE_BUTTON_MARGIN: f32 = 4.0;
/// Titlebar width the IDE button takes beside the right panel toggle.
const IDE_BUTTON_ROOM: f32 = IDE_BUTTON_WIDTH + 2.0 * IDE_BUTTON_MARGIN + TITLEBAR_BUTTON_GAP;

/// Square icon button shared by every titlebar control so they line up.
pub(super) fn titlebar_button(id: &'static str, enabled: bool) -> Stateful<Div> {
    let button = div()
        .id(id)
        .size(px(TITLEBAR_BUTTON_SIZE))
        .flex_none()
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .text_color(colors().muted);
    if !enabled {
        return button.opacity(0.35);
    }
    button
        .group("titlebar-action")
        .cursor_pointer()
        .hover(|button| button.bg(frame_hover()).text_color(colors().foreground))
        .active(|button| button.opacity(0.8))
}

pub(super) fn titlebar_icon(path: &'static str) -> gpui::Svg {
    // GPUI SVGs require their own color; a parent div's color is not inherited.
    svg()
        .path(path)
        .size(px(TITLEBAR_ICON_SIZE))
        .flex_none()
        .text_color(colors().muted)
        .group_hover("titlebar-action", |icon| {
            icon.text_color(colors().foreground)
        })
}

impl super::WorkspaceView {
    /// Width of the titlebar chrome above the left sidebar.
    pub(super) fn titlebar_left_width(&self) -> f32 {
        let progress = self.left_sidebar_progress;
        let expanded = self.left_sidebar_width();
        if progress > 0.99 {
            expanded
        } else {
            TITLEBAR_CHROME_COLLAPSED + (expanded - TITLEBAR_CHROME_COLLAPSED) * progress
        }
    }

    pub(super) fn titlebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.workspace_section == WorkspaceSection::Settings {
            return self.settings_titlebar();
        }
        let right_progress = if self.workspace_section == WorkspaceSection::Workspace {
            self.right_sidebar_progress
        } else {
            0.0
        };
        let right_expanded_width = self.right_sidebar_width();
        let left_chrome_width = self.titlebar_left_width();
        // The IDE button stays beside the panel toggle whether or not the
        // panel is open, so the collapsed chrome makes room for it.
        let show_ide = self.workspace_section == WorkspaceSection::Workspace;
        let right_collapsed_width =
            TITLEBAR_RIGHT_CHROME_COLLAPSED + if show_ide { IDE_BUTTON_ROOM } else { 0.0 };
        let right_chrome_width = if right_progress > 0.99 {
            right_expanded_width
        } else {
            right_collapsed_width + (right_expanded_width - right_collapsed_width) * right_progress
        };
        let right_open = right_progress > 0.5;
        let show_tab_selector = self.workspace_section == WorkspaceSection::Workspace
            && !self.visible_tab_order(cx).is_empty();
        // Without a space the titlebar stays blank rather than naming the app.
        // Repository needs no name: its tabs sit where a space's tabs would.
        let section_label = match self.workspace_section {
            WorkspaceSection::Workspace => self
                .snapshot
                .selected_project()
                .map(|project| project.name.as_str()),
            WorkspaceSection::Inbox => None,
            WorkspaceSection::Settings => Some("Settings"),
        }
        .map(str::to_owned);
        let mut center_chrome = div().h_full().flex_1().min_w(px(0.0)).flex().items_center();
        let tabs_shown = self.tabs_progress;
        if show_tab_selector && tabs_shown >= 0.999 {
            center_chrome = center_chrome.child(self.tab_bar(cx));
        } else if show_tab_selector && tabs_shown > 0.001 {
            // The tabs fade and drift up as the card rises over them.
            center_chrome = center_chrome.child(
                div()
                    .h_full()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .relative()
                    .top(px(-TAB_RISE * (1.0 - tabs_shown)))
                    .opacity(tabs_shown)
                    .child(self.tab_bar(cx)),
            );
        } else if show_tab_selector {
            // Under the card: the strip above it still moves the window.
            center_chrome = center_chrome
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    crate::infrastructure::window::start_drag();
                });
        } else if self.workspace_section == WorkspaceSection::Inbox {
            center_chrome = center_chrome.child(self.repository_tabs(cx));
        } else {
            center_chrome = center_chrome
                .px_4()
                .text_size(px(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    crate::infrastructure::window::start_drag();
                })
                .children(section_label);
        }
        // The titlebar is part of the frame: one tone from the traffic lights
        // to the right panel, with no seams above the sidebars.
        div()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(frame_surface())
            .child(
                div()
                    .w(px(left_chrome_width))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .pl(px(TRAFFIC_LIGHTS_INSET))
                    .pr_2()
                    .gap(px(TITLEBAR_BUTTON_GAP))
                    // Anchored beside the traffic lights so they stay put
                    // while the sidebar animates.
                    .child(
                        self.sidebar_button("toggle-left-sidebar", true, cx, |this, _, cx| {
                            this.set_left_sidebar_visible(
                                !this.settings.left_sidebar_visible,
                                true,
                                cx,
                            );
                        }),
                    )
                    .child(
                        div()
                            .h_full()
                            .flex_1()
                            .window_control_area(WindowControlArea::Drag)
                            .on_mouse_down(MouseButton::Left, |_, _, _| {
                                crate::infrastructure::window::start_drag();
                            }),
                    ),
            )
            .child(center_chrome)
            .child(
                div()
                    .w(px(right_chrome_width))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .pr_2()
                    .gap(px(TITLEBAR_BUTTON_GAP))
                    .overflow_hidden()
                    .when(right_open, |chrome| {
                        chrome.child(
                            div()
                                .h_full()
                                .flex_1()
                                .min_w(px(0.0))
                                .pl_3()
                                .flex()
                                .items_center()
                                .text_size(px(13.0))
                                .text_color(colors().muted)
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .window_control_area(WindowControlArea::Drag)
                                .on_mouse_down(MouseButton::Left, |_, _, _| {
                                    crate::infrastructure::window::start_drag();
                                })
                                .child("Workspace"),
                        )
                    })
                    .when(!right_open, |chrome| chrome.child(div().flex_1()))
                    .when(show_ide, |chrome| chrome.child(self.ide_button(cx)))
                    .child(self.sidebar_button(
                        "toggle-right-sidebar",
                        false,
                        cx,
                        |this, window, cx| {
                            this.toggle_workspace_panel(window, cx);
                        },
                    )),
            )
            .into_any_element()
    }

    /// Settings has no tabs or panels to toggle: the bar is all frame, and
    /// all of it moves the window. The page names itself on its card.
    fn settings_titlebar(&self) -> AnyElement {
        div()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .bg(frame_surface())
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_, _, _| {
                crate::infrastructure::window::start_drag();
            })
            .into_any_element()
    }

    pub(super) fn utility_mode_tabs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.right_sidebar_mode;
        let modes = [
            (RightSidebarMode::Files, "Explorer"),
            (RightSidebarMode::Diff, "Changes"),
        ];

        div()
            .h(px(38.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .px_2()
            .gap(px(4.0))
            .children(modes.into_iter().map(|(item_mode, label)| {
                let selected = item_mode == mode;
                lifted_tab(
                    div().id(SharedString::from(format!("utility-mode-{label}"))),
                    selected,
                    frame(),
                    28.0,
                )
                .min_w(px(0.0))
                .flex_1()
                .justify_center()
                .cursor_pointer()
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_workspace_mode(item_mode, cx);
                    this.focus_selected_terminal(window, cx);
                }))
                .child(label)
            }))
            .into_any_element()
    }

    pub(super) fn sidebar_button(
        &self,
        id: &'static str,
        left: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let open = if left {
            self.settings.left_sidebar_visible
        } else {
            self.workspace_section == WorkspaceSection::Workspace
                && self.settings.right_sidebar_visible
        };
        let icon = if left {
            "chrome-icons/panel-left.svg"
        } else {
            "chrome-icons/panel-right.svg"
        };
        let label = match (left, open) {
            (true, true) => "Hide sidebar · ⌘B",
            (true, false) => "Show sidebar · ⌘B",
            (false, true) => "Hide right panel · ⌥⌘B",
            (false, false) => "Show right panel · ⌥⌘B",
        };
        titlebar_button(id, true)
            .text_color(if open {
                colors().muted
            } else {
                colors().subtle
            })
            .tooltip(move |_, cx| sidebar_tooltip(label, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(titlebar_icon(icon).size(px(18.0)).text_color(if open {
                colors().muted
            } else {
                colors().subtle
            }))
    }

    /// The IDE remembered from the menu, which the label opens directly.
    fn preferred_editor(&self) -> Option<InstalledEditor> {
        self.settings
            .preferred_editor
            .as_deref()
            .and_then(crate::infrastructure::editor::supported_editor)
    }

    /// Caches an IDE's app icon so the button and the menu can show it.
    pub(super) fn load_editor_icon(&mut self, bundle_identifier: &'static str) {
        if self.ide_icons.contains_key(bundle_identifier) {
            return;
        }
        if let Some(png) = crate::infrastructure::editor::editor_icon_png(bundle_identifier) {
            self.ide_icons.insert(
                bundle_identifier,
                Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)),
            );
        }
    }

    /// Split button: the remembered IDE's icon opens it, the chevron picks one.
    pub(super) fn ide_button(&self, cx: &mut Context<Self>) -> Div {
        let menu_open = self.ide_menu_open;
        let preferred = self.preferred_editor();
        let label_tooltip: SharedString = match &preferred {
            Some(editor) => format!("Open in {}", editor.name).into(),
            None => "Open in IDE".into(),
        };
        // Until an IDE is picked, or when its icon cannot be read, say "IDE".
        let label = match preferred
            .and_then(|editor| self.ide_icons.get(editor.bundle_identifier).cloned())
        {
            Some(icon) => gpui::img(icon)
                .size(px(IDE_ICON_SIZE))
                .flex_none()
                .into_any_element(),
            None => "IDE".into_any_element(),
        };
        let hover_bg = frame_hover();
        let segment = |id: &'static str, width: f32| {
            div()
                .id(id)
                .w(px(width))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |segment| segment.bg(hover_bg).text_color(colors().foreground))
                .active(|segment| segment.opacity(0.8))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        div()
            .flex_none()
            .mx(px(IDE_BUTTON_MARGIN))
            .h(px(IDE_BUTTON_HEIGHT))
            .flex()
            .items_center()
            .rounded(px(IDE_BUTTON_HEIGHT / 2.0))
            .border_1()
            .border_color(colors().border_subtle)
            .text_size(px(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(colors().muted)
            .child(
                segment("open-ide", IDE_LABEL_WIDTH)
                    .rounded_l(px(IDE_BUTTON_HEIGHT / 2.0))
                    .pl(px(2.0))
                    .tooltip(move |_, cx| sidebar_tooltip(label_tooltip.clone(), cx))
                    .on_click(
                        cx.listener(|this, _, window, cx| match this.preferred_editor() {
                            Some(editor) => this.open_with_editor(editor, cx),
                            None => this.open_ide(&OpenIde, window, cx),
                        }),
                    )
                    .child(label),
            )
            .child(div().w(px(1.0)).h_full().bg(colors().border_subtle))
            .child(
                segment("choose-ide", IDE_CHEVRON_WIDTH)
                    .group("ide-chevron")
                    .rounded_r(px(IDE_BUTTON_HEIGHT / 2.0))
                    .pr(px(2.0))
                    .when(menu_open, |segment| segment.bg(hover_bg))
                    .tooltip(|_, cx| sidebar_tooltip("Choose IDE · ⇧⌘E", cx))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_ide(&OpenIde, window, cx);
                    }))
                    .child(
                        svg()
                            .path("chrome-icons/chevron-down.svg")
                            .size(px(12.0))
                            .flex_none()
                            .text_color(if menu_open {
                                colors().foreground
                            } else {
                                colors().muted
                            })
                            .group_hover("ide-chevron", |icon| {
                                icon.text_color(colors().foreground)
                            }),
                    ),
            )
    }

    pub(super) fn ide_menu_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.ide_menu_open.then(|| {
            // Hang the menu from the IDE button, right edges aligned.
            let right = 8.0 + TITLEBAR_BUTTON_SIZE + TITLEBAR_BUTTON_GAP + IDE_BUTTON_MARGIN;
            let top = (TITLEBAR_HEIGHT + IDE_BUTTON_HEIGHT) / 2.0 + 4.0;
            let preferred = self.settings.preferred_editor.clone();
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.ide_menu_open = false;
                        cx.notify();
                    }),
                )
                .child(
                    menu_panel()
                        .id("ide-menu")
                        .absolute()
                        .top(px(top))
                        .right(px(right))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(menu_heading("Open folder in"))
                        .when(self.ide_discovering, |menu| {
                            menu.child(
                                div()
                                    .h(px(28.0))
                                    .px(px(8.0))
                                    .flex()
                                    .items_center()
                                    .text_color(colors().muted)
                                    .child("Finding editors…"),
                            )
                        })
                        .children(self.installed_editors.clone().into_iter().enumerate().map(
                            |(index, editor)| {
                                let label = editor.name;
                                let remembered =
                                    preferred.as_deref() == Some(editor.bundle_identifier);
                                let icon = self.ide_icons.get(editor.bundle_identifier).cloned();
                                let icon = match icon {
                                    Some(icon) => gpui::img(icon).size(px(18.0)).into_any_element(),
                                    None => svg()
                                        .path("chrome-icons/open-external.svg")
                                        .size(px(15.0))
                                        .text_color(colors().muted)
                                        .into_any_element(),
                                };
                                div()
                                    .id(SharedString::from(format!("ide-menu-item-{index}")))
                                    .h(px(30.0))
                                    .px(px(8.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .cursor_pointer()
                                    .text_color(colors().foreground)
                                    .hover(|item| item.bg(menu_hover()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remember_editor(&editor, cx);
                                        this.open_with_editor(editor.clone(), cx);
                                    }))
                                    .child(
                                        div()
                                            .size(px(18.0))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(icon),
                                    )
                                    .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
                                    .when(remembered, |item| {
                                        item.child(
                                            svg()
                                                .path("chrome-icons/check.svg")
                                                .size(px(14.0))
                                                .flex_none()
                                                .text_color(colors().accent),
                                        )
                                    })
                            },
                        )),
                )
                .into_any_element()
        })
    }

    pub(super) fn open_ide(&mut self, _: &OpenIde, _: &mut Window, cx: &mut Context<Self>) {
        self.usage.open = false;
        if self.ide_menu_open {
            self.ide_menu_open = false;
            cx.notify();
            return;
        }
        self.ide_menu_open = true;
        self.context_menu = None;
        if self.ide_discovering {
            cx.notify();
            return;
        }
        self.installed_editors.clear();
        self.ide_discovering = true;
        let discovery =
            cx.background_spawn(async { crate::infrastructure::editor::installed_editors() });
        self._open_ide_task = Some(cx.spawn(async move |this, cx| {
            let editors = discovery.await;
            let _ = this.update(cx, |this, cx| {
                this.ide_discovering = false;
                if editors.is_empty() && this.ide_menu_open {
                    this.ide_menu_open = false;
                    this.persistence_error = Some(
                        concat!(
                            "No compatible IDE found. Install Cursor, VS Code, Windsurf, ",
                            "Zed, Xcode, Sublime Text, or VSCodium"
                        )
                        .into(),
                    );
                }
                for editor in &editors {
                    this.load_editor_icon(editor.bundle_identifier);
                }
                this.installed_editors = editors;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The IDE picked from the menu becomes the one the IDE button opens.
    fn remember_editor(&mut self, editor: &InstalledEditor, cx: &mut Context<Self>) {
        if self.settings.preferred_editor.as_deref() == Some(editor.bundle_identifier) {
            return;
        }
        self.settings.preferred_editor = Some(editor.bundle_identifier.to_owned());
        self.persist_settings(cx);
    }

    pub(super) fn open_with_editor(&mut self, editor: InstalledEditor, cx: &mut Context<Self>) {
        self.ide_menu_open = false;
        let path = self.selected_live_cwd(cx);
        let launch = cx.background_spawn(async move {
            crate::infrastructure::editor::open_in_editor(&path, &editor)
        });
        self._open_ide_task = Some(cx.spawn(async move |this, cx| {
            let result = launch.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if this.persistence_error.as_ref().is_some_and(|error| {
                            error.to_string().starts_with("Could not open the IDE:")
                        }) {
                            this.persistence_error = None;
                        }
                    }
                    Err(error) => {
                        this.persistence_error =
                            Some(format!("Could not open the IDE: {error}").into());
                    }
                }
                cx.notify();
            });
        }));
    }
}
