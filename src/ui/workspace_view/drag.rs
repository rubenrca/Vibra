//! Drag payloads and floating previews for tabs, panes, and projects.

use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, prelude::*, px};
use uuid::Uuid;

use crate::domain::workspace::{PaneBranch, WorkspaceSplitAxis};
use crate::ui::terminal::TerminalDragPreview;
use crate::ui::theme::{MONO_FONT, colors};

use super::chrome::TAB_LABEL_INSET;

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
    pub tab_id: Uuid,
    pub title: String,
    pub selected: bool,
    pub shortcut: Option<String>,
    pub tab_count: usize,
}

pub(crate) struct TabDragView {
    pub title: String,
    pub selected: bool,
    pub shortcut: Option<String>,
    pub width: f32,
}

#[derive(Clone)]
pub(crate) struct PaneDrag {
    pub session_id: Uuid,
    pub preview: TerminalDragPreview,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReorderDrag {
    Tab(Uuid),
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
        let selected = self.selected;
        div()
            .h(px(30.0))
            .w(px(self.width))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .px(px(TAB_LABEL_INSET))
            .overflow_hidden()
            .rounded(px(6.0))
            .bg(if selected {
                colors().selection
            } else {
                gpui::rgba(0x00000000)
            })
            .text_color(colors().foreground)
            .shadow_sm()
            .opacity(0.96)
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .truncate()
                    .text_center()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(self.title.clone()),
            )
            .when_some(self.shortcut.clone(), |tab, shortcut| {
                tab.child(
                    div()
                        .absolute()
                        .right(px(10.0))
                        .flex_none()
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
