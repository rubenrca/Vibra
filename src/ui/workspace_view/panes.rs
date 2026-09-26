//! Split pane layout, tab strip, and pane keyboard/mouse actions.

use gpui::{
    AnyElement, Context, DragMoveEvent, Focusable, MouseButton, MouseDownEvent, MouseUpEvent,
    SharedString, Window, WindowControlArea, div, prelude::*, px, relative, svg,
};
use uuid::Uuid;

use crate::domain::workspace::{
    PaneBranch, PaneFocusDirection, PaneLayoutSnapshot, PaneResizeDirection, PaneSplitDirection,
    TabSnapshot, WorkspaceSplitAxis,
};
use crate::ui::agent_marks::agent_compact_badge;
use crate::ui::terminal::TerminalDragPreview;
use crate::ui::theme::{MONO_FONT, colors, surface, surface_tint};
use crate::{
    EqualizePanes, FocusPaneDown, FocusPaneLeft, FocusPaneRight, FocusPaneUp, NextPane,
    PreviousPane, ResizePaneDown, ResizePaneLeft, ResizePaneRight, ResizePaneUp, SplitPaneDown,
    SplitPaneLeft, SplitPaneRight, SplitPaneUp, TogglePaneZoom,
};

use super::{
    ContextMenuKind, PaneDividerDrag, PaneDividerDragView, PaneDrag, ReorderDrag, TabDrag,
    TabDragView, WorkspaceSection, split_gutter,
};

impl super::WorkspaceView {
    fn center_has_splits(&self) -> bool {
        self.snapshot
            .selected_tab()
            .is_some_and(|tab| tab.sessions.len() > 1 && tab.zoomed_session_id.is_none())
    }

    pub(super) fn center_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let canvas = self.terminal_canvas(window, cx).into_any_element();
        div()
            .id("center-panel")
            .flex_1()
            .min_w(px(360.0))
            .h_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(canvas)
    }

    pub(super) fn tab_bar(
        &mut self,
        tabs: Vec<TabSnapshot>,
        selected_tab_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_reorder = tabs.len() > 1;
        let tab_count = tabs.len();
        // A tab is highlighted while its content is on screen, so a review
        // shown beside the terminal highlights both tabs.
        let terminal_hidden = self.review_covers_terminal(cx);
        let review_tab_number = tab_count + 1;
        let show_review_tab = self.diff_view.read(cx).review_expanded();
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
            .justify_start()
            .gap(px(4.0))
            .overflow_x_hidden()
            .children(tabs.into_iter().enumerate().map(|(index, tab)| {
                let tab_id = tab.id;
                let after_tab_id = tab_ids.get(index + 1).copied();
                let tab_order = tab_ids.clone();
                let selected = Some(tab_id) == selected_tab_id && !terminal_hidden;
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
                // The ⌘ hint numbers the first nine tabs; later ones carry it.
                let title = if tab_count > 1 && index >= 9 {
                    format!("{title} {}", index + 1)
                } else {
                    title
                };
                let shortcut = (index < 9).then(|| format!("⌘{}", index + 1));
                let pane_count = tab.sessions.len();
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
                    .h(px(30.0))
                    .min_w(px(0.0))
                    .max_w(px(300.0))
                    .flex_1()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_start()
                    .px_2()
                    .gap(px(6.0))
                    .overflow_hidden()
                    .rounded(px(6.0))
                    .when(can_reorder, |tab| tab.cursor_move())
                    .when(!can_reorder, |tab| tab.cursor_pointer())
                    .bg(if selected {
                        surface_tint(colors().selection, colors().terminal)
                    } else {
                        gpui::rgba(0x00000000)
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
                            // Keep the preview close to the visible tab width.
                            let window_width: f32 = window.bounds().size.width.into();
                            let width =
                                (window_width * (0.52 / drag.tab_count as f32)).clamp(160.0, 300.0);
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
                    .child(agent_compact_badge(
                        identity
                            .as_ref()
                            .and_then(|identity| identity.agent_kind.as_deref()),
                        identity.as_ref().and_then(|identity| identity.agent_state),
                        identity
                            .as_ref()
                            .and_then(|identity| identity.agent_attention),
                        selected,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_start()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_shrink()
                                    .truncate()
                                    .text_size(px(13.0))
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
                                        .text_size(px(10.0))
                                        .text_color(colors().subtle)
                                        .child(format!("{pane_count} panes")),
                                )
                            }),
                    )
                    .when_some(shortcut, |tab, shortcut| {
                        tab.child(
                            div()
                                .flex_none()
                                .font_family(MONO_FONT)
                                .text_size(px(10.0))
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
            .when(show_review_tab, |list| {
                list.child(self.review_tab(review_tab_number, cx))
            })
            .child(
                div()
                    .id("tab-bar-new-tab")
                    .size(px(26.0))
                    .flex_none()
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(colors().subtle)
                    .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
                    .tooltip(|_, cx| super::sidebar_tooltip("Nueva pestaña · ⌘T", cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_terminal_tab_in_project(window, cx);
                    }))
                    .child(svg().path("chrome-icons/plus.svg").size(px(13.0))),
            )
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
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .px(px(8.0))
            .bg(surface_tint(colors().terminal, colors().titlebar))
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
                let framed = self.center_has_splits();
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
                // Split panes get a header: grip to reorder, title, zoom, close.
                let show_header = pane_count > 1;
                let zoomed = self
                    .snapshot
                    .selected_tab()
                    .is_some_and(|tab| tab.zoomed_session_id == Some(session_id));
                let header = show_header
                    .then(|| self.pane_header(session_id, highlighted, zoomed, can_drag, drag, cx));
                div()
                    .id(SharedString::from(format!("pane-{session_id}")))
                    .size_full()
                    .min_w(px(80.0))
                    .min_h(px(48.0))
                    .relative()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .when(terminal.is_none(), |pane| {
                        pane.bg(surface(colors().terminal))
                    })
                    .when(framed, |pane| {
                        pane.border_1().border_color(if highlighted {
                            colors().muted
                        } else {
                            colors().border_subtle
                        })
                    })
                    .when(is_source, |pane| pane.opacity(0.55))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
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
                    .children(header)
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

    fn pane_header(
        &self,
        session_id: Uuid,
        highlighted: bool,
        zoomed: bool,
        can_drag: bool,
        drag: PaneDrag,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let identity = self.pane_identity_by_id(session_id, cx);
        let title = identity
            .as_ref()
            .map(|identity| identity.title.clone())
            .unwrap_or_else(|| "Terminal".to_owned());
        let button = |id: String, icon: &'static str, label: &'static str| {
            div()
                .id(SharedString::from(id))
                .size(px(22.0))
                .flex_none()
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(colors().subtle)
                .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
                .tooltip(move |_, cx| super::sidebar_tooltip(label, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(svg().path(icon).size(px(12.0)))
        };
        div()
            .id(SharedString::from(format!("pane-header-{session_id}")))
            .h(px(30.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .pl_1()
            .pr_1()
            .border_b_1()
            .border_color(colors().border_subtle)
            .bg(surface(colors().terminal))
            .text_color(if highlighted {
                colors().foreground
            } else {
                colors().muted
            })
            .when(can_drag, |header| {
                header
                    .cursor_move()
                    .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.preview.clone()))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if can_drag {
                        this.reorder_drag = Some(ReorderDrag::Pane(session_id));
                    }
                    if event.click_count == 2 {
                        this.toggle_pane_zoom_for(session_id, window, cx);
                    }
                }),
            )
            .child(
                div()
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(colors().subtle)
                    .when(!can_drag, |grip| grip.opacity(0.4))
                    .child(svg().path("chrome-icons/grip.svg").size(px(14.0))),
            )
            .child(agent_compact_badge(
                identity
                    .as_ref()
                    .and_then(|identity| identity.agent_kind.as_deref()),
                identity.as_ref().and_then(|identity| identity.agent_state),
                identity
                    .as_ref()
                    .and_then(|identity| identity.agent_attention),
                highlighted,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(12.5))
                    .font_weight(if highlighted {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .child(title),
            )
            .child(
                button(
                    format!("pane-zoom-{session_id}"),
                    if zoomed {
                        "chrome-icons/minimize.svg"
                    } else {
                        "chrome-icons/maximize.svg"
                    },
                    if zoomed {
                        "Restaurar pane · ⇧⌘↵"
                    } else {
                        "Agrandar pane · ⇧⌘↵"
                    },
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_pane_zoom_for(session_id, window, cx);
                })),
            )
            .child(
                button(
                    format!("pane-close-{session_id}"),
                    "chrome-icons/close.svg",
                    "Cerrar pane",
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_pane(session_id, window, cx);
                })),
            )
            .into_any_element()
    }

    /// Enlarges a pane to fill its tab, or restores the split.
    pub(super) fn toggle_pane_zoom_for(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.select_terminal_global(session_id)
            && self.snapshot.toggle_selected_pane_zoom()
        {
            self.sync_terminal_surface_visibility(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
            cx.notify();
        }
    }

    pub(super) fn close_pane(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.close_terminal(session_id) {
            self.agent_names.remove(&session_id);
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
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
        // Terminals share the continuous workspace surface; split panes keep
        // thin boundaries so focus and resize targets remain legible.
        div()
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .when(is_empty, |canvas| canvas.bg(surface(colors().terminal)))
            .when_some(panes, |canvas, panes| canvas.child(panes))
            .when(is_empty, |canvas| {
                canvas.child(self.empty_project_content(cx))
            })
    }

    /// Pane commands act on what the user sees: a full-tab review steps
    /// aside for the terminal first.
    fn reveal_terminal(&mut self, cx: &mut Context<Self>) {
        if self.review_covers_terminal(cx) {
            self.review_tab_active = false;
            self.sync_terminal_surface_visibility(cx);
        }
    }

    pub(super) fn split_pane(
        &mut self,
        direction: PaneSplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_section(WorkspaceSection::Workspace, window, cx);
        self.reveal_terminal(cx);
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
        self.select_section(WorkspaceSection::Workspace, window, cx);
        self.reveal_terminal(cx);
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
        self.select_section(WorkspaceSection::Workspace, window, cx);
        self.reveal_terminal(cx);
        if self.snapshot.cycle_terminal(offset) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    pub(super) fn resize_pane(&mut self, direction: PaneResizeDirection, cx: &mut Context<Self>) {
        if self.workspace_section != WorkspaceSection::Workspace {
            return;
        }
        self.reveal_terminal(cx);
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
        if self.workspace_section != WorkspaceSection::Workspace {
            return;
        }
        self.reveal_terminal(cx);
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
        self.select_section(WorkspaceSection::Workspace, window, cx);
        if let Some(session) = self.snapshot.selected_session().map(|item| item.id) {
            self.reveal_terminal(cx);
            self.toggle_pane_zoom_for(session, window, cx);
        }
    }
}
