//! Split pane layout, tab strip, and pane keyboard/mouse actions.

use gpui::{
    AnyElement, Context, DragMoveEvent, Focusable, MouseButton, MouseDownEvent, MouseUpEvent,
    SharedString, Window, WindowControlArea, div, prelude::*, px, relative, svg,
};
use uuid::Uuid;

use crate::domain::workspace::{
    PaneBranch, PaneFocusDirection, PaneLayoutSnapshot, PaneResizeDirection, PaneSplitDirection,
    WorkspaceSplitAxis, WorkspaceTabId,
};
use crate::ui::agent_marks::agent_compact_badge;
use crate::ui::terminal::TerminalDragPreview;
use crate::ui::theme::{MONO_FONT, colors, surface, surface_tint};
use crate::{EqualizePanes, TogglePaneZoom};

use super::{
    ContextMenuKind, DragGhost, PaneDividerDrag, PaneDividerDragView, PaneDrag, ReorderDrag,
    ReorderSlot, TabDrag, WorkspaceSection, split_gutter,
};

pub(super) const TAB_HEIGHT: f32 = 30.0;
pub(super) const TAB_MAX_WIDTH: f32 = 240.0;
pub(super) const TAB_RADIUS: f32 = 8.0;
pub(super) const TAB_TEXT_SIZE: f32 = 14.0;
const TAB_GAP: f32 = 2.0;

/// A tab as it looks while selected. Close and shortcut affordances stay behind
/// because they cannot be used mid-drag.
pub(super) fn tab_drag_ghost(leading: impl IntoElement, title: String) -> AnyElement {
    div()
        .h(px(TAB_HEIGHT))
        .pl(px(10.0))
        .pr(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .overflow_hidden()
        .rounded(px(TAB_RADIUS))
        .bg(surface_tint(colors().selection, colors().terminal))
        .text_color(colors().foreground)
        .child(leading)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(TAB_TEXT_SIZE))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title),
        )
        .into_any_element()
}

/// `⌘N` hint, revealed while the tab is hovered.
pub(super) fn tab_shortcut(shortcut: String) -> gpui::Div {
    div()
        .flex_none()
        .opacity(0.0)
        .group_hover("title-tab", |hint| hint.opacity(1.0))
        .font_family(MONO_FONT)
        .text_size(px(10.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(colors().subtle)
        .child(shortcut)
}

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

    pub(super) fn tab_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // A docked review belongs to its terminal tab; only standalone reviews
        // occupy a separate position in the strip.
        let terminal_hidden = self.review_covers_terminal(cx);
        let tab_ids = self.visible_tab_order(cx);
        let tab_count = tab_ids.len();
        let slots = self.tab_strip_slots(cx);
        self.tab_motion
            .update(&slots, self.tab_width.get() + px(TAB_GAP));
        let workspace = self.snapshot.selected_workspace();
        let tabs = workspace
            .map(|workspace| workspace.tabs.as_slice())
            .unwrap_or_default();
        let selected_tab_id = workspace.and_then(|workspace| workspace.selected_tab_id);
        let tab_list = div()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .justify_start()
            .gap(px(TAB_GAP))
            .overflow_x_hidden()
            .children(slots.into_iter().map(|slot| {
                let ReorderSlot::Item(item) = slot else {
                    return self.tab_strip_gap(cx);
                };
                // Labels keep their current position until the drop.
                let index = tab_ids.iter().position(|id| *id == item).unwrap_or(0);
                let WorkspaceTabId::Terminal(tab_id) = item else {
                    return self.review_tab(index, tab_count, cx);
                };
                let slide = self.tab_motion.slide(item);
                let tab = tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .expect("tab in strip");
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
                // Later tabs keep their position in the label.
                let title = if tab_count > 1 && index >= 9 {
                    format!("{title} {}", index + 1)
                } else {
                    title
                };
                let shortcut = super::tabs::tab_shortcut_label(index, tab_count);
                let pane_count =
                    tab.sessions.len() + usize::from(self.review_dock_owner() == Some(tab_id));
                let drag = TabDrag {
                    tab_id: item,
                    title: title.clone(),
                    from_pane: false,
                };
                let ghost = {
                    let title = title.clone();
                    let agent_kind = identity
                        .as_ref()
                        .and_then(|identity| identity.agent_kind.clone());
                    let agent_state = identity.as_ref().and_then(|identity| identity.agent_state);
                    let agent_attention = identity
                        .as_ref()
                        .and_then(|identity| identity.agent_attention);
                    DragGhost::new(TAB_RADIUS, colors().terminal, move || {
                        tab_drag_ghost(
                            agent_compact_badge(
                                agent_kind.as_deref(),
                                agent_state,
                                agent_attention,
                                true,
                            ),
                            title.clone(),
                        )
                    })
                };
                div()
                    .id(SharedString::from(format!("tab-{tab_id}")))
                    .group("title-tab")
                    .h(px(TAB_HEIGHT))
                    .min_w(px(0.0))
                    .max_w(px(TAB_MAX_WIDTH))
                    .flex_1()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_start()
                    .pl(px(10.0))
                    .pr(px(8.0))
                    .gap(px(8.0))
                    .overflow_hidden()
                    .rounded(px(TAB_RADIUS))
                    .cursor_move()
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.reorder_drag = Some(ReorderDrag::Tab(item));
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
                    .child(ghost.measure())
                    .child(super::measure_width(self.tab_width.clone()))
                    .on_drag(drag, move |_, _, _, cx| ghost.preview(cx))
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
                                    .text_size(px(TAB_TEXT_SIZE))
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
                                        .h(px(16.0))
                                        .px(px(5.0))
                                        .rounded(px(4.0))
                                        .flex()
                                        .items_center()
                                        .bg(surface_tint(colors().hover, colors().terminal))
                                        .font_family(MONO_FONT)
                                        .text_size(px(10.0))
                                        .text_color(colors().subtle)
                                        .child(format!("{pane_count} panes")),
                                )
                            }),
                    )
                    .when_some(shortcut, |tab, shortcut| tab.child(tab_shortcut(shortcut)))
                    .children(self.tab_drag_zone(item, cx))
                    .child(
                        div()
                            .id(SharedString::from(format!("close-tab-{tab_id}")))
                            .group("tab-close")
                            .size(px(20.0))
                            .flex_none()
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .opacity(if selected { 1.0 } else { 0.0 })
                            .group_hover("title-tab", |button| button.opacity(1.0))
                            .hover(|button| button.bg(colors().hover))
                            .tooltip(|_, cx| super::sidebar_tooltip("Close tab", cx))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_tab(tab_id, window, cx);
                                cx.stop_propagation();
                            }))
                            .child(
                                svg()
                                    .path("chrome-icons/close.svg")
                                    .size(px(12.0))
                                    .text_color(colors().muted)
                                    .group_hover("tab-close", |icon| {
                                        icon.text_color(colors().foreground)
                                    }),
                            ),
                    )
                    .map(|tab| super::slide_into_place(tab, slide, "tab", false))
            }))
            .child(self.tab_strip_end_target(cx));
        let tab_list = self.tab_strip_drop_area(tab_list, cx);

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
                let review_split = self.review_visible(cx) && !self.review_covers_terminal(cx);
                let can_drag = pane_count > 1 || review_split;
                let dragging_pane = match self.reorder_drag {
                    Some(ReorderDrag::Pane(id)) if cx.has_active_drag() => Some(id),
                    _ => None,
                };
                let is_source = dragging_pane == Some(session_id);
                let drag = PaneDrag {
                    session_id,
                    title: self
                        .pane_identity_by_id(session_id, cx)
                        .map(|identity| identity.title)
                        .unwrap_or_else(|| "Terminal".into()),
                    preview: drag_preview,
                };
                // Split panes get a header: grip to reorder, title, zoom, close.
                let show_header = can_drag;
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
                            crate::ui::theme::mix(colors().border_subtle, colors().muted, 0.28)
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
                    .child(self.pane_tab_drop_targets(session_id, cx))
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
                .group("pane-action")
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
                .child(
                    svg()
                        .path(icon)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(colors().muted)
                        .group_hover("pane-action", |icon| icon.text_color(colors().foreground)),
                )
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
                    .on_drag(drag, |drag, offset, _, cx| drag.preview(offset, cx))
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
                    .child(
                        svg()
                            .path("chrome-icons/grip.svg")
                            .size(px(14.0))
                            .flex_none()
                            .text_color(colors().subtle),
                    ),
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
                        "Restore pane · ⇧⌘↵"
                    } else {
                        "Zoom pane · ⇧⌘↵"
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
                    "Close pane",
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
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    pub(super) fn close_tab(&mut self, tab_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let review_in_front = self.review_covers_terminal(cx);
        let closing_selected = self
            .snapshot
            .selected_tab()
            .is_some_and(|tab| tab.id == tab_id);
        if self.snapshot.close_tab(tab_id) {
            self.reconcile_terminal_views(cx);
            if closing_selected && !review_in_front {
                self.show_terminal_tab(window, cx);
            } else {
                self.persist(cx);
            }
            cx.notify();
        }
    }

    pub(super) fn terminal_canvas(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let layout = self.snapshot.selected_tab().map(|tab| {
            tab.zoomed_session_id
                .map_or_else(|| tab.layout.clone(), PaneLayoutSnapshot::terminal)
        });
        let panes = layout
            .as_ref()
            .map(|layout| self.render_pane_layout(layout, Vec::new(), window, cx));
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
            self.sync_git_panel_visibility(cx);
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
            self.terminal_selection_changed(cx);
        }
        self.focus_selected_terminal(window, cx);
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
            self.terminal_selection_changed(cx);
        }
        self.focus_selected_terminal(window, cx);
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
        self.pane_drop_preview = None;
        self.tab_strip_drop = None;
        self.project_drop = None;
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
            self.terminal_selection_changed(cx);
            self.focus_terminal(from, window, cx);
        }
        cx.notify();
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
