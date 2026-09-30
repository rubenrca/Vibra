use gpui::{Context, Div, Window, prelude::*};
use uuid::Uuid;

use crate::domain::workspace::{PaneFocusDirection, PaneResizeDirection, PaneSplitDirection};
use crate::{
    CloseTerminal, FocusPaneDown, FocusPaneLeft, FocusPaneRight, FocusPaneUp, NewTerminalTab,
    NextPane, NextProject, PreviousPane, PreviousProject, ResizePaneDown, ResizePaneLeft,
    ResizePaneRight, ResizePaneUp, ShowSettings, SplitPaneDown, SplitPaneLeft, SplitPaneRight,
    SplitPaneUp, ToggleLeftSidebar, ToggleRightSidebar,
};

use super::*;

impl WorkspaceView {
    pub(super) fn new_terminal_tab(
        &mut self,
        _: &NewTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_terminal_tab_in_project(window, cx);
    }

    /// `⌘T` / `⌘N`: a new tab in the selected project, or a folder picker
    /// when there is no project yet.
    pub(super) fn open_terminal_tab_in_project(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.snapshot.selected_project_id {
            Some(project_id) => self.open_project_tab(project_id, window, cx),
            None => self.choose_project_folder(None, true, window, cx),
        }
    }

    pub(super) fn close_terminal(
        &mut self,
        _: &CloseTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_section != WorkspaceSection::Workspace {
            self.select_section(WorkspaceSection::Workspace, window, cx);
            return;
        }
        if self.review_visible(cx)
            && (self.review_covers_terminal(cx)
                || self.diff_view.read(cx).review_has_focus(window, cx))
        {
            self.close_review(window, cx);
            return;
        }
        if let Some(session_id) = self.snapshot.selected_session().map(|session| session.id) {
            self.close_pane(session_id, window, cx);
        }
    }

    pub(super) fn toggle_left_sidebar(
        &mut self,
        _: &ToggleLeftSidebar,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_left_sidebar_visible(!self.settings.left_sidebar_visible, true, cx);
    }

    pub(super) fn show_settings(
        &mut self,
        _: &ShowSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(window, cx);
    }

    pub(super) fn toggle_right_sidebar(
        &mut self,
        _: &ToggleRightSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_workspace_panel(window, cx);
    }

    pub(super) fn previous_project(
        &mut self,
        _: &PreviousProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_project(-1, window, cx);
    }

    pub(super) fn next_project(
        &mut self,
        _: &NextProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_project(1, window, cx);
    }

    /// Moves through projects in sidebar order (pinned first).
    pub(super) fn cycle_project(
        &mut self,
        offset: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let order = self.visible_project_order();
        if order.is_empty() {
            return;
        }
        let current = self
            .snapshot
            .selected_project_id
            .and_then(|id| order.iter().position(|item| *item == id))
            .unwrap_or(0) as isize;
        let next = (current + offset).rem_euclid(order.len() as isize) as usize;
        self.select_project(order[next], window, cx);
    }

    pub(crate) fn select_tab(&mut self, tab_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.select_tab(tab_id) {
            self.show_terminal_tab(window, cx);
        }
    }

    pub(super) fn bind_workspace_actions(
        &self,
        body: gpui::Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        body.on_action(cx.listener(Self::add_project))
            .on_action(cx.listener(Self::new_terminal_tab))
            .on_action(cx.listener(Self::close_terminal))
            .on_action(cx.listener(Self::toggle_left_sidebar))
            .on_action(cx.listener(Self::toggle_right_sidebar))
            .on_action(cx.listener(Self::previous_project))
            .on_action(cx.listener(Self::next_project))
            .on_action(cx.listener(Self::go_to_project))
            .on_action(cx.listener(Self::go_to_tab))
            .on_action(cx.listener(Self::navigate_back))
            .on_action(cx.listener(Self::navigate_forward))
            .on_action(cx.listener(|this, _: &SplitPaneLeft, window, cx| {
                this.split_pane(PaneSplitDirection::Left, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitPaneRight, window, cx| {
                this.split_pane(PaneSplitDirection::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitPaneUp, window, cx| {
                this.split_pane(PaneSplitDirection::Up, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitPaneDown, window, cx| {
                this.split_pane(PaneSplitDirection::Down, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPaneLeft, window, cx| {
                this.focus_pane(PaneFocusDirection::Left, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPaneRight, window, cx| {
                this.focus_pane(PaneFocusDirection::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPaneUp, window, cx| {
                this.focus_pane(PaneFocusDirection::Up, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPaneDown, window, cx| {
                this.focus_pane(PaneFocusDirection::Down, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousPane, window, cx| {
                this.cycle_pane(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextPane, window, cx| {
                this.cycle_pane(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ResizePaneLeft, _, cx| {
                this.resize_pane(PaneResizeDirection::Left, cx);
            }))
            .on_action(cx.listener(|this, _: &ResizePaneRight, _, cx| {
                this.resize_pane(PaneResizeDirection::Right, cx);
            }))
            .on_action(cx.listener(|this, _: &ResizePaneUp, _, cx| {
                this.resize_pane(PaneResizeDirection::Up, cx);
            }))
            .on_action(cx.listener(|this, _: &ResizePaneDown, _, cx| {
                this.resize_pane(PaneResizeDirection::Down, cx);
            }))
            .on_action(cx.listener(Self::equalize_panes))
            .on_action(cx.listener(Self::toggle_pane_zoom))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::quick_open))
            .on_action(cx.listener(Self::open_ide))
            .on_action(cx.listener(Self::show_settings))
    }
}
