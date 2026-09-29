//! Titlebar chrome, utility tabs, and the IDE launcher.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Div, MouseButton, SharedString, Stateful, Window, WindowControlArea, div,
    prelude::*, px, svg,
};

use crate::OpenIde;
use crate::infrastructure::editor::InstalledEditor;
use crate::ui::menu::{menu_heading, menu_hover, menu_panel};
use crate::ui::theme::{colors, surface, surface_tint};

use super::{
    PaletteMode, RightSidebarMode, TITLEBAR_CHROME_COLLAPSED, TITLEBAR_HEIGHT,
    TITLEBAR_RIGHT_CHROME_COLLAPSED, WorkspaceSection, sidebar_tooltip,
};

/// Room for the traffic lights: 14 px inset, three 14 px buttons, 20 px pitch.
const TRAFFIC_LIGHTS_INSET: f32 = 80.0;
pub(super) const TITLEBAR_BUTTON_SIZE: f32 = 28.0;
pub(super) const TITLEBAR_BUTTON_GAP: f32 = 2.0;
const TITLEBAR_ICON_SIZE: f32 = 17.0;

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
        .hover(|button| {
            button
                .bg(surface_tint(colors().hover, colors().titlebar))
                .text_color(colors().foreground)
        })
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
            return self.settings_titlebar(cx);
        }
        let left_progress = self.left_sidebar_progress;
        let right_progress = if self.workspace_section == WorkspaceSection::Workspace {
            self.right_sidebar_progress
        } else {
            0.0
        };
        let right_expanded_width = self.right_sidebar_width();
        let left_chrome_width = self.titlebar_left_width();
        let right_chrome_width = if right_progress > 0.99 {
            right_expanded_width
        } else {
            TITLEBAR_RIGHT_CHROME_COLLAPSED
                + (right_expanded_width - TITLEBAR_RIGHT_CHROME_COLLAPSED) * right_progress
        };
        let right_open = right_progress > 0.5;
        let tabs = self
            .snapshot
            .selected_workspace()
            .map(|workspace| workspace.tabs.clone())
            .unwrap_or_default();
        let selected_tab_id = self
            .snapshot
            .selected_workspace()
            .and_then(|workspace| workspace.selected_tab_id);
        let show_tab_selector = self.workspace_section == WorkspaceSection::Workspace
            && !self.visible_tab_order(cx).is_empty();
        let section_label = match self.workspace_section {
            WorkspaceSection::Workspace => self
                .snapshot
                .selected_project()
                .map(|project| project.name.as_str())
                .unwrap_or("Vibra"),
            WorkspaceSection::Inbox => "Inbox",
            WorkspaceSection::Notes => "Notes",
            WorkspaceSection::Automations => "Automations",
            WorkspaceSection::Settings => "Settings",
        }
        .to_owned();
        let mut center_chrome = div().h_full().flex_1().min_w(px(0.0)).flex().items_center();
        if show_tab_selector {
            center_chrome = center_chrome.child(self.tab_bar(tabs, selected_tab_id, cx));
        } else {
            center_chrome = center_chrome
                .px_4()
                .text_size(px(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    crate::infrastructure::window::start_drag();
                })
                .child(section_label);
        }
        div()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(surface(colors().titlebar))
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
                    .when(left_progress > 0.001, |chrome| {
                        chrome
                            .border_r_1()
                            .border_color(colors().border_subtle)
                            .bg(surface_tint(colors().sidebar, colors().titlebar))
                    })
                    // Anchored beside the traffic lights so they stay put while the sidebar animates.
                    .child(
                        self.sidebar_button("toggle-left-sidebar", true, cx, |this, _, cx| {
                            this.set_left_sidebar_visible(
                                !this.settings.left_sidebar_visible,
                                true,
                                cx,
                            );
                        }),
                    )
                    .child(self.navigation_buttons(cx))
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
                        chrome
                            .border_l_1()
                            .border_color(colors().border_subtle)
                            .bg(surface_tint(colors().sidebar, colors().titlebar))
                            .child(
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
                            .child(self.workspace_title_action(
                                "workspace-file-search",
                                "chrome-icons/search.svg",
                                "Find file · ⌘P",
                                cx,
                                |this, _, cx| this.open_palette(PaletteMode::Files, cx),
                            ))
                            .child(self.ide_button(cx))
                    })
                    .when(!right_open, |chrome| chrome.child(div().flex_1()))
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

    fn settings_titlebar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(surface(colors().titlebar))
            .child(
                div()
                    .w(px(self.left_sidebar_width()))
                    .h_full()
                    .flex_none()
                    .pl(px(TRAFFIC_LIGHTS_INSET))
                    .pr_2()
                    .flex()
                    .items_center()
                    .border_r_1()
                    .border_color(colors().border_subtle)
                    .bg(surface_tint(colors().sidebar, colors().titlebar))
                    .child(div().flex_1())
                    .child(self.navigation_buttons(cx)),
            )
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .min_w(px(0.0))
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(colors().border_subtle)
                    .text_size(px(13.0))
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |_, _, _| {
                        crate::infrastructure::window::start_drag();
                    })
                    .child(div().text_color(colors().muted).child("Settings"))
                    .child(div().text_color(colors().subtle).child("/"))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(self.settings_page.label()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors().subtle)
                            .child("Saved automatically"),
                    ),
            )
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
                div()
                    .id(SharedString::from(format!("utility-mode-{label}")))
                    .h(px(26.0))
                    .min_w(px(0.0))
                    .flex_1()
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .bg(if selected {
                        surface_tint(colors().selection, colors().sidebar)
                    } else {
                        gpui::rgba(0x00000000)
                    })
                    .text_color(if selected {
                        colors().foreground
                    } else {
                        colors().muted
                    })
                    .hover(move |tab| {
                        if selected {
                            tab
                        } else {
                            tab.bg(surface_tint(colors().hover, colors().sidebar))
                                .text_color(colors().foreground)
                        }
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_workspace_mode(item_mode, cx);
                        this.focus_selected_terminal(window, cx);
                    }))
                    .child(label)
            }))
            .into_any_element()
    }

    fn workspace_title_action(
        &self,
        id: &'static str,
        icon: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        titlebar_button(id, true)
            .tooltip(move |_, cx| sidebar_tooltip(label, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(titlebar_icon(icon))
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

    pub(super) fn ide_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        titlebar_button("open-ide", true)
            .tooltip(|_, cx| sidebar_tooltip("Open in IDE", cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_ide(&OpenIde, window, cx);
            }))
            .child(titlebar_icon("chrome-icons/open-external.svg"))
    }

    pub(super) fn ide_menu_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.ide_menu_open.then(|| {
            let right = 8.0;
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
                        .top(px(TITLEBAR_HEIGHT - 2.0))
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
                                    .child(label)
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
                    if !this.ide_icons.contains_key(editor.bundle_identifier)
                        && let Some(png) =
                            crate::infrastructure::editor::editor_icon_png(editor.bundle_identifier)
                    {
                        this.ide_icons.insert(
                            editor.bundle_identifier,
                            Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)),
                        );
                    }
                }
                this.installed_editors = editors;
                cx.notify();
            });
        }));
        cx.notify();
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
