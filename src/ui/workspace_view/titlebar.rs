//! Titlebar chrome, utility tabs, and the IDE launcher.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Div, MouseButton, SharedString, Stateful, Window, WindowControlArea, div,
    prelude::*, px, svg,
};

use crate::OpenIde;
use crate::infrastructure::editor::InstalledEditor;
use crate::ui::theme::colors;

use super::{
    LeftSidebarMode, RightSidebarMode, TITLEBAR_CHROME_COLLAPSED, TITLEBAR_HEIGHT,
    TITLEBAR_RIGHT_CHROME_COLLAPSED,
};

impl super::WorkspaceView {
    pub(super) fn titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let left_progress = self.left_sidebar_progress;
        let right_progress = self.right_sidebar_progress;
        // Keep titlebar controls aligned with the framed panels below.
        let left_chrome_width = if left_progress > 0.99 {
            self.left_sidebar_width()
        } else {
            TITLEBAR_CHROME_COLLAPSED
                + (self.left_sidebar_width() - TITLEBAR_CHROME_COLLAPSED) * left_progress
        };
        let right_chrome_width = if right_progress > 0.99 {
            self.right_sidebar_width()
        } else {
            TITLEBAR_RIGHT_CHROME_COLLAPSED
                + (self.right_sidebar_width() - TITLEBAR_RIGHT_CHROME_COLLAPSED) * right_progress
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
        let show_tab_selector = tabs.len() > 1;
        let right_chrome_content = if right_open {
            self.utility_mode_tabs(cx)
        } else {
            div()
                .h_full()
                .flex_1()
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    crate::infrastructure::window::start_drag();
                })
                .into_any_element()
        };

        let mut center_chrome = div()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .bg(colors().titlebar);
        if show_tab_selector {
            center_chrome = center_chrome.child(self.tab_bar(tabs, selected_tab_id, cx));
        } else {
            center_chrome = center_chrome
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    crate::infrastructure::window::start_drag();
                });
        }

        div()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(colors().titlebar)
            .child(
                div()
                    .w(px(left_chrome_width))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .pl(px(86.0))
                    .gap_1()
                    .bg(colors().titlebar)
                    .child(
                        self.sidebar_button("toggle-left-sidebar", true, cx, |this, _, cx| {
                            if !this.left_sidebar_visible {
                                this.left_sidebar_mode = LeftSidebarMode::Sessions;
                            }
                            this.set_left_sidebar_visible(!this.left_sidebar_visible, true, cx);
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
                    .overflow_hidden()
                    .bg(colors().titlebar)
                    .child(right_chrome_content)
                    .child(self.sidebar_button(
                        "toggle-right-sidebar",
                        false,
                        cx,
                        |this, _, cx| {
                            this.toggle_diff_panel(cx);
                        },
                    )),
            )
    }

    pub(super) fn utility_mode_tabs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.right_sidebar_mode;
        let modes = [
            (
                RightSidebarMode::Files,
                "Archivos",
                "chrome-icons/files.svg",
            ),
            (RightSidebarMode::Diff, "Git", "chrome-icons/git-branch.svg"),
        ];

        div()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .pl_3()
            .child(div().flex().items_center().children(modes.into_iter().map(
                |(item_mode, label, icon)| {
                    let selected = item_mode == mode;
                    div()
                        .id(SharedString::from(format!("utility-mode-{label}")))
                        .h(px(26.0))
                        .relative()
                        .w(px(32.0))
                        .mr_2()
                        .rounded(px(7.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .bg(if selected {
                            colors().selection
                        } else {
                            gpui::rgba(0x00000000)
                        })
                        .text_color(if selected {
                            colors().foreground
                        } else {
                            colors().subtle
                        })
                        .hover(|tab| tab.bg(colors().hover).text_color(colors().foreground))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.right_sidebar_mode = item_mode;
                            match item_mode {
                                RightSidebarMode::Files => this.refresh_project_files(cx),
                                RightSidebarMode::Diff => {
                                    this.sync_diff_root(cx);
                                    this.diff_view.update(cx, |diff_view, cx| {
                                        diff_view.refresh_now(cx);
                                    });
                                }
                            }
                            this.sync_files_watcher(cx);
                            cx.notify();
                        }))
                        .child(svg().path(icon).size(px(15.0)).text_color(if selected {
                            colors().foreground
                        } else {
                            colors().subtle
                        }))
                },
            )))
            .child(self.ide_button(cx))
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |_, _, _| {
                        crate::infrastructure::window::start_drag();
                    }),
            )
            .into_any_element()
    }

    pub(super) fn sidebar_close_button(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(18.0))
            .flex_none()
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_size(px(13.0))
            .text_color(colors().subtle)
            .hover(|close| close.bg(colors().hover).text_color(colors().foreground))
            .active(|close| close.opacity(0.72))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child("×")
    }

    pub(super) fn sidebar_icon(left: bool) -> Div {
        let panel = div()
            .w(px(4.0))
            .h_full()
            .flex_none()
            .bg(colors().foreground);
        let content = div().h_full().flex_1();
        let icon = div()
            .w(px(14.0))
            .h(px(12.0))
            .flex()
            .overflow_hidden()
            .rounded(px(2.0))
            .border_1()
            .border_color(colors().muted);

        if left {
            icon.child(panel).child(content)
        } else {
            icon.child(content).child(panel)
        }
    }

    pub(super) fn sidebar_button(
        &self,
        id: &'static str,
        left: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(24.0))
            .flex_none()
            .rounded(px(5.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(colors().titlebar)
            .hover(|button| button.bg(colors().hover))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(Self::sidebar_icon(left))
    }

    pub(super) fn ide_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("open-ide")
            .h(px(26.0))
            .relative()
            .w(px(32.0))
            .mr_2()
            .rounded(px(7.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(gpui::rgba(0x00000000))
            .text_color(colors().subtle)
            .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_ide(&OpenIde, window, cx);
            }))
            .child(
                svg()
                    .path("chrome-icons/open-external.svg")
                    .size(px(15.0))
                    .text_color(colors().subtle),
            )
    }

    pub(super) fn ide_menu_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.ide_menu_open.then(|| {
            let right = (self.right_sidebar_width() - 164.0).max(8.0);
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
                    div()
                        .id("ide-menu")
                        .absolute()
                        .top(px(34.0))
                        .right(px(right))
                        .min_w(px(190.0))
                        .py_1()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(colors().elevated)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .text_size(px(9.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors().subtle)
                                .child("ABRIR CARPETA EN"),
                        )
                        .children(self.installed_editors.clone().into_iter().enumerate().map(
                            |(index, editor)| {
                                let label = editor.name;
                                let icon = self.ide_icons.get(editor.bundle_identifier).cloned();
                                let icon = match icon {
                                    Some(icon) => gpui::img(icon).size(px(16.0)).into_any_element(),
                                    None => svg()
                                        .path("chrome-icons/open-external.svg")
                                        .size(px(16.0))
                                        .text_color(colors().subtle)
                                        .into_any_element(),
                                };
                                div()
                                    .id(SharedString::from(format!("ide-menu-item-{index}")))
                                    .h(px(32.0))
                                    .mx_1()
                                    .px_3()
                                    .rounded(px(5.0))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(colors().foreground)
                                    .hover(|item| item.bg(colors().hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_with_editor(editor.clone(), cx);
                                    }))
                                    .child(div().size(px(16.0)).flex_none().child(icon))
                                    .child(label)
                            },
                        )),
                )
                .into_any_element()
        })
    }

    pub(super) fn open_ide(&mut self, _: &OpenIde, _: &mut Window, cx: &mut Context<Self>) {
        if self.ide_menu_open {
            self.ide_menu_open = false;
            cx.notify();
            return;
        }
        self.installed_editors = crate::infrastructure::editor::installed_editors();
        for editor in &self.installed_editors {
            if !self.ide_icons.contains_key(editor.bundle_identifier)
                && let Some(png) =
                    crate::infrastructure::editor::editor_icon_png(editor.bundle_identifier)
            {
                self.ide_icons.insert(
                    editor.bundle_identifier,
                    Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)),
                );
            }
        }
        if self.installed_editors.is_empty() {
            self.persistence_error = Some(
                "No se encontró un IDE compatible. Instala Cursor, VS Code, Windsurf, Zed, Xcode, Sublime Text o VSCodium"
                    .into(),
            );
        } else {
            self.ide_menu_open = true;
            self.context_menu = None;
        }
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
                            error.to_string().starts_with("No se pudo abrir el IDE:")
                        }) {
                            this.persistence_error = None;
                        }
                    }
                    Err(error) => {
                        this.persistence_error =
                            Some(format!("No se pudo abrir el IDE: {error}").into());
                    }
                }
                cx.notify();
            });
        }));
    }
}
