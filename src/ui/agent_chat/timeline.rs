//! Timeline rows for the virtualized chat list, and the bookkeeping that keeps
//! the GPUI list in step with `ChatTimeline::items`.
//!
//! Each item keeps its own row, so row and item indices match and appends never
//! shift rows. What a row draws comes from `turns::plan_rows`: consecutive work
//! items (tool calls, reasoning) form a run whose first item (the leader) draws
//! the whole run, and a completed turn hides its work behind its header. A
//! change to any work item therefore re-measures the leader's row (see
//! `row_of`), and a change to the plan re-measures the rows whose plan changed.

mod assistant_row;
mod cards;
mod user_row;

use gpui::{AnyElement, Context, div, prelude::*, px};

use super::AgentChatView;
use super::layout::COLUMN_WIDTH;
use super::tool_card::is_running;
use super::turns::{Row, is_work, plan_rows, run_start, turn_range};
use crate::domain::agent_chat::{
    ChatCommand, ChatEvent, ChatItem, ChatTimeline, PermissionResolution, ToolContent, TurnOutcome,
};

/// Rows that `event` may change when applied, sorted and without repeats.
///
/// Must run before `ChatTimeline::apply`. Items the event appends are not
/// listed; the caller learns about them from the item count.
pub(super) fn dirty_indices(timeline: &ChatTimeline, event: &ChatEvent) -> Vec<usize> {
    let mut rows = changed_rows(timeline, event);
    rows.sort_unstable();
    rows.dedup();
    rows
}

fn changed_rows(timeline: &ChatTimeline, event: &ChatEvent) -> Vec<usize> {
    let items = &timeline.items;
    match event {
        ChatEvent::UserMessageChunk { .. }
        | ChatEvent::AgentMessageChunk { .. }
        | ChatEvent::ThoughtChunk { .. } => items
            .len()
            .checked_sub(1)
            .map(|last| row_of(items, last))
            .into_iter()
            .collect(),
        ChatEvent::ToolCall(call) => tool_rows(timeline, &call.id),
        ChatEvent::ToolCallUpdate(update) => tool_rows(timeline, &update.id),
        ChatEvent::Plan(_) => items
            .iter()
            .rposition(|item| matches!(item, ChatItem::Plan(_)))
            .into_iter()
            .collect(),
        ChatEvent::TerminalOutput { terminal_id, .. } => items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                matches!(item, ChatItem::ToolCall(call) if call.content.iter().any(|content| {
                    matches!(content, ToolContent::Terminal { terminal_id: id } if id == terminal_id)
                }))
            })
            .map(|(index, _)| row_of(items, index))
            .collect(),
        // Ending a turn cancels open permissions and stops the running tools' spinners.
        ChatEvent::TurnFinished { .. } | ChatEvent::Exited { .. } => items
            .iter()
            .enumerate()
            .filter(|(_, item)| match item {
                ChatItem::Permission(_) => true,
                ChatItem::ToolCall(call) => is_running(call.status),
                _ => false,
            })
            .map(|(index, _)| row_of(items, index))
            .collect(),
        ChatEvent::Initialized { .. }
        | ChatEvent::SessionStarted { .. }
        | ChatEvent::PermissionRequested(_)
        | ChatEvent::Modes(_)
        | ChatEvent::ConfigOptions(_)
        | ChatEvent::Title(_)
        | ChatEvent::Usage(_)
        | ChatEvent::Commands(_)
        | ChatEvent::Error { .. }
        | ChatEvent::Notice { .. } => Vec::new(),
    }
}

/// Rows to re-measure for the tool call `id`. A call that is not in the
/// timeline yet is appended, so it only matters when it joins a group at the end.
/// A late update to an ended turn also re-measures its final answer, which
/// lists the turn's changed files.
fn tool_rows(timeline: &ChatTimeline, id: &str) -> Vec<usize> {
    let items = &timeline.items;
    let index = items
        .iter()
        .position(|item| matches!(item, ChatItem::ToolCall(call) if call.id == id))
        .or_else(|| {
            items
                .len()
                .checked_sub(1)
                .filter(|&last| is_work(&items[last]))
        });
    let Some(index) = index else {
        return Vec::new();
    };
    let mut rows = vec![row_of(items, index)];
    rows.extend(final_answer_of_ended_turn(timeline, index));
    rows
}

/// The last agent message of the ended turn that contains `items[index]`.
fn final_answer_of_ended_turn(timeline: &ChatTimeline, index: usize) -> Option<usize> {
    let turn = timeline
        .turns
        .iter()
        .rposition(|turn| turn.start <= index)?;
    if timeline.turns[turn].outcome == TurnOutcome::Running {
        return None;
    }
    turn_range(timeline, turn)
        .rev()
        .find(|&item| matches!(timeline.items[item], ChatItem::AssistantMessage { .. }))
}

/// The row that draws `items[index]`: a work item's run leader, otherwise the item itself.
pub(super) fn row_of(items: &[ChatItem], index: usize) -> usize {
    if is_work(&items[index]) {
        run_start(items, index)
    } else {
        index
    }
}

impl AgentChatView {
    /// Brings the GPUI list in step with the timeline after a change: rebuilds
    /// the row plan, then re-measures the `dirty` rows and every row whose plan
    /// changed. Splicing only those rows keeps a scrolled-up reader in place;
    /// the list stays pinned to the newest item while the reader is at the bottom.
    pub(super) fn sync_list(&mut self, before: usize, dirty: &[usize]) {
        let rows = plan_rows(&self.timeline, &self.expanded);
        let after = rows.len();
        if self.list_state.item_count() != before || after < before {
            self.rows = rows;
            self.list_state.reset(after);
            self.scrolled_away = false;
            return;
        }
        let mut changed: Vec<usize> = dirty
            .iter()
            .copied()
            .chain((0..before).filter(|&index| self.rows.get(index) != rows.get(index)))
            .filter(|index| *index < before)
            .collect();
        changed.sort_unstable();
        changed.dedup();
        // GPUI's splice resets the offset inside a replaced row. A reader can
        // be partway through a long streaming message, so retain that offset.
        let anchor = self
            .scrolled_away
            .then(|| self.list_state.logical_scroll_top());
        self.rows = rows;
        for index in changed {
            self.list_state.splice(index..index + 1, 1);
        }
        if after > before {
            self.list_state.splice(before..before, after - before);
        }
        if let Some(anchor) = anchor {
            self.list_state.scroll_to(anchor);
        }
    }

    /// Expands or collapses a group, reasoning block or tool content and
    /// re-measures its row. `row` is the row that draws it.
    pub(super) fn toggle_expanded(&mut self, key: String, row: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        let count = self.timeline.items.len();
        self.sync_list(count, &[row]);
        cx.notify();
    }

    /// Answers a permission request from the card and tells the agent.
    pub(super) fn answer_permission(
        &mut self,
        request_id: u64,
        option_id: String,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let resolution = PermissionResolution::Selected(option_id);
        if !self
            .timeline
            .resolve_permission(request_id, resolution.clone())
        {
            return;
        }
        self.send(
            ChatCommand::ResolvePermission {
                request_id,
                resolution,
            },
            cx,
        );
        let count = self.timeline.items.len();
        self.sync_list(count, &[index]);
        cx.notify();
    }

    /// One list row, as the row plan says. The row owns the vertical rhythm
    /// (the space under it) and centers its content in the transcript column,
    /// so the list still scrolls across the full width. A hidden row draws
    /// nothing and takes no space.
    pub(super) fn render_item(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let (Some(item), Some(row)) = (self.timeline.items.get(index), self.rows.get(index)) else {
            return div().into_any_element();
        };
        let (content, space_below) = match (row, item) {
            (Row::User { turn, header }, ChatItem::UserMessage { text, files }) => {
                (self.user_row(index, text, files, *turn, header, cx), 12.0)
            }
            (Row::Assistant { final_of }, ChatItem::AssistantMessage { text }) => {
                (self.assistant_row(index, text, *final_of, cx), 16.0)
            }
            (Row::Work { end, live }, _) => (self.work_row(index, *end, *live, cx), 6.0),
            (Row::Plan, ChatItem::Plan(entries)) => (cards::plan(entries), 12.0),
            (Row::Notice, ChatItem::Notice { level, text }) => (cards::notice(*level, text), 12.0),
            _ => return div().into_any_element(),
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .pb(px(space_below))
            .child(div().w_full().max_w(px(COLUMN_WIDTH)).px_3().child(content))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::{
        PermissionItem, ToolCallItem, ToolCallUpdate, ToolKind, ToolStatus,
    };

    fn call(id: &str, content: Vec<ToolContent>) -> ToolCallItem {
        ToolCallItem {
            id: id.to_owned(),
            title: id.to_owned(),
            kind: ToolKind::Execute,
            status: ToolStatus::InProgress,
            content,
            locations: Vec::new(),
        }
    }

    fn tool(id: &str) -> ChatItem {
        ChatItem::ToolCall(call(id, Vec::new()))
    }

    fn message(text: &str) -> ChatItem {
        ChatItem::AssistantMessage {
            text: text.to_owned(),
        }
    }

    fn timeline_with(items: Vec<ChatItem>) -> ChatTimeline {
        let mut timeline = ChatTimeline::default();
        timeline.items = items;
        timeline
    }

    fn update(id: &str) -> ChatEvent {
        ChatEvent::ToolCallUpdate(ToolCallUpdate {
            id: id.into(),
            ..Default::default()
        })
    }

    #[test]
    fn chunks_only_touch_the_last_item() {
        let timeline = timeline_with(vec![
            ChatItem::UserMessage {
                files: Vec::new(),
                text: "hi".into(),
            },
            message("a"),
        ]);
        let chunk = ChatEvent::AgentMessageChunk { text: "b".into() };
        assert_eq!(dirty_indices(&timeline, &chunk), vec![1]);
        assert!(dirty_indices(&ChatTimeline::default(), &chunk).is_empty());
    }

    #[test]
    fn tool_updates_and_terminal_output_mark_the_group_leader() {
        let timeline = timeline_with(vec![
            message("a"),
            tool("t1"),
            ChatItem::ToolCall(call(
                "t2",
                vec![ToolContent::Terminal {
                    terminal_id: "term".into(),
                }],
            )),
        ]);
        assert_eq!(dirty_indices(&timeline, &update("t1")), vec![1]);
        assert_eq!(dirty_indices(&timeline, &update("t2")), vec![1]);
        let output = ChatEvent::TerminalOutput {
            terminal_id: "term".into(),
            output: "ok".into(),
            exit_status: None,
        };
        assert_eq!(dirty_indices(&timeline, &output), vec![1]);
    }

    #[test]
    fn a_call_after_a_message_starts_its_own_group() {
        let timeline = timeline_with(vec![tool("t1"), message("a"), tool("t2")]);
        assert_eq!(dirty_indices(&timeline, &update("t2")), vec![2]);
        assert_eq!(dirty_indices(&timeline, &update("t1")), vec![0]);
    }

    #[test]
    fn a_new_call_remeasures_only_the_group_it_joins() {
        let joins = timeline_with(vec![
            ChatItem::UserMessage {
                files: Vec::new(),
                text: "go".into(),
            },
            tool("t1"),
        ]);
        let new_call = ChatEvent::ToolCall(call("t2", Vec::new()));
        assert_eq!(dirty_indices(&joins, &new_call), vec![1]);

        let starts = timeline_with(vec![ChatItem::UserMessage {
            files: Vec::new(),
            text: "go".into(),
        }]);
        assert!(dirty_indices(&starts, &new_call).is_empty());
    }

    #[test]
    fn ending_a_turn_revisits_open_permissions_and_running_groups() {
        let permission = ChatItem::Permission(PermissionItem {
            request_id: 1,
            tool_call_id: None,
            title: "Run tests?".into(),
            options: Vec::new(),
            resolution: None,
        });
        let timeline = timeline_with(vec![
            ChatItem::UserMessage {
                files: Vec::new(),
                text: "go".into(),
            },
            tool("t1"),
            tool("t2"),
            permission,
        ]);
        let finished = ChatEvent::TurnFinished {
            stop_reason: "end_turn".into(),
        };
        // The permission joins the tools' run, so the run leader covers both.
        assert_eq!(dirty_indices(&timeline, &finished), vec![1]);
        let started = ChatEvent::SessionStarted {
            session_id: "s".into(),
            loaded: false,
        };
        assert!(dirty_indices(&timeline, &started).is_empty());
    }

    #[test]
    fn a_late_tool_update_remeasures_the_ended_turns_answer() {
        let mut timeline = ChatTimeline::default();
        timeline.push_user_prompt("go".into(), Vec::new());
        timeline.apply(ChatEvent::ToolCall(call("t1", Vec::new())));
        timeline.apply(ChatEvent::AgentMessageChunk {
            text: "done".into(),
        });
        assert_eq!(dirty_indices(&timeline, &update("t1")), vec![1]);
        timeline.apply(ChatEvent::TurnFinished {
            stop_reason: "end_turn".into(),
        });
        assert_eq!(dirty_indices(&timeline, &update("t1")), vec![1, 2]);
    }

    #[test]
    fn work_items_are_drawn_by_their_run_leader() {
        let items = vec![
            tool("t1"),
            ChatItem::Reasoning { text: "hm".into() },
            message("a"),
            tool("t3"),
        ];
        assert_eq!(row_of(&items, 1), 0);
        assert_eq!(row_of(&items, 2), 2);
        assert_eq!(row_of(&items, 3), 3);
    }
}
