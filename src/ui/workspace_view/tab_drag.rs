//! Stable drop targets shared by terminal tabs, review tabs and split panes.

use gpui::{
    AnyElement, App, Bounds, Context, DragMoveEvent, MouseButton, Pixels, Point, SharedString,
    Window, div, prelude::*, px, relative, svg,
};
use uuid::Uuid;

use crate::domain::workspace::{PaneSplitDirection, WorkspaceTabId};
use crate::ui::theme::colors;

use super::panes::{TAB_HEIGHT, TAB_MAX_WIDTH, TAB_RADIUS};
use super::{PaneDrag, ReorderDrag, ReorderSlot, TabDrag, TabStripDrop, WorkspaceView};

impl WorkspaceView {
    /// The strip as it will look after the drop. The dragged tab leaves a gap
    /// that follows the pointer, and the other tabs slide around it.
    pub(super) fn tab_strip_slots(&self, cx: &App) -> Vec<ReorderSlot<WorkspaceTabId>> {
        let order = self.visible_tab_order(cx);
        if !cx.has_active_drag() {
            return order.into_iter().map(ReorderSlot::Item).collect();
        }
        let source = self.strip_drag_source(&order);
        let landing = match self.reorder_drag {
            Some(ReorderDrag::Tab(_) | ReorderDrag::Pane(_)) => {
                self.tab_strip_drop.map(|drop| drop.before)
            }
            _ => None,
        };
        super::reorder_slots(&order, source, landing)
    }

    /// The dragged tab when it already has a place in the strip.
    fn strip_drag_source(&self, order: &[WorkspaceTabId]) -> Option<WorkspaceTabId> {
        match self.reorder_drag {
            Some(ReorderDrag::Tab(source)) if order.contains(&source) => Some(source),
            _ => None,
        }
    }

    /// Tracks the pointer over one tab while dragging. The outer quarters move
    /// the gap beside the tab, which slides it aside; the middle docks into it.
    pub(super) fn tab_drag_zone(
        &self,
        target: WorkspaceTabId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !cx.has_active_drag() {
            return None;
        }
        let merging = self
            .tab_strip_drop
            .is_some_and(|drop| drop.merge == Some(target));
        Some(
            div()
                .id(SharedString::from(format!("tab-drop-{target:?}")))
                .absolute()
                .inset_0()
                .on_drag_move(
                    cx.listener(move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                        let drag = event.drag(cx);
                        let can_merge = !drag.from_pane && drag.tab_id != target;
                        this.hover_tab(target, event.bounds, event.event.position, can_merge, cx);
                    }),
                )
                .on_drag_move(
                    cx.listener(move |this, event: &DragMoveEvent<PaneDrag>, _, cx| {
                        this.hover_tab(target, event.bounds, event.event.position, false, cx);
                    }),
                )
                .when(merging, |zone| zone.child(merge_preview()))
                .into_any_element(),
        )
    }

    fn hover_tab(
        &mut self,
        target: WorkspaceTabId,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        can_merge: bool,
        cx: &mut Context<Self>,
    ) {
        if !bounds.contains(&position) {
            return;
        }
        let x = f32::from(position.x - bounds.left()) / f32::from(bounds.size.width).max(1.0);
        let order = self.visible_tab_order(cx);
        let source = self.strip_drag_source(&order);
        let next = if can_merge && (0.25..=0.75).contains(&x) {
            // Keep the gap where it is so the strip does not shift under the pointer.
            let before = self
                .tab_strip_drop
                .map(|drop| drop.before)
                .unwrap_or_else(|| {
                    source.and_then(|source| super::landing_beside(&order, None, source, true))
                });
            TabStripDrop {
                before,
                merge: Some(target),
            }
        } else {
            TabStripDrop {
                before: super::landing_beside(&order, source, target, x > 0.5),
                merge: None,
            }
        };
        self.set_tab_strip_drop(Some(next), cx);
    }

    fn set_tab_strip_drop(&mut self, drop: Option<TabStripDrop>, cx: &mut Context<Self>) {
        if self.tab_strip_drop != drop {
            self.tab_strip_drop = drop;
            cx.notify();
        }
    }

    /// Where the dragged tab will land. Hovering it keeps the current landing.
    pub(super) fn tab_strip_gap(&self, cx: &mut Context<Self>) -> AnyElement {
        super::reorder_gap()
            .id("tab-drop-gap")
            .h(px(TAB_HEIGHT))
            .min_w(px(0.0))
            .max_w(px(TAB_MAX_WIDTH))
            .flex_1()
            .rounded(px(TAB_RADIUS))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<TabDrag>, _, cx| {
                this.hover_tab_gap(event.bounds, event.event.position, cx);
            }))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<PaneDrag>, _, cx| {
                this.hover_tab_gap(event.bounds, event.event.position, cx);
            }))
            .into_any_element()
    }

    fn hover_tab_gap(
        &mut self,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if bounds.contains(&position) {
            let drop = self.tab_strip_drop.map(|drop| TabStripDrop {
                merge: None,
                ..drop
            });
            self.set_tab_strip_drop(drop, cx);
        }
    }

    /// The whole strip accepts the drop and forgets the landing once the
    /// pointer leaves it, so the dragged tab returns to its place.
    pub(super) fn tab_strip_drop_area<E: InteractiveElement>(
        &self,
        strip: E,
        cx: &mut Context<Self>,
    ) -> E {
        strip
            .can_drop(|value, _, _| {
                value.downcast_ref::<TabDrag>().is_some()
                    || value.downcast_ref::<PaneDrag>().is_some()
            })
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<TabDrag>, _, cx| {
                if !event.bounds.contains(&event.event.position) {
                    this.set_tab_strip_drop(None, cx);
                }
            }))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<PaneDrag>, _, cx| {
                if !event.bounds.contains(&event.event.position) {
                    this.set_tab_strip_drop(None, cx);
                }
            }))
            .on_drop(cx.listener(
                |this, drag: &TabDrag, window, cx| match this.tab_strip_drop.take() {
                    Some(TabStripDrop {
                        merge: Some(target),
                        ..
                    }) => this.dock_tab(drag.tab_id, target, window, cx),
                    Some(drop) => this.drop_tab_in_strip(drag, drop.before, window, cx),
                    None => {}
                },
            ))
            .on_drop(cx.listener(|this, drag: &PaneDrag, window, cx| {
                if let Some(drop) = this.tab_strip_drop.take() {
                    this.detach_pane(drag.session_id, drop.before, window, cx);
                }
            }))
    }

    pub(super) fn drop_tab_in_strip(
        &mut self,
        drag: &TabDrag,
        before: Option<WorkspaceTabId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reorder_drag = None;
        self.pane_drop_preview = None;
        if drag.from_pane && drag.tab_id == WorkspaceTabId::Review {
            self.review_docked_tab_id = None;
            self.diff_view
                .update(cx, |diff, cx| diff.set_review_focused(true, cx));
            self.activate_review_tab(window, cx);
        }
        let review_open = self.review_has_tab(cx);
        if self
            .snapshot
            .move_workspace_tab(drag.tab_id, before, review_open)
        {
            self.persist(cx);
        }
        cx.notify();
    }

    pub(super) fn detach_pane(
        &mut self,
        session_id: Uuid,
        before: Option<WorkspaceTabId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reorder_drag = None;
        self.pane_drop_preview = None;
        let removes_terminal_from_review = self.review_dock_owner().is_some_and(|owner| {
            self.snapshot.selected_workspace().is_some_and(|workspace| {
                workspace.tabs.iter().any(|tab| {
                    tab.id == owner
                        && tab.sessions.len() == 1
                        && tab.layout.contains_terminal(session_id)
                })
            })
        });
        if let Some(tab_id) = self.snapshot.detach_terminal_to_tab(session_id) {
            if removes_terminal_from_review {
                self.review_docked_tab_id = None;
                self.diff_view
                    .update(cx, |diff, cx| diff.set_review_focused(true, cx));
            }
            let review_open = self.review_has_tab(cx);
            self.snapshot
                .move_workspace_tab(WorkspaceTabId::Terminal(tab_id), before, review_open);
            self.show_terminal_tab(window, cx);
        }
        cx.notify();
    }

    pub(super) fn dock_tab(
        &mut self,
        source: WorkspaceTabId,
        target: WorkspaceTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if source == target {
            return;
        }
        match (source, target) {
            (WorkspaceTabId::Terminal(source), WorkspaceTabId::Terminal(target)) => {
                let target_session = self
                    .snapshot
                    .selected_workspace()
                    .and_then(|workspace| workspace.tabs.iter().find(|tab| tab.id == target))
                    .and_then(|tab| tab.selected_session_id);
                if let Some(target_session) = target_session {
                    self.dock_tab_at_pane(
                        WorkspaceTabId::Terminal(source),
                        target_session,
                        PaneSplitDirection::Right,
                        window,
                        cx,
                    );
                }
            }
            (WorkspaceTabId::Review, WorkspaceTabId::Terminal(terminal)) => {
                self.dock_review(terminal, PaneSplitDirection::Right, window, cx);
            }
            (WorkspaceTabId::Terminal(terminal), WorkspaceTabId::Review) => {
                self.dock_review(terminal, PaneSplitDirection::Left, window, cx);
            }
            _ => {}
        }
    }

    fn dock_review(
        &mut self,
        terminal: Uuid,
        direction: PaneSplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reorder_drag = None;
        self.pane_drop_preview = None;
        if self.diff_view.read(cx).review_expanded() && self.snapshot.select_tab(terminal) {
            self.review_docked_tab_id = Some(terminal);
            self.review_split_direction = direction;
            self.diff_view
                .update(cx, |diff, cx| diff.set_review_focused(false, cx));
            self.activate_review_tab(window, cx);
            self.persist(cx);
        }
        cx.notify();
    }

    pub(super) fn dock_tab_at_pane(
        &mut self,
        source: WorkspaceTabId,
        target_session: Uuid,
        direction: PaneSplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reorder_drag = None;
        self.pane_drop_preview = None;
        match source {
            WorkspaceTabId::Terminal(source) => {
                let carries_review = self.review_dock_owner() == Some(source);
                if self
                    .snapshot
                    .merge_tab_into_pane(source, target_session, direction)
                {
                    if carries_review {
                        self.review_docked_tab_id = self.snapshot.selected_tab().map(|tab| tab.id);
                    }
                    self.show_terminal_tab(window, cx);
                }
            }
            WorkspaceTabId::Review => {
                let target = self
                    .snapshot
                    .selected_workspace()
                    .and_then(|workspace| {
                        workspace
                            .tabs
                            .iter()
                            .find(|tab| tab.layout.contains_terminal(target_session))
                    })
                    .map(|tab| tab.id);
                if let Some(target) = target {
                    self.dock_review(target, direction, window, cx);
                }
            }
        }
        cx.notify();
    }

    /// One stable target, with a separate preview of the resulting half pane.
    pub(super) fn pane_tab_drop_targets(
        &self,
        session_id: Uuid,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab_id = self
            .snapshot
            .selected_tab()
            .map(|tab| WorkspaceTabId::Terminal(tab.id));
        let preview = self
            .pane_drop_preview
            .filter(|(target, _)| *target == session_id && cx.has_active_drag())
            .map(|(_, direction)| direction);
        div()
            .id(SharedString::from(format!("pane-tab-drop-{session_id}")))
            .absolute()
            .inset_0()
            .can_drop(move |value, _, _| {
                value
                    .downcast_ref::<TabDrag>()
                    .is_some_and(|drag| Some(drag.tab_id) != tab_id)
            })
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                    let valid = Some(event.drag(cx).tab_id) != tab_id;
                    if valid && event.bounds.contains(&event.event.position) {
                        let x = f32::from(event.event.position.x - event.bounds.left())
                            / f32::from(event.bounds.size.width).max(1.0);
                        let y = f32::from(event.event.position.y - event.bounds.top())
                            / f32::from(event.bounds.size.height).max(1.0);
                        let direction = if x < 0.25 {
                            PaneSplitDirection::Left
                        } else if x > 0.75 {
                            PaneSplitDirection::Right
                        } else if y < 0.5 {
                            PaneSplitDirection::Up
                        } else {
                            PaneSplitDirection::Down
                        };
                        let next = Some((session_id, direction));
                        if this.pane_drop_preview != next {
                            this.pane_drop_preview = next;
                            cx.notify();
                        }
                    } else if this
                        .pane_drop_preview
                        .is_some_and(|(target, _)| target == session_id)
                    {
                        this.pane_drop_preview = None;
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                let direction = this
                    .pane_drop_preview
                    .filter(|(target, _)| *target == session_id)
                    .map_or(PaneSplitDirection::Right, |(_, direction)| direction);
                this.dock_tab_at_pane(drag.tab_id, session_id, direction, window, cx);
            }))
            .when_some(preview, |target, direction| {
                target.child(split_drop_preview(direction))
            })
            .into_any_element()
    }

    pub(super) fn tab_strip_end_target(&self, cx: &mut Context<Self>) -> AnyElement {
        let dragging = cx.has_active_drag()
            && matches!(
                self.reorder_drag,
                Some(ReorderDrag::Tab(_) | ReorderDrag::Pane(_))
            );
        div()
            .id("tab-drop-end")
            .h_full()
            .min_w(px(54.0))
            .flex_1()
            .flex()
            .items_center()
            .pl(px(4.0))
            .when(!dragging, |zone| {
                zone.child(
                    super::titlebar::titlebar_button("tab-bar-new-tab", true)
                        .tooltip(|_, cx| super::sidebar_tooltip("New tab · ⌘T", cx))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_terminal_tab_in_project(window, cx);
                        }))
                        .child(super::titlebar::titlebar_icon("chrome-icons/plus.svg")),
                )
            })
            .when(dragging, |zone| {
                zone.on_drag_move(cx.listener(|this, event: &DragMoveEvent<TabDrag>, _, cx| {
                    this.hover_tab_strip_end(event.bounds, event.event.position, cx);
                }))
                .on_drag_move(cx.listener(
                    |this, event: &DragMoveEvent<PaneDrag>, _, cx| {
                        this.hover_tab_strip_end(event.bounds, event.event.position, cx);
                    },
                ))
            })
            .into_any_element()
    }

    fn hover_tab_strip_end(
        &mut self,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if bounds.contains(&position) {
            self.set_tab_strip_drop(
                Some(TabStripDrop {
                    before: None,
                    merge: None,
                }),
                cx,
            );
        }
    }
}

/// Covers a tab that the dragged one will dock into as a split.
fn merge_preview() -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(TAB_RADIUS))
        .border_1()
        .border_color(colors().accent)
        .bg(gpui::Hsla::from(colors().accent).opacity(0.14))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .px(px(6.0))
                .py(px(3.0))
                .rounded(px(5.0))
                .bg(colors().elevated)
                .text_size(px(11.0))
                .text_color(colors().accent)
                .child(
                    svg()
                        .path("chrome-icons/split-view.svg")
                        .size(px(12.0))
                        .flex_none(),
                )
                .child("Split"),
        )
        .into_any_element()
}

fn split_drop_preview(direction: PaneSplitDirection) -> AnyElement {
    let (x, y, width, height, label) = match direction {
        PaneSplitDirection::Left => (0.0, 0.0, 0.5, 1.0, "Split left"),
        PaneSplitDirection::Right => (0.5, 0.0, 0.5, 1.0, "Split right"),
        PaneSplitDirection::Up => (0.0, 0.0, 1.0, 0.5, "Split up"),
        PaneSplitDirection::Down => (0.0, 0.5, 1.0, 0.5, "Split down"),
    };
    div()
        .absolute()
        .left(relative(x))
        .top(relative(y))
        .w(relative(width))
        .h(relative(height))
        .p(px(6.0))
        .child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.0))
                .border_1()
                .border_color(gpui::Hsla::from(colors().accent).opacity(0.65))
                .bg(gpui::Hsla::from(colors().accent).opacity(0.10))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(gpui::Hsla::from(colors().accent).opacity(0.3))
                        .bg(colors().elevated)
                        .shadow_sm()
                        .text_size(px(11.0))
                        .text_color(colors().foreground)
                        .child(
                            svg()
                                .path("chrome-icons/split-view.svg")
                                .size(px(15.0))
                                .text_color(colors().accent),
                        )
                        .child(label),
                ),
        )
        .into_any_element()
}
