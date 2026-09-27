//! Drag payloads and floating previews for tabs, panes, and projects.

use gpui::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Pixels, Point, Render, Styled,
    Window, div, prelude::*, px, svg,
};
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
    pub icon: &'static str,
    pub kind: &'static str,
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
            icon: if self.tab_id == WorkspaceTabId::Review {
                "chrome-icons/diff-unified.svg"
            } else {
                "chrome-icons/terminal.svg"
            },
            kind: if self.from_pane { "Panel" } else { "Tab" },
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
            icon: "chrome-icons/terminal.svg",
            kind: "Panel",
            cursor_offset,
            terminal: Some(terminal),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReorderDrag {
    Tab(WorkspaceTabId),
    Pane(Uuid),
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
        // GPUI anchors a drag at the grab point. Offset the compact preview so
        // even a grab near the far edge of a wide pane stays beside the cursor.
        div()
            .relative()
            .left(self.cursor_offset.x + px(14.0))
            .top(self.cursor_offset.y + px(18.0))
            .w(px(264.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(10.0))
            .border_1()
            .border_color(gpui::Hsla::from(colors().accent).opacity(0.45))
            .bg(floating_surface(colors().elevated))
            .shadow_lg()
            .child(
                div()
                    .h(px(46.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        svg()
                            .path(self.icon)
                            .size(px(17.0))
                            .flex_none()
                            .text_color(colors().accent),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(colors().muted)
                                    .child(self.kind),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child(self.title.clone()),
                            ),
                    )
                    .child(
                        svg()
                            .path("chrome-icons/grip.svg")
                            .size(px(14.0))
                            .text_color(colors().subtle),
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
    }
}

#[derive(Clone)]
pub(crate) struct ProjectDrag {
    pub project_id: Uuid,
    pub name: String,
}

impl Render for ProjectDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(colors().elevated)
            .text_size(px(12.0))
            .text_color(colors().foreground)
            .child(self.name.clone())
    }
}
