//! Scratch terminal: a hidden project without a folder. Its tabs, panes, and
//! shortcuts are the usual ones; new tabs start in the current tab's cwd and
//! Explorer, Changes, and the branch follow the selected terminal.

use gpui::{Context, Window};

use crate::ToggleScratch;

use super::{WorkspaceSection, WorkspaceView};

pub(super) const SCRATCH_SHORTCUT: &str = "⌥⌘T";

impl WorkspaceView {
    pub(super) fn toggle_scratch_action(
        &mut self,
        _: &ToggleScratch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_scratch(window, cx);
    }

    pub(super) fn scratch_selected(&self) -> bool {
        self.workspace_section == WorkspaceSection::Workspace
            && self
                .snapshot
                .selected_project()
                .is_some_and(|project| project.scratch)
    }

    /// The shortcut opens Scratch, or returns to where it was
    /// opened from, whatever tabs were opened in between.
    pub(super) fn toggle_scratch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.scratch_selected() {
            self.open_scratch(window, cx);
            return;
        }
        let origin = self.navigation.scratch_origin.take();
        if !origin.is_some_and(|origin| self.apply_location(origin, window, cx))
            && self.navigation.can_go_back()
        {
            self.navigate(true, window, cx);
        }
    }

    pub(super) fn open_scratch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.scratch_selected() {
            self.navigation.scratch_origin = self.current_location(cx);
        }
        let id = self.snapshot.ensure_scratch();
        let has_tabs = self
            .snapshot
            .project(id)
            .is_some_and(|project| project.terminal_sessions().next().is_some());
        if has_tabs {
            self.select_project(id, window, cx);
        } else {
            self.open_project_tab(id, window, cx);
        }
    }

    /// The current Scratch tab's live cwd, else the home folder.
    pub(super) fn scratch_tab_directory(&self, cx: &Context<Self>) -> String {
        let current = self
            .scratch_selected()
            .then(|| self.snapshot.selected_session())
            .flatten()
            .map(|_| self.selected_live_cwd(cx));
        current
            .or_else(|| self.home_directory.clone())
            .unwrap_or_else(|| self.launch_directory.clone())
            .to_string_lossy()
            .into_owned()
    }
}
