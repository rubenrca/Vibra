//! Timeline rows for the virtualized chat list, and the bookkeeping that keeps
//! the GPUI list in step with `ChatTimeline::items`.
//!
//! Each item keeps its own row, so row and item indices match and appends never
//! shift rows. Consecutive tool calls form a group: the first call (the group's
//! leader) draws the whole group and the others draw nothing. A change to any
//! call therefore re-measures the leader's row (see `row_of`).

mod cards;

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, relative, svg};

use super::AgentChatView;
use super::layout::COLUMN_WIDTH;
use super::tool_card::is_running;
use crate::domain::agent_chat::{
    ChatCommand, ChatEvent, ChatItem, ChatTimeline, PermissionResolution, ToolContent,
};
use crate::ui::markdown::markdown_prose;
use crate::ui::theme::{colors, surface_tint};

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
        ChatEvent::ToolCall(call) => tool_rows(items, &call.id),
        ChatEvent::ToolCallUpdate(update) => tool_rows(items, &update.id),
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
        | ChatEvent::Error { .. } => Vec::new(),
    }
}

/// Rows to re-measure for the tool call `id`. A call that is not in the
/// timeline yet is appended, so it only matters when it joins a group at the end.
fn tool_rows(items: &[ChatItem], id: &str) -> Vec<usize> {
    let index = items
        .iter()
        .position(|item| matches!(item, ChatItem::ToolCall(call) if call.id == id))
        .or_else(|| {
            items
                .len()
                .checked_sub(1)
                .filter(|&last| is_tool(&items[last]))
        });
    index
        .map(|index| row_of(items, index))
        .into_iter()
        .collect()
}

/// The row that draws `items[index]`: a tool call's group leader, otherwise the item itself.
pub(super) fn row_of(items: &[ChatItem], index: usize) -> usize {
    if is_tool(&items[index]) {
        group_start(items, index)
    } else {
        index
    }
}

/// First item of the tool group that contains the tool call at `index`.
pub(super) fn group_start(items: &[ChatItem], index: usize) -> usize {
    let mut start = index;
    while start > 0 && is_tool(&items[start - 1]) {
        start -= 1;
    }
    start
}

/// One past the last item of the tool group that starts at `start`.
pub(super) fn group_end(items: &[ChatItem], start: usize) -> usize {
    start
        + items[start..]
            .iter()
            .take_while(|item| is_tool(item))
            .count()
}

fn is_tool(item: &ChatItem) -> bool {
    matches!(item, ChatItem::ToolCall(_))
}

impl AgentChatView {
    /// Brings the GPUI list in step with the timeline after a change. Splicing
    /// only the changed rows keeps a scrolled-up reader in place; the list stays
    /// pinned to the newest item while the reader is at the bottom.
    pub(super) fn sync_list(&mut self, before: usize, dirty: &[usize]) {
        let after = self.timeline.items.len();
        if self.list_state.item_count() != before || after < before {
            self.list_state.reset(after);
            return;
        }
        let mut dirty: Vec<usize> = dirty
            .iter()
            .copied()
            .filter(|index| *index < before)
            .collect();
        dirty.sort_unstable();
        dirty.dedup();
        for index in dirty {
            self.list_state.splice(index..index + 1, 1);
        }
        if after > before {
            self.list_state.splice(before..before, after - before);
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

    /// One list row. The row owns the vertical rhythm and centers its content
    /// in the transcript column, so the list still scrolls across the full
    /// width. A group member draws nothing because its leader draws the group.
    pub(super) fn render_item(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let items = &self.timeline.items;
        let Some(item) = items.get(index) else {
            return div().into_any_element();
        };
        let content = match item {
            ChatItem::UserMessage { text, files } => user_message(text, files),
            ChatItem::AssistantMessage { text } => div()
                .w_full()
                .child(markdown_prose(&format!("chat-assistant-{index}"), text))
                .into_any_element(),
            ChatItem::Reasoning { text } => self.reasoning(index, text, cx),
            ChatItem::ToolCall(_) => {
                if group_start(items, index) != index {
                    return div().into_any_element();
                }
                self.tool_group(index, group_end(items, index), cx)
            }
            ChatItem::Plan(entries) => cards::plan(entries),
            ChatItem::Permission(permission) => self.permission_card(index, permission, cx),
            ChatItem::Notice { level, text } => cards::notice(*level, text),
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .pb_4()
            .child(div().w_full().max_w(px(COLUMN_WIDTH)).px_3().child(content))
            .into_any_element()
    }

    fn reasoning(&self, index: usize, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("thought-{index}");
        let expanded = self.expanded.contains(&key);
        let header = div()
            .id(SharedString::from(format!("chat-thought-{index}")))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(colors().muted)
            .hover(|row| row.text_color(colors().foreground))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(key.clone(), index, cx);
            }))
            .child(
                svg()
                    .path(if expanded {
                        "chrome-icons/chevron-down.svg"
                    } else {
                        "chrome-icons/chevron-right.svg"
                    })
                    .size(px(12.0))
                    .flex_none()
                    .text_color(colors().subtle),
            )
            .child("Thinking");
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(header)
            .when(expanded, |column| {
                column.child(
                    div()
                        .pl_5()
                        .text_size(px(12.5))
                        .line_height(px(18.0))
                        .text_color(colors().muted)
                        .children(
                            text.lines()
                                .map(|line| div().min_h(px(18.0)).child(line.to_owned())),
                        ),
                )
            })
            .into_any_element()
    }
}

fn user_message(text: &str, files: &[String]) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap_1p5()
        .when(!files.is_empty(), |row| {
            row.child(
                div()
                    .max_w(relative(0.8))
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap_1()
                    .children(files.iter().map(|file| {
                        div()
                            .px_2()
                            .py(px(1.0))
                            .rounded(px(5.0))
                            .bg(colors().hover)
                            .font_family(crate::ui::theme::MONO_FONT)
                            .text_size(px(11.0))
                            .text_color(colors().muted)
                            .child(file.clone())
                    })),
            )
        })
        .child(
            // A quiet fill and no border: the bubble is text first, not a card.
            div()
                .max_w(relative(0.8))
                .px_3()
                .py_2()
                .rounded(px(12.0))
                .bg(surface_tint(colors().elevated, colors().terminal))
                .text_size(px(14.0))
                .line_height(px(22.0))
                .text_color(colors().foreground)
                .children(
                    text.lines()
                        .map(|line| div().min_h(px(22.0)).child(line.to_owned())),
                ),
        )
        .into_any_element()
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
        assert_eq!(dirty_indices(&timeline, &finished), vec![1, 3]);
        let started = ChatEvent::SessionStarted {
            session_id: "s".into(),
            loaded: false,
        };
        assert!(dirty_indices(&timeline, &started).is_empty());
    }

    #[test]
    fn groups_run_from_their_leader_to_the_last_consecutive_call() {
        let items = vec![tool("t1"), tool("t2"), message("a"), tool("t3")];
        assert_eq!(group_start(&items, 1), 0);
        assert_eq!(group_end(&items, 0), 2);
        assert_eq!(group_start(&items, 3), 3);
        assert_eq!(group_end(&items, 3), 4);
        assert_eq!(row_of(&items, 1), 0);
        assert_eq!(row_of(&items, 2), 2);
    }
}
