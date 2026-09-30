//! Native Inbox coordination. Layout and detail actions follow MonoCode’s Inbox.

mod connection;
mod detail;
mod feed;
mod launch;
mod layout;
mod review;
mod state;

pub(crate) use crate::domain::work_items::{
    PrAction, WorkCheck, WorkDiff, WorkFilter, WorkItem, WorkKind, WorkSource, WorkStatus,
};
pub(crate) use crate::infrastructure::work_items;
pub(crate) use crate::ui::theme::colors;
pub(crate) use connection::message;
pub(crate) use gpui::{
    AnyElement, ClipboardItem, Context, SharedString, Window, div, prelude::*, px,
};
pub(crate) use state::*;
pub(crate) use uuid::Uuid;

use crate::ui::text_edit::{TextKeyOutcome, apply_text_key};
use gpui::KeyDownEvent;

pub(crate) use super::navigation::section_button;
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
        if matches!(event.keystroke.key.as_str(), "escape" | "esc")
            && self.work_inbox.menu.take().is_some()
        {
            cx.notify();
            return true;
        }
        let search = self.work_inbox.search_editing;
        let composer = self.work_inbox.composer_editing && self.work_inbox.composer_open;
        let comment = self.work_inbox.comment_editing;
        let buffer = if search {
            &mut self.work_inbox.filter.query
        } else if composer {
            &mut self.work_inbox.composer_note
        } else if comment {
            let Some(url) = self.work_inbox.selected.clone() else {
                return false;
            };
            let state = self.work_inbox.details.entry(url).or_default();
            if state.posting {
                return false;
            }
            &mut state.draft
        } else {
            return false;
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
            TextKeyOutcome::Submit if composer => self.start_work_item(window, cx),
            TextKeyOutcome::Submit
            | TextKeyOutcome::Cancel
            | TextKeyOutcome::NextField
            | TextKeyOutcome::PreviousField => {
                self.work_inbox.search_editing = false;
                self.work_inbox.composer_editing = false;
                self.work_inbox.comment_editing = false;
            }
            TextKeyOutcome::Unhandled => return false,
        }
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests;
