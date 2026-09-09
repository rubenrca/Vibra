//! Lifecycle of the utility terminals, owned by a workspace rather than a pane.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, Context, DragMoveEvent, Entity, Focusable, MouseButton, MouseDownEvent,
    SharedString, Window, div, prelude::*, px,
};
use uuid::Uuid;

use crate::ToggleDevTerminal;
use crate::domain::workspace::WorkspaceSplitAxis;
use crate::ui::agent_marks::agent_compact_badge;
use crate::ui::terminal::{TerminalView, TerminalViewEvent};
use crate::ui::theme::{MONO_FONT, colors};

use super::{
    ContextMenuKind, DEV_TERMINAL_PRIMARY_RESERVE, DevTerminalResize, MAX_DEV_TERMINAL_HEIGHT,
    MIN_DEV_TERMINAL_HEIGHT, PANE_HEADER_HEIGHT, PANEL_GAP, PANEL_RADIUS, PaneDividerDragView,
    WorkspaceView, directory_basename, format_sidebar_path, split_gutter,
};

pub(super) struct DevTerminalDrawer {
    pub(super) terminals: Vec<Entity<TerminalView>>,
    pub(super) selected_id: Uuid,
    pub(super) visible: bool,
}

impl WorkspaceView {
    pub(super) fn is_dev_terminal_visible(&self) -> bool {
        self.current_workspace_id()
            .and_then(|workspace_id| self.dev_terminals.get(&workspace_id))
            .is_some_and(|drawer| drawer.visible)
    }

    pub(super) fn is_dev_terminal(&self, session_id: Uuid, cx: &Context<Self>) -> bool {
        self.dev_workspace_for_session(session_id, cx).is_some()
    }

    pub(super) fn dev_workspace_for_session(
        &self,
        session_id: Uuid,
        cx: &Context<Self>,
    ) -> Option<Uuid> {
        self.dev_terminals
            .iter()
            .find_map(|(workspace_id, drawer)| {
                drawer
                    .terminals
                    .iter()
                    .any(|terminal| terminal.read(cx).session_id() == session_id)
                    .then_some(*workspace_id)
            })
    }

    pub(super) fn selected_dev_terminal(
        &self,
        workspace_id: Uuid,
        cx: &Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        let drawer = self.dev_terminals.get(&workspace_id)?;
        drawer
            .terminals
            .iter()
            .find(|terminal| terminal.read(cx).session_id() == drawer.selected_id)
            .or_else(|| drawer.terminals.first())
            .cloned()
    }

    fn spawn_dev_terminal(
        &mut self,
        workspace_id: Uuid,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let working_directory = self.selected_live_cwd(cx);
        let terminal_port = self.terminal_port.clone();
        let font_size = self.settings.terminal_font_size;
        let visible = self
            .dev_terminals
            .get(&workspace_id)
            .is_some_and(|drawer| drawer.visible);
        let mut environment = HashMap::new();
        if let Ok(executable) = std::env::current_exe() {
            environment.insert(
                "VIBRA_CLI".into(),
                executable.to_string_lossy().into_owned(),
            );
        }
        let session_id = Uuid::new_v4();
        let terminal = cx.new(|cx| {
            TerminalView::new_with_environment(
                session_id,
                "Dev terminal".to_owned(),
                &working_directory,
                terminal_port,
                environment,
                cx,
            )
        });
        terminal.update(cx, |terminal, cx| {
            terminal.apply_font_size(font_size, cx);
            terminal.set_surface_visible(visible);
        });
        let subscription = cx.subscribe(
            &terminal,
            |this, _terminal, event: &TerminalViewEvent, cx| {
                this.handle_terminal_view_event(event, cx);
            },
        );
        self.dev_terminal_subscriptions
            .insert(session_id, subscription);
        let drawer = self
            .dev_terminals
            .entry(workspace_id)
            .or_insert_with(|| DevTerminalDrawer {
                terminals: Vec::new(),
                selected_id: session_id,
                visible: false,
            });
        drawer.terminals.push(terminal.clone());
        drawer.selected_id = session_id;
        terminal
    }

    fn ensure_dev_terminal(&mut self, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let workspace_id = self.current_workspace_id()?;
        if let Some(terminal) = self.selected_dev_terminal(workspace_id, cx) {
            return Some(terminal);
        }
        Some(self.spawn_dev_terminal(workspace_id, cx))
    }

    pub(super) fn prune_dev_terminals(&mut self, cx: &mut Context<Self>) {
        let live: HashSet<Uuid> = self
            .snapshot
            .workspace_entries()
            .into_iter()
            .map(|entry| entry.workspace_id)
            .collect();
        let stale: Vec<Uuid> = self
            .dev_terminals
            .keys()
            .copied()
            .filter(|workspace_id| !live.contains(workspace_id))
            .collect();
        for workspace_id in stale {
            self.shutdown_dev_drawer(workspace_id, cx);
        }
    }

    fn shutdown_dev_drawer(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let Some(drawer) = self.dev_terminals.remove(&workspace_id) else {
            return;
        };
        for terminal in drawer.terminals {
            let session_id = terminal.read(cx).session_id();
            terminal.read(cx).shutdown();
            self.dev_terminal_subscriptions.remove(&session_id);
        }
    }

    pub(super) fn set_dev_terminal_visible(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.current_workspace_id() else {
            return;
        };
        if visible {
            let Some(terminal) = self.ensure_dev_terminal(cx) else {
                return;
            };
            if let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) {
                drawer.visible = true;
            }
            self.sync_terminal_surface_visibility(cx);
            cx.notify();
            cx.defer_in(window, move |_, window, cx| {
                terminal.read(cx).focus_handle(cx).focus(window);
            });
            return;
        }
        let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) else {
            return;
        };
        if !drawer.visible {
            return;
        }
        drawer.visible = false;
        self.pending_focus_dev_terminal = false;
        self.sync_terminal_surface_visibility(cx);
        self.focus_selected_terminal(window, cx);
        cx.notify();
    }

    pub(super) fn toggle_dev_terminal(
        &mut self,
        _: &ToggleDevTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_dev_terminal_visible(!self.is_dev_terminal_visible(), window, cx);
    }

    pub(super) fn add_dev_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.current_workspace_id() else {
            return;
        };
        let terminal = self.spawn_dev_terminal(workspace_id, cx);
        self.select_dev_terminal_tab(terminal.read(cx).session_id(), window, cx);
    }

    pub(super) fn select_dev_terminal_tab(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.dev_workspace_for_session(session_id, cx) else {
            return;
        };
        if let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) {
            drawer.selected_id = session_id;
            drawer.visible = true;
        }
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
        if let Some(terminal) = self.selected_dev_terminal(workspace_id, cx) {
            cx.defer_in(window, move |_, window, cx| {
                terminal.read(cx).focus_handle(cx).focus(window);
            });
        }
    }

    pub(super) fn close_dev_terminal(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.dev_workspace_for_session(session_id, cx) else {
            return;
        };
        let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) else {
            return;
        };
        let Some(index) = drawer
            .terminals
            .iter()
            .position(|terminal| terminal.read(cx).session_id() == session_id)
        else {
            return;
        };
        drawer.terminals.remove(index).read(cx).shutdown();
        self.dev_terminal_subscriptions.remove(&session_id);
        if drawer.terminals.is_empty() {
            self.dev_terminals.remove(&workspace_id);
            self.pending_focus_dev_terminal = false;
            self.sync_terminal_surface_visibility(cx);
            self.focus_selected_terminal(window, cx);
            cx.notify();
            return;
        }
        if drawer.selected_id == session_id {
            let next = index.min(drawer.terminals.len() - 1);
            drawer.selected_id = drawer.terminals[next].read(cx).session_id();
        }
        let selected = self.selected_dev_terminal(workspace_id, cx);
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
        if let Some(terminal) = selected {
            cx.defer_in(window, move |_, window, cx| {
                terminal.read(cx).focus_handle(cx).focus(window);
            });
        }
    }

    pub(super) fn on_dev_terminal_resize_move(
        &mut self,
        event: &DragMoveEvent<DevTerminalResize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let y: f32 = event.event.position.y.into();
        let top: f32 = event.bounds.top().into();
        let bottom: f32 = event.bounds.bottom().into();
        let available = (bottom - top).max(MIN_DEV_TERMINAL_HEIGHT + DEV_TERMINAL_PRIMARY_RESERVE);
        let max_height = (available - DEV_TERMINAL_PRIMARY_RESERVE).min(MAX_DEV_TERMINAL_HEIGHT);
        let height = (bottom - y - PANEL_GAP).clamp(MIN_DEV_TERMINAL_HEIGHT, max_height);
        if (self.dev_terminal_height - height).abs() < 0.5 {
            return;
        }
        self.dev_terminal_height = height;
        cx.notify();
    }

    pub(super) fn dev_terminal_drawer(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.is_dev_terminal_visible() {
            return None;
        }
        let workspace_id = self.current_workspace_id()?;
        let drawer = self.dev_terminals.get(&workspace_id)?;
        let selected_id = drawer.selected_id;
        let tab_count = drawer.terminals.len();
        let can_close = tab_count > 1;
        let tabs: Vec<(Uuid, String, Entity<TerminalView>)> = drawer
            .terminals
            .iter()
            .enumerate()
            .map(|(index, terminal)| {
                let session_id = terminal.read(cx).session_id();
                let cwd = terminal.read(cx).current_working_directory();
                let name = directory_basename(&cwd.to_string_lossy());
                let title = if name == "—" {
                    format!("Terminal {}", index + 1)
                } else {
                    name
                };
                (session_id, title, terminal.clone())
            })
            .collect();
        let selected_terminal = tabs
            .iter()
            .find(|(session_id, _, _)| *session_id == selected_id)
            .or(tabs.first())
            .map(|(_, _, terminal)| terminal.clone())?;
        let focused = selected_terminal
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let presence = self.resolved_agent_presence(selected_id);
        let selected_cwd = selected_terminal
            .read(cx)
            .current_working_directory()
            .to_string_lossy()
            .into_owned();
        let detail = (tab_count == 1)
            .then(|| format_sidebar_path(&selected_cwd, self.home_directory.as_deref()))
            .filter(|path| path != "—");
        let tabs_grow = detail.is_none();
        Some(
            div()
                .id("dev-terminal-pane")
                .flex_none()
                .flex()
                .flex_col()
                .child(
                    split_gutter("dev-terminal-divider", WorkspaceSplitAxis::Vertical).on_drag(
                        DevTerminalResize,
                        |_, _, _, cx| {
                            cx.new(|_| PaneDividerDragView {
                                axis: WorkspaceSplitAxis::Vertical,
                            })
                        },
                    ),
                )
                .child(
                    div()
                        .id("dev-terminal-drawer")
                        .h(px(self.dev_terminal_height))
                        .min_h(px(MIN_DEV_TERMINAL_HEIGHT))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(px(PANEL_RADIUS))
                        .border_1()
                        .border_color(if focused {
                            colors().accent
                        } else {
                            colors().border_subtle
                        })
                        .bg(colors().terminal)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.select_dev_terminal_tab(selected_id, window, cx);
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                this.select_dev_terminal_tab(selected_id, window, cx);
                                this.open_context_menu(
                                    ContextMenuKind::Pane {
                                        session_id: selected_id,
                                    },
                                    f32::from(event.position.x),
                                    f32::from(event.position.y),
                                    cx,
                                );
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            div()
                                .h(px(PANE_HEADER_HEIGHT))
                                .w_full()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(8.0))
                                .overflow_hidden()
                                .bg(if focused {
                                    colors().elevated
                                } else {
                                    colors().terminal
                                })
                                .border_b_1()
                                .border_color(colors().border_subtle)
                                .child(agent_compact_badge(
                                    presence.as_ref().map(|presence| presence.kind.as_str()),
                                    presence.as_ref().map(|presence| presence.state),
                                    presence.as_ref().and_then(|presence| presence.attention),
                                    focused,
                                ))
                                .child(
                                    div()
                                        .min_w(px(0.0))
                                        .when(tabs_grow, |row| row.flex_1())
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .overflow_x_hidden()
                                        .children(tabs.into_iter().map(
                                            |(session_id, title, _)| {
                                                let selected = session_id == selected_id;
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "dev-terminal-tab-{session_id}"
                                                    )))
                                                    .h(px(20.0))
                                                    .flex_none()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(4.0))
                                                    .px(px(8.0))
                                                    .rounded_full()
                                                    .cursor_pointer()
                                                    .bg(if selected {
                                                        colors().selection
                                                    } else {
                                                        gpui::rgba(0x00000000)
                                                    })
                                                    .border_1()
                                                    .border_color(if selected {
                                                        colors().muted
                                                    } else {
                                                        colors().border_subtle
                                                    })
                                                    .text_color(if selected {
                                                        colors().foreground
                                                    } else {
                                                        colors().muted
                                                    })
                                                    .hover(|tab| {
                                                        if selected {
                                                            tab
                                                        } else {
                                                            tab.bg(colors().hover)
                                                                .text_color(colors().foreground)
                                                        }
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.select_dev_terminal_tab(
                                                                session_id, window, cx,
                                                            );
                                                        },
                                                    ))
                                                    .on_mouse_down(
                                                        MouseButton::Right,
                                                        cx.listener(
                                                            move |this,
                                                                  event: &MouseDownEvent,
                                                                  window,
                                                                  cx| {
                                                                this.select_dev_terminal_tab(
                                                                    session_id, window, cx,
                                                                );
                                                                this.open_context_menu(
                                                                    ContextMenuKind::Pane {
                                                                        session_id,
                                                                    },
                                                                    f32::from(event.position.x),
                                                                    f32::from(event.position.y),
                                                                    cx,
                                                                );
                                                                cx.stop_propagation();
                                                            },
                                                        ),
                                                    )
                                                    .child(
                                                        div()
                                                            .max_w(px(160.0))
                                                            .truncate()
                                                            .font_family(MONO_FONT)
                                                            .text_size(px(10.5))
                                                            .font_weight(if selected {
                                                                gpui::FontWeight::MEDIUM
                                                            } else {
                                                                gpui::FontWeight::NORMAL
                                                            })
                                                            .child(title),
                                                    )
                                                    .when(can_close, |tab| {
                                                        tab.child(
                                                            div()
                                                                .id(SharedString::from(format!(
                                                                    "dev-terminal-close-{session_id}"
                                                                )))
                                                                .size(px(14.0))
                                                                .rounded_full()
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .text_size(px(11.0))
                                                                .text_color(colors().subtle)
                                                                .hover(|button| {
                                                                    button
                                                                        .bg(colors().hover)
                                                                        .text_color(
                                                                            colors().foreground,
                                                                        )
                                                                })
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    cx.listener(
                                                                        move |_, _, _, cx| {
                                                                            cx.stop_propagation();
                                                                        },
                                                                    ),
                                                                )
                                                                .on_click(cx.listener(
                                                                    move |this, _, window, cx| {
                                                                        cx.stop_propagation();
                                                                        this.close_dev_terminal(
                                                                            session_id, window, cx,
                                                                        );
                                                                    },
                                                                ))
                                                                .child("×"),
                                                        )
                                                    })
                                            },
                                        )),
                                )
                                .when_some(detail, |header, path| {
                                    header.child(
                                        div()
                                            .min_w(px(0.0))
                                            .flex_1()
                                            .truncate()
                                            .text_right()
                                            .font_family(MONO_FONT)
                                            .text_size(px(9.0))
                                            .text_color(colors().subtle)
                                            .child(path),
                                    )
                                })
                                .child(
                                    div()
                                        .id("dev-terminal-add")
                                        .size(px(20.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .text_size(px(14.0))
                                        .text_color(colors().muted)
                                        .hover(|button| button.text_color(colors().foreground))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _, _, cx| {
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.add_dev_terminal(window, cx);
                                        }))
                                        .child("+"),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .overflow_hidden()
                                .child(selected_terminal),
                        ),
                )
                .into_any_element(),
        )
    }
}
