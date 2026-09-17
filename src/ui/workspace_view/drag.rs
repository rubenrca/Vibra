//! Drag payloads and floating previews for tabs, panes, and sidebar sessions.

use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, prelude::*, px};
use uuid::Uuid;

use crate::domain::workspace::{PaneBranch, WorkspaceSplitAxis};
use crate::ui::terminal::TerminalDragPreview;
use crate::ui::theme::{MONO_FONT, colors};

use super::SIDEBAR_WORKSPACE_HEIGHT;
use super::chrome::{
    SIDEBAR_ROW_PADDING, SIDEBAR_ROW_RADIUS, SidebarSessionCard, TAB_LABEL_INSET,
    sidebar_workspace_appearance, sidebar_workspace_content,
};

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

#[derive(Clone)]
pub(crate) struct SidebarWorkspaceDrag {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub card: SidebarSessionCard,
}

pub(crate) struct SidebarWorkspaceDragView {
    pub card: SidebarSessionCard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReorderDrag {
    Tab(Uuid),
    Pane(Uuid),
    SidebarWorkspace(Uuid),
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
            .h(px(26.0))
            .w(px(self.width))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .px(px(TAB_LABEL_INSET))
            .overflow_hidden()
            .rounded_full()
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

impl Render for SidebarWorkspaceDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let card = &self.card;
        let appearance = sidebar_workspace_appearance(card.selected, card.dirty, card.behind);
        div()
            .h(px(SIDEBAR_WORKSPACE_HEIGHT))
            .w(px(card.width))
            .px(px(SIDEBAR_ROW_PADDING))
            .rounded(px(SIDEBAR_ROW_RADIUS))
            .flex()
            .items_center()
            .bg(if card.selected {
                appearance.background
            } else {
                colors().sidebar
            })
            .shadow_sm()
            .child(sidebar_workspace_content(card))
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
