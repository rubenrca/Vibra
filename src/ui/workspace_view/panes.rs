//! Split pane layout, tab strip, and pane keyboard/mouse actions.

use gpui::{
    AnyElement, Context, DragMoveEvent, Focusable, MouseButton, MouseDownEvent, MouseUpEvent,
    SharedString, Window, WindowControlArea, div, prelude::*, px, relative,
};
use uuid::Uuid;

use crate::domain::workspace::{
    PaneBranch, PaneFocusDirection, PaneLayoutSnapshot, PaneResizeDirection, PaneSplitDirection,
    TabSnapshot, WorkspaceSplitAxis,
};
use crate::ui::agent_marks::{TERMINAL_GLYPH, agent_status_color};
use crate::ui::terminal::TerminalDragPreview;
use crate::ui::theme::{MONO_FONT, colors};
use crate::{
    EqualizePanes, FocusPaneDown, FocusPaneLeft, FocusPaneRight, FocusPaneUp, NextPane,
    PreviousPane, ResizePaneDown, ResizePaneLeft, ResizePaneRight, ResizePaneUp, SplitPaneDown,
    SplitPaneLeft, SplitPaneRight, SplitPaneUp, TogglePaneZoom,
};

use super::{
    ContextMenuKind, PANEL_RADIUS, PaneDividerDrag, PaneDividerDragView, PaneDrag, ReorderDrag,
    TabDrag, TabDragView, split_gutter,
};

impl super::WorkspaceView {
    fn center_is_bento(&self) -> bool {
        let split_tiles = self
            .snapshot
            .selected_tab()
            .is_some_and(|tab| tab.sessions.len() > 1 && tab.zoomed_session_id.is_none());
        split_tiles || self.is_dev_terminal_visible()
    }

    pub(super) fn center_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let canvas = self.terminal_canvas(window, cx).into_any_element();
        let drawer = self.dev_terminal_drawer(window, cx);
        let bento = self.center_is_bento();
        div()
            .id("center-panel")
            .flex_1()
            .min_w(px(360.0))
            .h_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .overflow_hidden()
            .when(bento, |panel| panel.bg(colors().background))
            .when(!bento, |panel| {
                panel
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().terminal)
            })
            .on_drag_move(cx.listener(Self::on_dev_terminal_resize_move))
            .child(canvas)
            .children(drawer)
    }

    pub(super) fn tab_bar(
        &mut self,
        tabs: Vec<TabSnapshot>,
        selected_tab_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_reorder = tabs.len() > 1;
        let tab_count = tabs.len();
        let dragging_tab = match self.reorder_drag {
            Some(ReorderDrag::Tab(id)) if cx.has_active_drag() => Some(id),
            _ => None,
        };
        let tab_ids = tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        let tab_list = div()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .overflow_x_hidden()
            .when(can_reorder, |list| {
                list.child(div().w(px(24.0)).flex_none())
            })
            .children(tabs.into_iter().enumerate().map(|(index, tab)| {
                let tab_id = tab.id;
                let after_tab_id = tab_ids.get(index + 1).copied();
                let tab_order = tab_ids.clone();
                let selected = Some(tab_id) == selected_tab_id;
                let session = tab
                    .sessions
                    .iter()
                    .find(|session| Some(session.id) == tab.selected_session_id)
                    .or_else(|| tab.sessions.first());
                let identity = session.map(|session| self.pane_identity(session, index, cx));
                let session_id = session.map(|session| session.id);
                let title = identity
                    .as_ref()
                    .map(|identity| identity.title.clone())
                    .unwrap_or_else(|| format!("Terminal {}", index + 1));
                let title = if tab_count > 1 {
                    format!("{title} {}", index + 1)
                } else {
                    title
                };
                let shortcut = (index < 9).then(|| format!("⌘{}", index + 1));
                let pane_count = tab.sessions.len();
                // A tab identifies its focused pane. Background agents keep their state in
                // their own pane headers instead of changing an unrelated tab dot.
                let agent_color = identity.as_ref().and_then(|identity| {
                    agent_status_color(identity.agent_state, identity.agent_attention)
                });
                let drag = TabDrag {
                    tab_id,
                    title: title.clone(),
                    selected,
                    shortcut: shortcut.clone(),
                    tab_count,
                };
                let is_source = dragging_tab == Some(tab_id);
                div()
                    .id(SharedString::from(format!("tab-{tab_id}")))
                    .h(px(26.0))
                    .min_w(px(0.0))
                    .flex_1()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(10.0))
                    .rounded_full()
                    .when(can_reorder, |tab| tab.cursor_move())
                    .when(!can_reorder, |tab| tab.cursor_pointer())
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
                            tab.bg(colors().hover).text_color(colors().foreground)
                        }
                    })
                    .active(|tab| tab.opacity(0.88))
                    .when(is_source, |tab| tab.opacity(0.45))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.reorder_drag = Some(ReorderDrag::Tab(tab_id));
                            this.select_tab(tab_id, window, cx);
                            // Tabs own their drag gesture; only the empty bar moves the window.
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_tab(tab_id, window, cx);
                    }))
                    .when_some(session_id, |tab, session_id| {
                        // Keep pane actions accessible when a CLI captures terminal mouse input.
                        tab.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                this.select_tab(tab_id, window, cx);
                                this.select_terminal(session_id, window, cx);
                                let x: f32 = event.position.x.into();
                                let y: f32 = event.position.y.into();
                                this.open_context_menu(
                                    ContextMenuKind::Pane { session_id },
                                    x,
                                    y,
                                    cx,
                                );
                                cx.stop_propagation();
                            }),
                        )
                    })
                    .when(can_reorder, |tab| {
                        tab.on_drag(drag, |drag, _, window, cx| {
                            // Tabs fill the central chrome, so derive the preview width from the
                            // live window and tab count instead of rendering a compact chip.
                            let window_width: f32 = window.bounds().size.width.into();
                            let width =
                                (window_width * (0.52 / drag.tab_count as f32)).clamp(160.0, 420.0);
                            cx.new(|_| TabDragView {
                                title: drag.title.clone(),
                                selected: drag.selected,
                                shortcut: drag.shortcut.clone(),
                                width,
                            })
                        })
                        .can_drop(move |value, _, _| {
                            value
                                .downcast_ref::<TabDrag>()
                                .is_some_and(|drag| drag.tab_id != tab_id)
                        })
                        .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                            let dragged_from_left = tab_order
                                .iter()
                                .position(|id| *id == drag.tab_id)
                                .is_some_and(|source_index| source_index < index);
                            let before_tab_id = if dragged_from_left {
                                after_tab_id
                            } else {
                                Some(tab_id)
                            };
                            this.reorder_tab(drag.tab_id, before_tab_id, window, cx);
                        }))
                        .drag_over::<TabDrag>(|style, _, _, _| style.bg(colors().hover))
                    })
                    .when_some(agent_color, |tab, color| {
                        tab.child(
                            div()
                                .absolute()
                                .left(px(12.0))
                                .size(px(6.0))
                                .rounded_full()
                                .bg(color),
                        )
                    })
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_center()
                                    .text_size(px(12.0))
                                    .font_weight(if selected {
                                        gpui::FontWeight::MEDIUM
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .child(title),
                            )
                            .when(pane_count > 1, |label| {
                                label.child(
                                    div()
                                        .flex_none()
                                        .px(px(5.0))
                                        .py(px(1.0))
                                        .rounded(px(4.0))
                                        .bg(colors().elevated)
                                        .font_family(MONO_FONT)
                                        .text_size(px(8.5))
                                        .text_color(colors().subtle)
                                        .child(format!("{pane_count} panes")),
                                )
                            }),
                    )
                    .when_some(shortcut, |tab, shortcut| {
                        tab.child(
                            div()
                                .absolute()
                                .right(px(10.0))
                                .font_family(MONO_FONT)
                                .text_size(px(9.5))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(if selected {
                                    colors().muted
                                } else {
                                    colors().subtle
                                })
                                .child(shortcut),
                        )
                    })
            }))
            .when(can_reorder, |list| {
                list.child(
                    div()
                        .id("tab-drop-end")
                        .h_full()
                        .w(px(24.0))
                        .flex_none()
                        .can_drop(|value, _, _| value.downcast_ref::<TabDrag>().is_some())
                        .drag_over::<TabDrag>(|style, _, _, _| style.bg(colors().hover))
                        .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                            this.reorder_tab(drag.tab_id, None, window, cx);
                        })),
                )
            });

        div()
            .h_full()
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .px(px(12.0))
            .bg(colors().terminal)
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                crate::infrastructure::window::start_drag();
                cx.stop_propagation();
            })
            .child(tab_list)
    }

    pub(super) fn render_pane_layout(
        &mut self,
        layout: &PaneLayoutSnapshot,
        path: Vec<PaneBranch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            PaneLayoutSnapshot::Terminal { id } => {
                let session_id = *id;
                let terminal = self.terminals.get(&session_id).cloned();
                let drag_preview = terminal
                    .as_ref()
                    .map(|terminal| terminal.read(cx).drag_preview())
                    .unwrap_or_else(TerminalDragPreview::empty);
                let pane_count = self
                    .snapshot
                    .selected_tab()
                    .map_or(1, |tab| tab.sessions.len());
                let framed = self.center_is_bento();
                let highlighted = terminal
                    .as_ref()
                    .is_some_and(|terminal| terminal.read(cx).focus_handle(cx).is_focused(window));
                let can_drag = self
                    .snapshot
                    .selected_tab()
                    .is_some_and(|tab| tab.zoomed_session_id.is_none() && pane_count > 1);
                let dragging_pane = match self.reorder_drag {
                    Some(ReorderDrag::Pane(id)) if cx.has_active_drag() => Some(id),
                    _ => None,
                };
                let is_source = dragging_pane == Some(session_id);
                let drag = PaneDrag {
                    session_id,
                    preview: drag_preview,
                };
                div()
                    .id(SharedString::from(format!("pane-{session_id}")))
                    .size_full()
                    .min_w(px(80.0))
                    .min_h(px(48.0))
                    .relative()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .bg(colors().terminal)
                    .when(framed, |pane| {
                        pane.rounded(px(PANEL_RADIUS))
                            .border_1()
                            .border_color(if highlighted {
                                colors().accent
                            } else {
                                colors().border_subtle
                            })
                    })
                    .when(is_source, |pane| pane.opacity(0.55))
                    .when(can_drag, |pane| {
                        pane.cursor_move()
                            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.preview.clone()))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if can_drag {
                                this.reorder_drag = Some(ReorderDrag::Pane(session_id));
                            }
                            this.close_context_menu(cx);
                            this.select_terminal(session_id, window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.select_terminal(session_id, window, cx);
                            let x: f32 = event.position.x.into();
                            let y: f32 = event.position.y.into();
                            this.open_context_menu(ContextMenuKind::Pane { session_id }, x, y, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .when_some(terminal, |pane, terminal| {
                        pane.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .child(terminal),
                        )
                    })
                    // GPUI registers drop listeners while laying out the element. Keep this
                    // transparent target mounted before a drag begins; mounting it only once a
                    // drag is active means it never receives the drop that started that drag.
                    .child(
                        div()
                            .id(SharedString::from(format!("pane-drop-{session_id}")))
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .can_drop(move |value, _, _| {
                                value
                                    .downcast_ref::<PaneDrag>()
                                    .is_some_and(|drag| drag.session_id != session_id)
                            })
                            .drag_over::<PaneDrag>(|style, _, _, _| {
                                style
                                    .border_2()
                                    .border_color(colors().accent)
                                    .bg(colors().selection)
                            })
                            .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                                this.swap_panes(drag.session_id, session_id, window, cx);
                            })),
                    )
                    .into_any_element()
            }
            PaneLayoutSnapshot::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let axis = *axis;
                let fraction = f32::from(*ratio) / 10_000.0;
                let mut first_path = path.clone();
                first_path.push(PaneBranch::First);
                let mut second_path = path.clone();
                second_path.push(PaneBranch::Second);
                let first = self.render_pane_layout(first, first_path, window, cx);
                let second = self.render_pane_layout(second, second_path, window, cx);
                let divider_id = format!(
                    "pane-divider-{}",
                    path.iter()
                        .map(|branch| match branch {
                            PaneBranch::First => '0',
                            PaneBranch::Second => '1',
                        })
                        .collect::<String>()
                );
                let drag = PaneDividerDrag {
                    path: path.clone(),
                    axis,
                };
                let divider = split_gutter(divider_id, axis)
                    .on_drag(drag, move |drag, _, _, cx| {
                        cx.new(|_| PaneDividerDragView { axis: drag.axis })
                    });
                let listener_path = path;
                div()
                    .size_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .when(axis == WorkspaceSplitAxis::Horizontal, |split| {
                        split.flex_row()
                    })
                    .when(axis == WorkspaceSplitAxis::Vertical, |split| {
                        split.flex_col()
                    })
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<PaneDividerDrag>, _, cx| {
                            let drag = event.drag(cx).clone();
                            if drag.path != listener_path || drag.axis != axis {
                                return;
                            }
                            let (offset, length): (f32, f32) = match axis {
                                WorkspaceSplitAxis::Horizontal => (
                                    (event.event.position.x - event.bounds.left()).into(),
                                    event.bounds.size.width.into(),
                                ),
                                WorkspaceSplitAxis::Vertical => (
                                    (event.event.position.y - event.bounds.top()).into(),
                                    event.bounds.size.height.into(),
                                ),
                            };
                            if length <= 0.0 {
                                return;
                            }
                            let ratio = ((offset / length) * 10_000.0).round() as u16;
                            if this.snapshot.set_selected_split_ratio(&drag.path, ratio) {
                                this.pane_resize_dirty = true;
                                cx.notify();
                            }
                        },
                    ))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .flex_none()
                            .when(axis == WorkspaceSplitAxis::Horizontal, |pane| {
                                pane.w(relative(fraction)).h_full()
                            })
                            .when(axis == WorkspaceSplitAxis::Vertical, |pane| {
                                pane.h(relative(fraction)).w_full()
                            })
                            .child(first),
                    )
                    .child(divider)
                    .child(div().min_w(px(0.0)).min_h(px(0.0)).flex_1().child(second))
                    .into_any_element()
            }
        }
    }

    pub(super) fn terminal_canvas(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tab = self.snapshot.selected_tab().cloned();
        let panes = tab.as_ref().map(|tab| {
            if let Some(zoomed_id) = tab.zoomed_session_id {
                self.render_pane_layout(
                    &PaneLayoutSnapshot::terminal(zoomed_id),
                    Vec::new(),
                    window,
                    cx,
                )
            } else {
                self.render_pane_layout(&tab.layout, Vec::new(), window, cx)
            }
        });
        let is_empty = panes.is_none();
        let zoomed = tab.as_ref().and_then(|tab| tab.zoomed_session_id).is_some();
        let bento = self.center_is_bento();

        // One terminal fills the center card. Several tiles (splits or ⌘J) drop
        // that outer frame and each pane paints its own bento border.
        div()
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .bg(if bento {
                colors().background
            } else {
                colors().terminal
            })
            .when_some(panes, |canvas, panes| canvas.child(panes))
            .when(zoomed, |canvas| {
                canvas.child(
                    div()
                        .absolute()
                        .left_3()
                        .bottom_3()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(colors().elevated)
                        .text_xs()
                        .text_color(colors().muted)
                        .child("Pane ampliado · ⇧⌘↵ restaurar"),
                )
            })
            .when(is_empty, |canvas| {
                canvas.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .child(
                            div()
                                .size(px(32.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(13.0))
                                .text_color(colors().muted)
                                .child(TERMINAL_GLYPH),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors().muted)
                                .child("Ninguna terminal seleccionada"),
                        ),
                )
            })
    }

    pub(super) fn split_pane(
        &mut self,
        direction: PaneSplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.capture_selected_working_directory(cx);
        if let Some(session_id) = self.snapshot.split_selected_terminal(direction) {
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.persist(cx);
            self.focus_terminal(session_id, window, cx);
        }
    }

    pub(super) fn focus_pane(
        &mut self,
        direction: PaneFocusDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.focus_terminal(direction) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    pub(super) fn cycle_pane(
        &mut self,
        offset: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.cycle_terminal(offset) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    pub(super) fn resize_pane(&mut self, direction: PaneResizeDirection, cx: &mut Context<Self>) {
        if self.snapshot.resize_selected_pane(direction) {
            self.persist(cx);
        }
    }

    pub(super) fn finish_pane_resize(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_resize_dirty {
            self.pane_resize_dirty = false;
            self.persist(cx);
        }
        if self.sidebar_resize_dirty {
            self.sidebar_resize_dirty = false;
            self.persist_settings(cx);
        }
        if self.reorder_drag.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn swap_panes(
        &mut self,
        from: Uuid,
        onto: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reorder_drag = None;
        if self.snapshot.swap_tab_terminals(from, onto) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.refresh_sidebar_workspace_meta(cx);
            self.persist(cx);
            self.focus_terminal(from, window, cx);
        }
        cx.notify();
    }

    pub(super) fn split_pane_left(
        &mut self,
        _: &SplitPaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_pane(PaneSplitDirection::Left, window, cx);
    }

    pub(super) fn split_pane_right(
        &mut self,
        _: &SplitPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_pane(PaneSplitDirection::Right, window, cx);
    }

    pub(super) fn split_pane_up(
        &mut self,
        _: &SplitPaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_pane(PaneSplitDirection::Up, window, cx);
    }

    pub(super) fn split_pane_down(
        &mut self,
        _: &SplitPaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_pane(PaneSplitDirection::Down, window, cx);
    }

    pub(super) fn focus_pane_left(
        &mut self,
        _: &FocusPaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneFocusDirection::Left, window, cx);
    }

    pub(super) fn focus_pane_right(
        &mut self,
        _: &FocusPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneFocusDirection::Right, window, cx);
    }

    pub(super) fn focus_pane_up(
        &mut self,
        _: &FocusPaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneFocusDirection::Up, window, cx);
    }

    pub(super) fn focus_pane_down(
        &mut self,
        _: &FocusPaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneFocusDirection::Down, window, cx);
    }

    pub(super) fn previous_pane(
        &mut self,
        _: &PreviousPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_pane(-1, window, cx);
    }

    pub(super) fn next_pane(&mut self, _: &NextPane, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_pane(1, window, cx);
    }

    pub(super) fn resize_pane_left(
        &mut self,
        _: &ResizePaneLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_pane(PaneResizeDirection::Left, cx);
    }

    pub(super) fn resize_pane_right(
        &mut self,
        _: &ResizePaneRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_pane(PaneResizeDirection::Right, cx);
    }

    pub(super) fn resize_pane_up(
        &mut self,
        _: &ResizePaneUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_pane(PaneResizeDirection::Up, cx);
    }

    pub(super) fn resize_pane_down(
        &mut self,
        _: &ResizePaneDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_pane(PaneResizeDirection::Down, cx);
    }

    pub(super) fn equalize_panes(
        &mut self,
        _: &EqualizePanes,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.equalize_selected_panes() {
            self.persist(cx);
        }
    }

    pub(super) fn toggle_pane_zoom(
        &mut self,
        _: &TogglePaneZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.toggle_selected_pane_zoom() {
            self.sync_terminal_surface_visibility(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }
}
