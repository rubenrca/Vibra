//! Split pane layout, tab strip, and pane keyboard/mouse actions.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, Context, DragMoveEvent, Focusable, Hsla, MouseButton,
    MouseDownEvent, MouseUpEvent, SharedString, Window, WindowControlArea, div, prelude::*, px,
    relative, svg,
};
use uuid::Uuid;

use crate::domain::workspace::{
    PaneBranch, PaneFocusDirection, PaneLayoutSnapshot, PaneResizeDirection, PaneSplitDirection,
    WorkspaceSplitAxis, WorkspaceTabId,
};
use crate::ui::agent_marks::{agent_compact_badge, agent_tile_stack, agent_tile_stack_shuffle};
use crate::ui::tab_style::{self, lifted_tab};
use crate::ui::terminal::TerminalDragPreview;
use crate::ui::theme::{MONO_FONT, colors, frame, lift, mix, surface_tint};
use crate::{EqualizePanes, TogglePaneZoom};

use super::{
    CARD_RADIUS, ContextMenuKind, DragGhost, PaneDividerDrag, PaneDividerDragView, PaneDrag,
    ReorderDrag, ReorderSlot, TabDrag, WorkspaceSection, card, split_gutter,
};

pub(super) const TAB_HEIGHT: f32 = 30.0;
pub(super) const TAB_MAX_WIDTH: f32 = 240.0;
pub(super) use crate::ui::tab_style::TAB_RADIUS;
pub(super) const TAB_TEXT_SIZE: f32 = 13.0;
/// The leading tile sits as far from the tab's top, bottom, and left edge.
pub(super) const TAB_TILE_SIZE: f32 = 20.0;
pub(super) const TAB_TILE_INSET: f32 = (TAB_HEIGHT - TAB_TILE_SIZE) / 2.0;
const TAB_GAP: f32 = 4.0;
/// Pane headers sit inside their card, so they need no divider of their own.
pub(super) const PANE_HEADER_HEIGHT: f32 = 30.0;

/// A titlebar tab on the frame, its leading tile inset as far from the left
/// edge as from the top and bottom.
pub(super) fn titlebar_tab_style(
    tab: gpui::Stateful<gpui::Div>,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    lifted_tab(tab, selected, frame(), TAB_HEIGHT)
        .pl(px(TAB_TILE_INSET))
        .pr(px(6.0))
        .gap(px(8.0))
}

/// A tab as it looks while selected. Close and shortcut affordances stay behind
/// because they cannot be used mid-drag.
pub(super) fn tab_drag_ghost(leading: impl IntoElement, title: String) -> AnyElement {
    div()
        .h(px(TAB_HEIGHT))
        .pl(px(TAB_TILE_INSET))
        .pr(px(10.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .overflow_hidden()
        .rounded(px(TAB_RADIUS))
        .bg(tab_style::selected_fill(frame()))
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

/// A quiet icon action in a card header; `card_fill` is the card it sits on.
pub(super) fn card_header_button(
    id: impl Into<SharedString>,
    icon: &'static str,
    label: &'static str,
    card_fill: gpui::Rgba,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .group("card-action")
        .size(px(20.0))
        .flex_none()
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |button| button.bg(surface_tint(colors().hover, card_fill)))
        .tooltip(move |_, cx| super::sidebar_tooltip(label, cx))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            svg()
                .path(icon)
                .size(px(13.0))
                .flex_none()
                .text_color(colors().subtle)
                .group_hover("card-action", |icon| icon.text_color(colors().foreground)),
        )
}

/// The control at a tab's end: the selected tab's menu, or a close button
/// that appears while another tab is hovered.
pub(super) fn tab_trailing_button(
    id: impl Into<SharedString>,
    icon: &'static str,
    label: &'static str,
    visible: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .group("tab-trailing")
        .size(px(20.0))
        .flex_none()
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .opacity(if visible { 1.0 } else { 0.0 })
        .group_hover("title-tab", |button| button.opacity(1.0))
        .hover(|button| button.bg(Hsla::from(colors().foreground).opacity(0.08)))
        .tooltip(move |_, cx| super::sidebar_tooltip(label, cx))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            svg()
                .path(icon)
                .size(px(14.0))
                .text_color(colors().subtle)
                .group_hover("tab-trailing", |icon| icon.text_color(colors().foreground)),
        )
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

/// How long a tab takes to bring a newly focused pane to its front.
const TAB_FRONT_SWAP: Duration = Duration::from_millis(320);
/// How far a tab's title rolls as it changes to another pane's.
const TITLE_ROLL: f32 = 14.0;

/// What a tab shows in front, to animate the change when another of its
/// panes takes focus.
pub(super) struct TabFront {
    session_id: Uuid,
    stack: Vec<(Uuid, Option<String>)>,
    title: String,
    motion: Option<TabFrontMotion>,
}

#[derive(Clone)]
pub(super) struct TabFrontMotion {
    /// Gives each change an animation of its own.
    pub(super) id: u64,
    pub(super) from_stack: Vec<(Uuid, Option<String>)>,
    pub(super) from_title: String,
    /// 1.0 when focus moved to a later pane, so the title rolls up; −1.0 rolls down.
    pub(super) direction: f32,
    started: Instant,
}

/// The title `progress` (0 to 1) of the way from `from` to `to`: the old one
/// rolls out along `direction` while the new one rolls in behind it.
fn rolling_title(
    label: gpui::Div,
    from: &str,
    to: &str,
    direction: f32,
    progress: f32,
) -> gpui::Div {
    let line = |text: &str, offset: f32, opacity: f32| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(offset))
            .bottom(px(-offset))
            .flex()
            .items_center()
            .opacity(opacity)
            .child(div().min_w(px(0.0)).truncate().child(text.to_owned()))
    };
    label
        .relative()
        .h_full()
        .overflow_hidden()
        .child(line(
            from,
            -direction * TITLE_ROLL * progress,
            1.0 - progress,
        ))
        .child(line(
            to,
            direction * TITLE_ROLL * (1.0 - progress),
            progress,
        ))
}

#[derive(Clone, Copy)]
struct PaneHeaderState {
    highlighted: bool,
    zoomed: bool,
    can_drag: bool,
    can_zoom: bool,
}

impl super::WorkspaceView {
    /// Records the pane `tab_id` shows in front. When another of its panes
    /// took focus since the last frame, starts the swap; returns the swap
    /// while it runs.
    pub(super) fn tab_front_motion(
        &self,
        tab_id: Uuid,
        session_id: Uuid,
        stack: &[(Uuid, Option<String>)],
        title: &str,
        order: &[Uuid],
    ) -> Option<TabFrontMotion> {
        let now = Instant::now();
        let mut fronts = self.tab_fronts.borrow_mut();
        let Some(front) = fronts.get_mut(&tab_id) else {
            fronts.insert(
                tab_id,
                TabFront {
                    session_id,
                    stack: stack.to_vec(),
                    title: title.to_owned(),
                    motion: None,
                },
            );
            return None;
        };
        if front.session_id != session_id {
            let position = |id: Uuid| order.iter().position(|other| *other == id);
            let direction = match (position(front.session_id), position(session_id)) {
                (Some(from), Some(to)) if to < from => -1.0,
                _ => 1.0,
            };
            let id = self.tab_front_motions.get() + 1;
            self.tab_front_motions.set(id);
            front.motion = Some(TabFrontMotion {
                id,
                from_stack: std::mem::take(&mut front.stack),
                from_title: std::mem::take(&mut front.title),
                direction,
                started: now,
            });
            front.session_id = session_id;
        }
        front.stack = stack.to_vec();
        front.title = title.to_owned();
        if front
            .motion
            .as_ref()
            .is_some_and(|motion| now.duration_since(motion.started) >= TAB_FRONT_SWAP)
        {
            front.motion = None;
        }
        front.motion.clone()
    }

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
        self.tab_fronts
            .get_mut()
            .retain(|id, _| tabs.iter().any(|tab| tab.id == *id));
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
                // The focused pane's agent leads; the tab's other panes stack behind it.
                let pane_stack: Vec<(Uuid, Option<String>)> = session_id
                    .zip(identity.as_ref())
                    .map(|(id, identity)| (id, identity.agent_kind.clone()))
                    .into_iter()
                    .chain(
                        tab.sessions
                            .iter()
                            .filter(|other| Some(other.id) != session_id)
                            .map(|other| {
                                (other.id, self.pane_identity(other, index, cx).agent_kind)
                            }),
                    )
                    .collect();
                let pane_kinds: Vec<Option<String>> =
                    pane_stack.iter().map(|(_, kind)| kind.clone()).collect();
                let agent_state = identity.as_ref().and_then(|identity| identity.agent_state);
                let agent_attention = identity
                    .as_ref()
                    .and_then(|identity| identity.agent_attention);
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
                let motion = session_id.and_then(|session_id| {
                    let order: Vec<Uuid> = tab.sessions.iter().map(|session| session.id).collect();
                    self.tab_front_motion(tab_id, session_id, &pane_stack, &title, &order)
                });
                let drag = TabDrag {
                    tab_id: item,
                    title: title.clone(),
                    from_pane: false,
                };
                let ghost = {
                    let title = title.clone();
                    let pane_kinds = pane_kinds.clone();
                    DragGhost::new(TAB_RADIUS, lift(frame(), 1.0), move || {
                        tab_drag_ghost(
                            agent_tile_stack(
                                &pane_kinds,
                                agent_state,
                                agent_attention,
                                lift(frame(), 1.0),
                                TAB_TILE_SIZE,
                            ),
                            title.clone(),
                        )
                    })
                };
                div()
                    .id(SharedString::from(format!("tab-{tab_id}")))
                    .group("title-tab")
                    .min_w(px(0.0))
                    .max_w(px(TAB_MAX_WIDTH))
                    .flex_1()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_start()
                    // Not clipped: the leading tile arcs above the tab as panes swap.
                    .cursor_move()
                    .map(|tab| titlebar_tab_style(tab, selected))
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
                    .map(|tab| {
                        let ring: Hsla = if selected {
                            tab_style::selected_fill(frame())
                        } else {
                            frame().into()
                        };
                        let label = div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(TAB_TEXT_SIZE))
                            .font_weight(if selected {
                                gpui::FontWeight::MEDIUM
                            } else {
                                gpui::FontWeight::NORMAL
                            });
                        let Some(motion) = motion else {
                            return tab
                                .child(agent_tile_stack(
                                    &pane_kinds,
                                    agent_state,
                                    agent_attention,
                                    ring,
                                    TAB_TILE_SIZE,
                                ))
                                .child(label.truncate().child(title));
                        };
                        // Another pane took focus: shuffle the tiles and roll the title.
                        let to = pane_stack.clone();
                        tab.child(div().flex_none().with_animation(
                            SharedString::from(format!("tab-front-stack-{}", motion.id)),
                            Animation::new(TAB_FRONT_SWAP).with_easing(gpui::ease_in_out),
                            {
                                let from = motion.from_stack.clone();
                                move |shell, progress| {
                                    shell.child(agent_tile_stack_shuffle(
                                        &from,
                                        &to,
                                        progress,
                                        agent_state,
                                        agent_attention,
                                        ring,
                                        TAB_TILE_SIZE,
                                    ))
                                }
                            },
                        ))
                        .child(label.with_animation(
                            SharedString::from(format!("tab-front-title-{}", motion.id)),
                            Animation::new(TAB_FRONT_SWAP).with_easing(gpui::ease_in_out),
                            move |label, progress| {
                                rolling_title(
                                    label,
                                    &motion.from_title,
                                    &title,
                                    motion.direction,
                                    progress,
                                )
                            },
                        ))
                    })
                    .when_some(shortcut, |tab, shortcut| tab.child(tab_shortcut(shortcut)))
                    .children(self.tab_drag_zone(item, cx))
                    // The selected tab offers its menu; the others close from a hover.
                    .map(|tab| match (selected, session_id) {
                        (true, Some(session_id)) => tab.child(
                            tab_trailing_button(
                                format!("tab-menu-{tab_id}"),
                                "chrome-icons/ellipsis.svg",
                                "More",
                                true,
                            )
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.select_terminal(session_id, window, cx);
                                    let position = event.position();
                                    this.open_context_menu(
                                        ContextMenuKind::Pane { session_id },
                                        position.x.into(),
                                        position.y.into(),
                                        cx,
                                    );
                                    cx.stop_propagation();
                                },
                            )),
                        ),
                        _ => tab.child(
                            tab_trailing_button(
                                format!("close-tab-{tab_id}"),
                                "chrome-icons/close.svg",
                                "Close tab",
                                selected,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.close_tab(tab_id, window, cx);
                                    cx.stop_propagation();
                                },
                            )),
                        ),
                    })
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
            .px(px(6.0))
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
                let split = self.center_has_splits();
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
                // Every pane is a card with its own header: title, split, zoom, close.
                let zoomed = self
                    .snapshot
                    .selected_tab()
                    .is_some_and(|tab| tab.zoomed_session_id == Some(session_id));
                let header = self.pane_header(
                    session_id,
                    PaneHeaderState {
                        highlighted,
                        zoomed,
                        can_drag,
                        can_zoom: pane_count > 1,
                    },
                    drag,
                    cx,
                );
                card(colors().terminal)
                    .id(SharedString::from(format!("pane-{session_id}")))
                    .size_full()
                    .min_w(px(80.0))
                    .min_h(px(48.0))
                    .flex()
                    .flex_col()
                    // Among splits, the focused card's outline brightens slightly.
                    .when(split && highlighted, |pane| {
                        pane.border_color(mix(colors().border_subtle, colors().muted, 0.28))
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
                    .child(header)
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
                            .rounded(px(CARD_RADIUS - 1.0))
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
        state: PaneHeaderState,
        drag: PaneDrag,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let PaneHeaderState {
            highlighted,
            zoomed,
            can_drag,
            can_zoom,
        } = state;
        let identity = self.pane_identity_by_id(session_id, cx);
        let title = identity
            .as_ref()
            .map(|identity| identity.title.clone())
            .unwrap_or_else(|| "Terminal".to_owned());
        let detail = identity
            .as_ref()
            .and_then(|identity| identity.detail.clone());
        let button = |id: String, icon: &'static str, label: &'static str| {
            card_header_button(id, icon, label, colors().terminal)
        };
        let split_button =
            |id: String, icon: &'static str, label: &'static str, direction: PaneSplitDirection| {
                button(id, icon, label).on_click(cx.listener(move |this, _, window, cx| {
                    this.select_terminal(session_id, window, cx);
                    this.split_pane(direction, window, cx);
                }))
            };
        div()
            .id(SharedString::from(format!("pane-header-{session_id}")))
            .h(px(PANE_HEADER_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .pl(px(10.0))
            .pr(px(5.0))
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
                    if can_zoom && event.click_count == 2 {
                        this.toggle_pane_zoom_for(session_id, window, cx);
                    }
                }),
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
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(relative(0.7))
                            .truncate()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .when_some(detail, |label, detail| {
                        label.child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(12.0))
                                .text_color(colors().subtle)
                                .child(detail),
                        )
                    }),
            )
            .child(split_button(
                format!("pane-split-right-{session_id}"),
                "chrome-icons/split-view.svg",
                "Split right · ⌘D",
                PaneSplitDirection::Right,
            ))
            .child(split_button(
                format!("pane-split-down-{session_id}"),
                "chrome-icons/rows.svg",
                "Split down · ⇧⌘D",
                PaneSplitDirection::Down,
            ))
            .when(can_zoom, |header| {
                header.child(
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
            })
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
            if self.drop_empty_spaces(cx) {
                self.reconcile_terminal_views(cx);
                self.show_terminal_tab(window, cx);
                return;
            }
            self.follow_spaces();
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    /// The space's tab that holds `session_id`.
    pub(super) fn tab_of_session(
        &self,
        session_id: Uuid,
    ) -> Option<&crate::domain::workspace::TabSnapshot> {
        self.snapshot
            .selected_workspace()?
            .tabs
            .iter()
            .find(|tab| tab.sessions.iter().any(|session| session.id == session_id))
    }

    pub(super) fn close_tab(&mut self, tab_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let review_in_front = self.review_covers_terminal(cx);
        let closing_selected = self
            .snapshot
            .selected_tab()
            .is_some_and(|tab| tab.id == tab_id);
        if self.snapshot.close_tab(tab_id) {
            if self.drop_empty_spaces(cx) {
                self.reconcile_terminal_views(cx);
                self.show_terminal_tab(window, cx);
                return;
            }
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
        // Each pane draws its own card; with none, the canvas is the card.
        let canvas = if is_empty {
            card(colors().terminal)
        } else {
            div().relative().overflow_hidden()
        };
        canvas
            .flex_1()
            .min_h(px(0.0))
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

    /// A press that ends without a drag was a click, so the row it marked as a
    /// reorder source is not moving. Runs in the capture phase because a
    /// row's click handler stops the release before it reaches the root, and a
    /// stale source would make the next drag of any kind (resizing Agents, a
    /// file from Finder) open a reorder gap in its list.
    pub(super) fn forget_unstarted_reorder(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !cx.has_active_drag() && self.reorder_drag.take().is_some() {
            cx.notify();
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
