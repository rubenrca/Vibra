//! Drag payloads and floating previews for tabs, panes, and projects.

use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext, Context, Entity, IntoElement,
    ParentElement, Pixels, Point, Render, Styled, Window, canvas, div, prelude::*, px, svg,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::domain::workspace::{PaneBranch, WorkspaceSplitAxis, WorkspaceTabId};
use crate::ui::terminal::TerminalDragPreview;
use crate::ui::theme::{colors, floating_surface};

#[derive(Clone)]
pub(crate) struct PaneDividerDrag {
    pub path: Vec<PaneBranch>,
    pub axis: WorkspaceSplitAxis,
}

pub(crate) struct PaneDividerDragView {
    pub axis: WorkspaceSplitAxis,
}

#[derive(Clone)]
pub(crate) struct TabDrag {
    pub tab_id: WorkspaceTabId,
    pub title: String,
    pub from_pane: bool,
}

pub(crate) struct TabDragView {
    pub title: String,
    pub label: &'static str,
    pub icon: &'static str,
    pub icon_color: gpui::Rgba,
    pub cursor_offset: Point<Pixels>,
    pub terminal: Option<Entity<TerminalDragPreview>>,
}

#[derive(Clone)]
pub(crate) struct PaneDrag {
    pub session_id: Uuid,
    pub title: String,
    pub preview: TerminalDragPreview,
}

impl TabDrag {
    pub fn preview(&self, cursor_offset: Point<Pixels>, cx: &mut App) -> Entity<TabDragView> {
        cx.new(|_| TabDragView {
            title: self.title.clone(),
            label: if self.from_pane {
                "Moving pane"
            } else {
                "Moving tab"
            },
            icon: if self.tab_id == WorkspaceTabId::Review {
                "chrome-icons/diff-unified.svg"
            } else {
                "chrome-icons/terminal.svg"
            },
            icon_color: colors().accent,
            cursor_offset,
            terminal: None,
        })
    }
}

impl PaneDrag {
    pub fn preview(&self, cursor_offset: Point<Pixels>, cx: &mut App) -> Entity<TabDragView> {
        let terminal = cx.new(|_| self.preview.clone().thumbnail(262.0, 120.0));
        cx.new(|_| TabDragView {
            title: self.title.clone(),
            label: "Moving pane",
            icon: "chrome-icons/terminal.svg",
            icon_color: colors().accent,
            cursor_offset,
            terminal: Some(terminal),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReorderDrag {
    Tab(WorkspaceTabId),
    Pane(Uuid),
    Project(Uuid),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarResizeEdge {
    Left,
    Right,
}

pub(crate) struct SidebarResizeDragView;

impl Render for PaneDividerDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .when(self.axis == WorkspaceSplitAxis::Horizontal, |line| {
                line.w(px(2.0)).h(px(40.0))
            })
            .when(self.axis == WorkspaceSplitAxis::Vertical, |line| {
                line.w(px(40.0)).h(px(2.0))
            })
            .rounded_full()
            .bg(colors().border_subtle)
    }
}

impl Render for SidebarResizeDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Keep sidebar resizing available without a floating bar during the drag.
        div()
    }
}

impl Render for TabDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Compensate for GPUI's grab offset so long rows cannot put the preview
        // far away from the pointer. Keep the actual insertion point uncovered.
        let offset = self.cursor_offset;
        div()
            .relative()
            .left(offset.x + px(12.0))
            .top(offset.y + px(12.0))
            .w(px(if self.terminal.is_some() {
                264.0
            } else {
                224.0
            }))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(8.0))
            .border_1()
            .border_color(gpui::Hsla::from(colors().accent).opacity(0.65))
            .bg(floating_surface(colors().elevated))
            .shadow_lg()
            .child(
                div()
                    .h(px(36.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        svg()
                            .path(self.icon)
                            .size(px(14.0))
                            .flex_none()
                            .text_color(self.icon_color),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().foreground)
                            .child(self.title.clone()),
                    )
                    .child(
                        svg()
                            .path("chrome-icons/grip.svg")
                            .size(px(12.0))
                            .flex_none()
                            .text_color(colors().accent),
                    ),
            )
            .when_some(self.terminal.clone(), |card, terminal| {
                card.child(
                    div()
                        .flex()
                        .justify_center()
                        .bg(colors().terminal)
                        .border_t_1()
                        .border_color(colors().border_subtle)
                        .overflow_hidden()
                        .child(terminal),
                )
            })
            .child(
                div()
                    .h(px(24.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(gpui::Hsla::from(colors().accent).opacity(0.18))
                    .bg(gpui::Hsla::from(colors().accent).opacity(0.08))
                    .text_size(px(10.0))
                    .child(div().text_color(colors().accent).child(self.label))
                    .child(div().text_color(colors().muted).child("Esc to cancel")),
            )
            .with_animation(
                "drag-preview-lift",
                Animation::new(Duration::from_millis(120)).with_easing(gpui::ease_out_quint()),
                move |card, progress| {
                    card.opacity(0.8 + 0.2 * progress)
                        .top(offset.y + px(12.0 + 4.0 * (1.0 - progress)))
                },
            )
    }
}

#[derive(Clone)]
pub(crate) struct ProjectDrag {
    pub project_id: Uuid,
}

/// A lifted copy of the row being dragged. GPUI places the drag view at the
/// pointer minus the grab offset, so rendering the copy at its own origin keeps
/// it exactly where the row was grabbed instead of swapping in another card.
#[derive(Clone)]
pub(crate) struct DragGhost {
    width: Rc<Cell<Pixels>>,
    radius: Pixels,
    base: gpui::Rgba,
    content: Rc<dyn Fn() -> AnyElement>,
}

pub(crate) struct DragGhostView {
    ghost: DragGhost,
}

impl DragGhost {
    /// `base` is the surface the row normally sits on; the copy floats above
    /// other content, so it paints that surface itself before the row tint.
    pub fn new(radius: f32, base: gpui::Rgba, content: impl Fn() -> AnyElement + 'static) -> Self {
        Self {
            width: Rc::new(Cell::new(px(0.0))),
            radius: px(radius),
            base,
            content: Rc::new(content),
        }
    }

    /// Records the laid-out width of flexible rows such as tabs.
    pub fn measure(&self) -> impl IntoElement {
        measure_width(self.width.clone())
    }

    pub fn preview(&self, cx: &mut App) -> Entity<DragGhostView> {
        let ghost = self.clone();
        cx.new(|_| DragGhostView { ghost })
    }
}

impl Render for DragGhostView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let width = self.ghost.width.get();
        div()
            .when(width > px(0.0), |ghost| ghost.w(width))
            .rounded(self.ghost.radius)
            .bg(floating_surface(self.ghost.base))
            .shadow_lg()
            .child((self.ghost.content)())
    }
}

/// Where the dragged item would land in the tab strip.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TabStripDrop {
    /// The tab the dragged one lands before; `None` is the end of the strip.
    pub before: Option<WorkspaceTabId>,
    /// Dropping docks the dragged tab into this one as a split instead.
    pub merge: Option<WorkspaceTabId>,
}

/// One position in a list that is being reordered live.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReorderSlot<T> {
    Item(T),
    /// The space the dragged item takes when dropped.
    Gap,
}

/// The list as it looks after the drop: the source leaves its place and a gap
/// opens where it lands, so the other items slide aside while dragging.
/// `landing` is `None` while the pointer is outside the list, which keeps the
/// source's original place, or opens no gap for items from elsewhere.
pub(super) fn reorder_slots<T: Copy + PartialEq>(
    order: &[T],
    source: Option<T>,
    landing: Option<Option<T>>,
) -> Vec<ReorderSlot<T>> {
    let mut slots: Vec<_> = order
        .iter()
        .copied()
        .filter(|item| Some(*item) != source)
        .map(ReorderSlot::Item)
        .collect();
    let gap = match landing {
        Some(before) => Some(
            before
                .and_then(|before| {
                    slots
                        .iter()
                        .position(|slot| *slot == ReorderSlot::Item(before))
                })
                .unwrap_or(slots.len()),
        ),
        None => source.and_then(|source| order.iter().position(|item| *item == source)),
    };
    if let Some(gap) = gap {
        slots.insert(gap, ReorderSlot::Gap);
    }
    slots
}

/// The item a dragged one lands before while the pointer is over `target`.
/// Past the middle the gap moves after the target, which pushes it aside.
pub(super) fn landing_beside<T: Copy + PartialEq>(
    order: &[T],
    source: Option<T>,
    target: T,
    after: bool,
) -> Option<T> {
    if !after {
        return Some(target);
    }
    let mut rest = order.iter().copied().filter(|item| Some(*item) != source);
    rest.find(|item| *item == target);
    rest.next()
}

/// Stores the laid-out width of the parent element for the next render.
pub(super) fn measure_width(width: Rc<Cell<Pixels>>) -> impl IntoElement {
    canvas(
        move |bounds, _, _| width.set(bounds.size.width),
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

const SLIDE_DURATION: Duration = Duration::from_millis(160);

/// An item sliding from its previous place in a reordered list.
#[derive(Clone, Copy)]
pub(crate) struct Slide {
    id: u64,
    /// Offset from the new place when the slide started.
    from: f32,
    started: Instant,
}

impl Slide {
    fn offset(&self, now: Instant) -> f32 {
        let progress = (now - self.started).as_secs_f32() / SLIDE_DURATION.as_secs_f32();
        self.from * (1.0 - gpui::ease_out_quint()(progress.min(1.0)))
    }
}

/// Lets items in a live reordered list glide into their new place. Positions
/// come from slot sizes, so the offset is known before layout and nothing
/// flashes at the destination first.
pub(crate) struct SlotMotion<T> {
    previous: Vec<ReorderSlot<T>>,
    previous_sizes: Vec<Pixels>,
    slides: Vec<(T, Slide)>,
    next_id: u64,
}

impl<T> Default for SlotMotion<T> {
    fn default() -> Self {
        Self {
            previous: Vec::new(),
            previous_sizes: Vec::new(),
            slides: Vec::new(),
            next_id: 0,
        }
    }
}

impl<T: Copy + PartialEq> SlotMotion<T> {
    /// Record the slots about to be drawn. Only a rearrangement of the same
    /// items slides; opening, closing, or switching lists stays instant.
    pub fn update(&mut self, slots: &[ReorderSlot<T>], pitch: Pixels) {
        self.update_with_sizes(slots, |_| pitch);
    }

    /// Project groups include a varying number of agent rows. Their drag gap
    /// and slide offsets must reserve the entire group, not just its header.
    pub fn update_with_sizes(
        &mut self,
        slots: &[ReorderSlot<T>],
        size: impl Fn(ReorderSlot<T>) -> Pixels,
    ) {
        let sizes: Vec<_> = slots.iter().copied().map(size).collect();
        let now = Instant::now();
        self.slides
            .retain(|(_, slide)| now - slide.started < SLIDE_DURATION);
        if slots == self.previous.as_slice() && sizes == self.previous_sizes {
            return;
        }
        let items = |slots: &[ReorderSlot<T>]| {
            slots
                .iter()
                .filter(|slot| matches!(slot, ReorderSlot::Item(_)))
                .count()
        };
        let same_items = items(slots) == items(&self.previous)
            && slots
                .iter()
                .all(|slot| matches!(slot, ReorderSlot::Gap) || self.previous.contains(slot));
        let same_sizes = slots.iter().zip(&sizes).all(|(slot, size)| {
            self.previous
                .iter()
                .position(|previous| previous == slot)
                .is_none_or(|old| self.previous_sizes[old] == *size)
        });
        if same_items && same_sizes {
            for (index, slot) in slots.iter().enumerate() {
                let ReorderSlot::Item(item) = *slot else {
                    continue;
                };
                let Some(old) = self.previous.iter().position(|previous| previous == slot) else {
                    continue;
                };
                let old_position: f32 = self.previous_sizes[..old]
                    .iter()
                    .map(|size| f32::from(*size))
                    .sum();
                let new_position: f32 = sizes[..index].iter().map(|size| f32::from(*size)).sum();
                if old_position == new_position {
                    continue;
                }
                // Continue from wherever an interrupted slide currently is.
                let current = self.slide(item).map_or(0.0, |slide| slide.offset(now));
                self.slides.retain(|(moving, _)| *moving != item);
                self.next_id += 1;
                self.slides.push((
                    item,
                    Slide {
                        id: self.next_id,
                        from: current + old_position - new_position,
                        started: now,
                    },
                ));
            }
        } else {
            self.slides.clear();
        }
        self.previous = slots.to_vec();
        self.previous_sizes = sizes;
    }

    pub fn slide(&self, item: T) -> Option<Slide> {
        self.slides
            .iter()
            .find(|(moving, _)| *moving == item)
            .map(|(_, slide)| *slide)
    }
}

/// Draws `element` along its current slide. It must be positioned relatively.
pub(super) fn slide_into_place<E>(
    element: E,
    slide: Option<Slide>,
    list: &'static str,
    vertical: bool,
) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    let Some(slide) = slide else {
        return element.into_any_element();
    };
    element
        .with_animation(
            gpui::SharedString::from(format!("{list}-slide-{}", slide.id)),
            Animation::new(SLIDE_DURATION),
            move |element, _| {
                let offset = px(slide.offset(Instant::now()));
                if vertical {
                    element.top(offset)
                } else {
                    element.left(offset)
                }
            },
        )
        .into_any_element()
}

/// The room a dragged item takes before it is dropped.
pub(super) fn reorder_gap() -> gpui::Div {
    div()
        .bg(gpui::Hsla::from(colors().accent).opacity(0.08))
        .border_1()
        .border_dashed()
        .border_color(gpui::Hsla::from(colors().accent).opacity(0.3))
}

#[cfg(test)]
mod reorder_tests {
    use super::{ReorderSlot::*, SlotMotion, landing_beside, reorder_slots};
    use gpui::px;

    #[test]
    fn only_rearranged_items_slide_from_their_previous_place() {
        let mut motion = SlotMotion::default();
        motion.update(&[Item(1), Item(2), Item(3)], px(100.0));
        motion.update(&[Gap, Item(2), Item(3)], px(100.0));
        assert!(
            motion.slide(2).is_none(),
            "the gap takes the source's place"
        );
        motion.update(&[Item(2), Gap, Item(3)], px(100.0));
        assert_eq!(motion.slide(2).map(|slide| slide.from), Some(100.0));
        assert!(motion.slide(3).is_none());
        motion.update(&[Item(2), Item(1), Item(3), Item(4)], px(100.0));
        assert!(motion.slide(1).is_none(), "new items appear in place");
    }

    #[test]
    fn project_groups_slide_by_their_full_height() {
        let mut motion = SlotMotion::default();
        let size = |slot| px(if slot == Item(2) { 34.0 } else { 94.0 });
        motion.update_with_sizes(&[Gap, Item(2), Item(3)], size);
        motion.update_with_sizes(&[Item(2), Item(3), Gap], size);
        assert_eq!(motion.slide(2).map(|slide| slide.from), Some(94.0));
        assert_eq!(motion.slide(3).map(|slide| slide.from), Some(94.0));
        // An agent leaving changes a group's height without reordering it.
        motion.update_with_sizes(&[Item(2), Item(3), Gap], |_| px(34.0));
        assert!(motion.slide(2).is_none());
        assert!(motion.slide(3).is_none());
    }

    #[test]
    fn unequal_groups_can_move_an_item_without_changing_its_index() {
        let mut motion = SlotMotion::default();
        let size = |slot| px(if slot == Item(1) { 94.0 } else { 34.0 });
        motion.update_with_sizes(&[Item(1), Item(2), Item(3)], size);
        motion.update_with_sizes(&[Item(3), Item(2), Item(1)], size);
        assert_eq!(motion.slide(2).map(|slide| slide.from), Some(60.0));
    }

    #[test]
    fn the_gap_follows_the_landing_and_the_source_leaves_its_place() {
        let order = [1, 2, 3];
        assert_eq!(
            reorder_slots(&order, Some(1), None),
            vec![Gap, Item(2), Item(3)]
        );
        assert_eq!(
            reorder_slots(&order, Some(1), Some(Some(3))),
            vec![Item(2), Gap, Item(3)]
        );
        assert_eq!(
            reorder_slots(&order, Some(1), Some(None)),
            vec![Item(2), Item(3), Gap]
        );
        assert_eq!(
            reorder_slots(&order, None, None),
            vec![Item(1), Item(2), Item(3)]
        );
        assert_eq!(
            reorder_slots(&order, None, Some(Some(2))),
            vec![Item(1), Gap, Item(2), Item(3)]
        );
    }

    #[test]
    fn crossing_the_middle_of_a_neighbor_moves_past_it() {
        let order = [1, 2, 3];
        assert_eq!(landing_beside(&order, Some(1), 2, false), Some(2));
        assert_eq!(landing_beside(&order, Some(1), 2, true), Some(3));
        assert_eq!(landing_beside(&order, Some(1), 3, true), None);
        assert_eq!(landing_beside(&order, Some(3), 2, true), None);
    }
}
