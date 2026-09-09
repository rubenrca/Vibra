//! Drag payloads and floating previews for tabs, panes, and sidebar sessions.

use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, prelude::*, px};
use uuid::Uuid;

use crate::domain::workspace::{PaneBranch, WorkspaceSplitAxis};
use crate::infrastructure::automation::{AgentAttention, AgentRuntimeState};
use crate::ui::agent_marks::{agent_sidebar_badge, agent_status_color};
use crate::ui::terminal::TerminalDragPreview;
use crate::ui::theme::{MONO_FONT, colors};

use super::chrome::{
    sidebar_agent_line, sidebar_location_line, sidebar_workspace_appearance,
    sidebar_workspace_text_column,
};
use super::{SIDEBAR_WORKSPACE_CARD_CHROME, SIDEBAR_WORKSPACE_HEIGHT};

#[derive(Clone)]
pub(crate) struct PaneDividerDrag {
    pub path: Vec<PaneBranch>,
    pub axis: WorkspaceSplitAxis,
}

pub(crate) struct PaneDividerDragView {
    pub axis: WorkspaceSplitAxis,
}

#[derive(Clone, Copy)]
pub(crate) struct DevTerminalResize;

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
    pub source_space_id: Option<Uuid>,
    pub title: String,
    pub branch: Option<String>,
    pub path: String,
    pub selected: bool,
    pub dirty: bool,
    pub behind: usize,
    pub agent_kind: Option<String>,
    pub agent_state: Option<AgentRuntimeState>,
    pub agent_attention: Option<AgentAttention>,
    pub agent_model: Option<String>,
    pub width: f32,
}

pub(crate) struct SidebarWorkspaceDragView {
    pub title: String,
    pub branch: Option<String>,
    pub path: String,
    pub selected: bool,
    pub dirty: bool,
    pub behind: usize,
    pub agent_kind: Option<String>,
    pub agent_state: Option<AgentRuntimeState>,
    pub agent_attention: Option<AgentAttention>,
    pub agent_model: Option<String>,
    pub width: f32,
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
            .px(px(10.0))
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
        let appearance = sidebar_workspace_appearance(self.selected, self.dirty, self.behind);
        let agent_line = sidebar_agent_line(
            self.agent_kind.as_deref(),
            self.agent_model.as_deref(),
            self.agent_state,
            self.agent_attention,
        );
        let agent_color = agent_status_color(self.agent_state, self.agent_attention)
            .unwrap_or(appearance.agent_fallback);
        let location_line = sidebar_location_line(self.branch.as_deref(), &self.path);
        let location_color = if self.branch.is_some() {
            appearance.branch
        } else {
            appearance.path
        };

        div()
            .h(px(SIDEBAR_WORKSPACE_HEIGHT))
            .w(px(self.width))
            .px(px(10.0))
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap_2()
            .bg(appearance.background)
            .border_1()
            .border_color(appearance.border)
            .child(agent_sidebar_badge(
                self.agent_kind.as_deref(),
                self.agent_state,
                self.agent_attention,
                self.selected,
            ))
            .child(sidebar_workspace_text_column(
                (self.width - SIDEBAR_WORKSPACE_CARD_CHROME).max(80.0),
                &self.title,
                appearance.title,
                &agent_line,
                agent_color,
                &location_line,
                location_color,
            ))
    }
}
