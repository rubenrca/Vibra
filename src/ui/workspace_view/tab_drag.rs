//! Stable drop targets shared by terminal tabs, review tabs and split panes.

use gpui::{
    AnyElement, Context, DragMoveEvent, SharedString, Window, div, prelude::*, px, relative, svg,
};
use uuid::Uuid;

use crate::domain::workspace::{PaneSplitDirection, WorkspaceTabId};
use crate::ui::theme::colors;

use super::{PaneDrag, ReorderDrag, TabDrag, WorkspaceView};

impl WorkspaceView {
    /// Edges insert into the strip, while the middle docks a tab as a split.
    /// A pane dropped anywhere in the strip becomes an independent tab.
    pub(super) fn tab_drop_targets(
        &self,
        target: WorkspaceTabId,
        after: Option<WorkspaceTabId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detaching = matches!(self.reorder_drag, Some(ReorderDrag::Pane(_)))
            || (self.reorder_drag == Some(ReorderDrag::Tab(WorkspaceTabId::Review))
                && self.review_dock_owner().is_some());
        let accepts_drag = self.reorder_drag != Some(ReorderDrag::Tab(target)) || detaching;
        div()
            .absolute()
            .inset_0()
            .flex()
            .children((0..3).map(|zone| {
                let before = if zone == 2 { after } else { Some(target) };
                div()
                    .id(SharedString::from(format!("tab-drop-{target:?}-{zone}")))
                    .group("tab-drop-zone")
                    .relative()
                    .h_full()
                    .w(relative(if zone == 1 { 0.5 } else { 0.25 }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .can_drop(move |value, _, _| {
                        value.downcast_ref::<PaneDrag>().is_some()
                            || value
                                .downcast_ref::<TabDrag>()
                                .is_some_and(|drag| drag.tab_id != target || drag.from_pane)
                    })
                    .drag_over::<TabDrag>(move |style, drag, _, _| {
                        let style = style.border_color(colors().accent);
                        match zone {
                            0 => style.border_l_2(),
                            2 => style.border_r_2(),
                            _ if drag.from_pane => style.border_l_2(),
                            _ => style
                                .rounded(px(5.0))
                                .border_1()
                                .bg(gpui::Hsla::from(colors().accent).opacity(0.14)),
                        }
                    })
                    .drag_over::<PaneDrag>(move |style, _, _, _| {
                        let style = style.border_color(colors().accent);
                        if zone == 2 {
                            style.border_r_2()
                        } else {
                            style.border_l_2()
                        }
                    })
                    .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                        if zone == 1 && !drag.from_pane {
                            this.dock_tab(drag.tab_id, target, window, cx);
                        } else {
                            this.drop_tab_in_strip(drag, before, window, cx);
                        }
                    }))
                    .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                        this.detach_pane(drag.session_id, before, window, cx);
                    }))
                    .when(zone == 1 && accepts_drag, |zone| {
                        zone.child(
                            div()
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .px(px(5.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .bg(colors().elevated)
                                .text_color(colors().accent)
                                .opacity(0.0)
                                .group_drag_over::<TabDrag>("tab-drop-zone", |style| {
                                    style.opacity(1.0)
                                })
                                .group_drag_over::<PaneDrag>("tab-drop-zone", |style| {
                                    style.opacity(1.0)
                                })
                                .child(
                                    svg()
                                        .path(if detaching {
                                            "chrome-icons/plus.svg"
                                        } else {
                                            "chrome-icons/split-view.svg"
                                        })
                                        .size(px(12.0))
                                        .flex_none(),
                                )
                                .child(div().truncate().text_size(px(10.0)).child(if detaching {
                                    "Tab"
                                } else {
                                    "Split"
                                })),
                        )
                    })
            }))
            .into_any_element()
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
        div()
            .id("tab-drop-end")
            .h_full()
            .min_w(px(24.0))
            .flex_1()
            .can_drop(|value, _, _| {
                value.downcast_ref::<TabDrag>().is_some()
                    || value.downcast_ref::<PaneDrag>().is_some()
            })
            .drag_over::<TabDrag>(|style, _, _, _| style.border_l_2().border_color(colors().accent))
            .drag_over::<PaneDrag>(|style, _, _, _| {
                style.border_l_2().border_color(colors().accent)
            })
            .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                this.drop_tab_in_strip(drag, None, window, cx)
            }))
            .on_drop(cx.listener(|this, drag: &PaneDrag, window, cx| {
                this.detach_pane(drag.session_id, None, window, cx)
            }))
            .into_any_element()
    }
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
