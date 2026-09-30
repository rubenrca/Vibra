use std::collections::HashSet;

use gpui::{Context, Window};

use crate::infrastructure::work_items;

use super::{AGENTS, WorkspaceView};

impl WorkspaceView {
    pub(crate) fn open_inbox_composer(&mut self, cx: &mut Context<Self>) {
        self.work_inbox.composer_open = true;
        self.work_inbox.discussion_open = false;
        self.work_inbox.menu = None;
        self.work_inbox.search_editing = false;
        self.work_inbox.comment_editing = false;
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
    }

    pub(crate) fn start_work_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut item) = self.selected_work_item().cloned() else {
            return;
        };
        if let Some(detail) = self
            .work_inbox
            .details
            .get(&item.url)
            .and_then(|state| state.summary.data.as_ref())
        {
            item.body = detail.body.clone();
        }
        if !self.work_inbox.composer_note.trim().is_empty() {
            item.body.push_str("\n\nAdditional user instructions:\n");
            item.body.push_str(&self.work_inbox.composer_note);
        }
        let Some(project) = self.work_inbox.target_project else {
            self.work_inbox.action_error =
                Some("Choose a project with a folder to start the task.".into());
            cx.notify();
            return;
        };
        let (_, agent) = AGENTS[self.work_inbox.agent];
        if !self
            .snapshot
            .projects
            .iter()
            .any(|candidate| candidate.id == project && candidate.directory().is_some())
        {
            self.work_inbox.action_error =
                Some("The project is no longer available. Choose another project.".into());
            cx.notify();
            return;
        }
        let (command, prompt_path) = match work_items::prepare_launch(agent, &item) {
            Ok(launch) => launch,
            Err(error) => {
                self.work_inbox.action_error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        match self.run_in_new_tab(
            project,
            &format!("{} · {}", item.reference, item.title),
            &command,
            false,
            cx,
        ) {
            Ok((pane, true)) => {
                let live: HashSet<_> = self
                    .snapshot
                    .terminal_sessions()
                    .map(|session| session.id)
                    .collect();
                self.settings.inbox.linked_sessions.retain(|_, panes| {
                    panes.retain(|pane| live.contains(pane));
                    !panes.is_empty()
                });
                self.settings
                    .inbox
                    .linked_sessions
                    .entry(item.url)
                    .or_default()
                    .push(pane);
                self.work_inbox.composer_open = false;
                self.work_inbox.composer_editing = false;
                self.persist_settings(cx);
                self.work_inbox.action_error = None;
                self.open_pane(pane, window, cx);
            }
            Ok((_, false)) => {
                let _ = std::fs::remove_file(&prompt_path);
                self.work_inbox.action_error = Some(
                    "The tab opened, but the terminal could not start the agent. Check the terminal and try again."
                        .into(),
                );
            }
            Err(message) => {
                let _ = std::fs::remove_file(&prompt_path);
                self.work_inbox.action_error = Some(message.into());
            }
        }
        cx.notify();
    }
}
