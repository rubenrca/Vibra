//! Native Inbox coordination. Layout and detail actions follow MonoCode’s Inbox.

mod detail;
mod feed;
mod history;
mod launch;
mod layout;
mod local;
mod review;
mod state;

pub(crate) use crate::domain::work_items::{
    PrAction, RepositoryTab, WorkCheck, WorkDiff, WorkFilter, WorkItem, WorkKind, WorkStatus,
};
pub(crate) use crate::infrastructure::work_items;
pub(crate) use crate::ui::theme::colors;
pub(crate) use gpui::{
    AnyElement, ClipboardItem, Context, SharedString, Window, div, prelude::*, px,
};
pub(crate) use state::*;
pub(crate) use uuid::Uuid;

use crate::ui::text_edit::{TextKeyOutcome, apply_text_key};
use gpui::KeyDownEvent;

use super::{WorkspaceSection, WorkspaceView};

impl WorkspaceView {
    pub(super) fn handle_inbox_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspace_section != WorkspaceSection::Inbox || self.work_inbox.activity {
            return false;
        }
        if matches!(event.keystroke.key.as_str(), "escape" | "esc") && self.dismiss_inbox_menu() {
            cx.notify();
            return true;
        }
        let search = self.work_inbox.search_editing;
        let composer = self.work_inbox.composer_editing && self.work_inbox.composer_open;
        let comment = self.work_inbox.comment_editing;
        let review = self.work_inbox.selected.as_ref().is_some_and(|url| {
            self.work_inbox
                .details
                .get(url)
                .is_some_and(|state| state.review_draft.open && state.review_draft.editing)
        });
        let buffer = if search {
            &mut self.work_inbox.filter.query
        } else if composer {
            &mut self.work_inbox.composer_note
        } else if comment || review {
            let Some(url) = self.work_inbox.selected.clone() else {
                return false;
            };
            let state = self.work_inbox.details.entry(url).or_default();
            if comment {
                if state.posting {
                    return false;
                }
                &mut state.draft
            } else {
                if state.review_draft.busy {
                    return false;
                }
                &mut state.review_draft.body
            }
        } else {
            return self.handle_inbox_shortcut(event, window, cx);
        };
        let outcome = apply_text_key(
            buffer,
            &event.keystroke.key,
            event.keystroke.key_char.as_deref(),
            &event.keystroke.modifiers,
            !search,
            || cx.read_from_clipboard().and_then(|item| item.text()),
        );
        match outcome {
            TextKeyOutcome::Edited => {
                if search {
                    self.sync_inbox_selection(cx);
                    self.work_inbox.search_editing = true;
                }
            }
            TextKeyOutcome::Submit if comment => self.post_inbox_comment(cx),
            TextKeyOutcome::Submit if review => self.submit_inbox_review(cx),
            TextKeyOutcome::Submit if composer => self.start_work_item(window, cx),
            TextKeyOutcome::Submit
            | TextKeyOutcome::Cancel
            | TextKeyOutcome::NextField
            | TextKeyOutcome::PreviousField => {
                self.work_inbox.search_editing = false;
                self.work_inbox.composer_editing = false;
                self.work_inbox.comment_editing = false;
                if let Some(url) = self.work_inbox.selected.clone()
                    && let Some(state) = self.work_inbox.details.get_mut(&url)
                {
                    state.review_draft.editing = false;
                }
            }
            TextKeyOutcome::Unhandled => return false,
        }
        cx.notify();
        true
    }

    /// Single-key shortcuts while no Review field takes text.
    fn handle_inbox_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt || modifiers.function {
            return false;
        }
        let key = keystroke.key.to_ascii_lowercase();
        if keystroke.key_char.as_deref() == Some("?")
            || key == "?"
            || (key == "/" && modifiers.shift)
        {
            let width: f32 = window.bounds().size.width.into();
            self.work_inbox.menu_position = (width - 250.0, 40.0);
            self.work_inbox.menu = Some(InboxMenu::Shortcuts);
            cx.notify();
            return true;
        }
        match key.as_str() {
            "h" => self.select_repository_tab(RepositoryTab::History, cx),
            "p" => self.select_repository_tab(RepositoryTab::PullRequests, cx),
            "i" => self.select_repository_tab(RepositoryTab::Issues, cx),
            "r" => self.refresh_inbox(cx),
            _ if self.settings.inbox.tab == RepositoryTab::History => {
                return self.handle_history_shortcut(&key, cx);
            }
            "/" => self.focus_inbox_search(window, cx),
            "j" | "down" => self.step_inbox_item(1, cx),
            "k" | "up" => self.step_inbox_item(-1, cx),
            "1" | "2" | "3" | "4" => {
                let index = key.parse::<usize>().unwrap_or(1) - 1;
                self.select_inbox_detail_tab(DetailTab::ALL[index], cx);
            }
            "]" | "[" => {
                let Some(view) = self.visible_code_review() else {
                    return false;
                };
                let delta = if key == "]" { 1 } else { -1 };
                view.update(cx, |view, cx| view.step_file(delta, cx));
            }
            "v" if self.work_inbox.detail_tab == DetailTab::Code => {
                self.toggle_current_file_viewed(cx)
            }
            "c" | "w" | "o" | "a" => {
                let Some(item) = self.selected_work_item().cloned() else {
                    return false;
                };
                match key.as_str() {
                    "o" => cx.open_url(&item.url),
                    "a" => self.open_inbox_composer(cx),
                    _ if item.pull.is_none() => return false,
                    "c" => self.checkout_pull(&item.url, window, cx),
                    _ => self.open_pull_worktree(&item.url, window, cx),
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Keys of the History tab: walk commits, close one, open it on GitHub.
    fn handle_history_shortcut(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "j" | "down" => self.step_history_commit(1, cx),
            "k" | "up" => self.step_history_commit(-1, cx),
            "escape" | "esc" if self.work_inbox.history.selected.is_some() => {
                self.select_history_commit(None, cx)
            }
            "o" => {
                let (Some(sha), Some(repository)) = (
                    self.work_inbox.history.selected.clone(),
                    self.history_github_repository(),
                ) else {
                    return false;
                };
                cx.open_url(&format!("https://github.com/{repository}/commit/{sha}"));
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Select the next or previous task in display order and keep it visible.
    fn step_inbox_item(&mut self, delta: isize, cx: &mut Context<Self>) {
        let items = self.ordered_inbox_items();
        if items.is_empty() {
            return;
        }
        let current = self
            .work_inbox
            .selected
            .as_deref()
            .and_then(|url| items.iter().position(|item| item.url == url));
        let next = current.map_or(0, |index| {
            index.saturating_add_signed(delta).min(items.len() - 1)
        });
        let url = items[next].url.clone();
        drop(items);
        self.select_work_item(&url, cx);
        if let Some(index) = self.inbox_card_child_index(&url) {
            self.work_inbox.list_scroll.scroll_to_item(index);
        }
    }

    /// Position of a task card among the list's children (messages, group
    /// headings, and cards), as `inbox_task_list` lays them out.
    fn inbox_card_child_index(&self, url: &str) -> Option<usize> {
        let groups = self.grouped_inbox_items();
        let grouped = groups.len() > 1;
        let mut index = usize::from(self.work_inbox.github.error.is_some());
        for (relation, items) in groups {
            index += usize::from(grouped);
            if grouped && self.work_inbox.collapsed_groups.contains(&relation) {
                continue;
            }
            if let Some(position) = items.iter().position(|item| item.url == url) {
                return Some(index + position);
            }
            index += items.len();
        }
        None
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn message(text: &str, error: bool) -> gpui::Div {
    div()
        .px_2()
        .py_1()
        .text_size(px(12.0))
        .text_color(if error {
            colors().warning
        } else {
            colors().muted
        })
        .child(text.to_owned())
}
