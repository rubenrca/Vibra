//! What each timeline row draws, derived from the items and their turns.
//!
//! A turn is a user message and everything up to the next one. While it runs,
//! its header says "Working for …" and its trailing work (tool calls and
//! reasoning) collapses into one live row. Once it completes, its work and
//! commentary fold behind "Worked for …", leaving the prompt, the final answer
//! and any notices. A stopped turn says so and stays open; a failed one stays
//! open with no header.
//!
//! Pure: the view caches the result and re-measures the rows that changed.

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use crate::domain::agent_chat::{ChatItem, ChatTimeline, TurnOutcome, TurnRecord};

/// The line under a user message that describes its turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TurnHeader {
    /// The turn is running. `since` drives "Working for …"; `None` for replayed history.
    Working { since: Option<SystemTime> },
    /// Completed with work to fold: "Worked for …", a toggle. `folded` is false
    /// once the reader expands it.
    Worked {
        elapsed: Option<Duration>,
        folded: bool,
    },
    /// The user stopped it: "You stopped after …". Nothing folds.
    Stopped { elapsed: Option<Duration> },
    /// Nothing to say (a failed turn, or a completed one with nothing to fold).
    None,
}

/// What one list row draws. Every item has a row, so row and item indices match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Row {
    /// Folded away, or drawn by another row (a work run's later items).
    Hidden,
    /// A user message and its turn's header. `turn` indexes `ChatTimeline::turns`.
    User {
        turn: usize,
        header: TurnHeader,
    },
    /// An agent message. `final_of` is the turn it answers once that turn has
    /// ended (it carries the turn's changed files); `None` for commentary or a
    /// turn still running.
    Assistant {
        final_of: Option<usize>,
    },
    /// Leader of a work run, `items[index..end]`. `live` while it is the running
    /// turn's trailing activity.
    Work {
        end: usize,
        live: bool,
    },
    Plan,
    Notice,
}

/// Items that join a work run. Permission items join too, so an approval in the
/// middle of tool calls does not split them; the composer shows pending ones.
pub(super) fn is_work(item: &ChatItem) -> bool {
    matches!(
        item,
        ChatItem::ToolCall(_) | ChatItem::Reasoning { .. } | ChatItem::Permission(_)
    )
}

/// First item of the work run that contains `items[index]`.
pub(super) fn run_start(items: &[ChatItem], index: usize) -> usize {
    let mut start = index;
    while start > 0 && is_work(&items[start - 1]) {
        start -= 1;
    }
    start
}

/// One past the last item of the work run that starts at `start`.
pub(super) fn run_end(items: &[ChatItem], start: usize) -> usize {
    start
        + items[start..]
            .iter()
            .take_while(|item| is_work(item))
            .count()
}

/// Key in `AgentChatView::expanded` for a completed turn the reader unfolded.
pub(super) fn turn_key(start: usize) -> String {
    format!("turn-{start}")
}

/// Items `turn` spans: its user message up to the next turn's.
pub(super) fn turn_range(timeline: &ChatTimeline, turn: usize) -> std::ops::Range<usize> {
    let start = timeline.turns[turn].start;
    let end = timeline
        .turns
        .get(turn + 1)
        .map_or(timeline.items.len(), |next| next.start);
    start..end.min(timeline.items.len())
}

/// How long `turn` ran, when both ends are known.
pub(super) fn turn_elapsed(timeline: &ChatTimeline, turn: &TurnRecord) -> Option<Duration> {
    let started = timeline.turn_started_at(turn)?;
    turn.finished_at?.duration_since(started).ok()
}

/// One `Row` per item. `expanded` holds `turn_key`s of unfolded turns.
pub(super) fn plan_rows(timeline: &ChatTimeline, expanded: &HashSet<String>) -> Vec<Row> {
    let items = &timeline.items;
    let mut rows = vec![Row::Hidden; items.len()];
    // Items before the first prompt (notices, replayed agent text) belong to no turn.
    let first = timeline
        .turns
        .first()
        .map_or(items.len(), |turn| turn.start);
    plan_span(items, 0..first.min(items.len()), false, &mut rows);
    for (index, turn) in timeline.turns.iter().enumerate() {
        let range = turn_range(timeline, index);
        if range.is_empty() {
            continue;
        }
        let body = range.start + 1..range.end;
        // The last agent message answers the prompt; earlier ones are commentary.
        let final_message = body
            .clone()
            .rev()
            .find(|&item| matches!(items[item], ChatItem::AssistantMessage { .. }));
        let ended = turn.outcome != TurnOutcome::Running;
        let foldable = turn.outcome == TurnOutcome::Completed
            && body
                .clone()
                .any(|item| folds(&items[item], Some(item) == final_message));
        let folded = foldable && !expanded.contains(&turn_key(turn.start));
        let header = match turn.outcome {
            TurnOutcome::Running => TurnHeader::Working {
                since: timeline.turn_started_at(turn),
            },
            TurnOutcome::Completed if foldable => TurnHeader::Worked {
                elapsed: turn_elapsed(timeline, turn),
                folded,
            },
            TurnOutcome::Cancelled => TurnHeader::Stopped {
                elapsed: turn_elapsed(timeline, turn),
            },
            TurnOutcome::Completed | TurnOutcome::Failed => TurnHeader::None,
        };
        rows[range.start] = Row::User {
            turn: index,
            header,
        };
        if folded {
            for item in body.clone() {
                let is_final = Some(item) == final_message;
                if !folds(&items[item], is_final) {
                    rows[item] = visible_row(items, item, false);
                }
            }
        } else {
            let live = turn.outcome == TurnOutcome::Running && timeline.is_working();
            plan_span(items, body.clone(), live, &mut rows);
        }
        if ended && let Some(item) = final_message {
            rows[item] = Row::Assistant {
                final_of: Some(index),
            };
        }
    }
    rows
}

/// Whether a folded turn hides `item`. The answer and notices stay.
fn folds(item: &ChatItem, is_final: bool) -> bool {
    match item {
        ChatItem::AssistantMessage { .. } => !is_final,
        ChatItem::Notice { .. } | ChatItem::UserMessage { .. } => false,
        ChatItem::ToolCall(_)
        | ChatItem::Reasoning { .. }
        | ChatItem::Permission(_)
        | ChatItem::Plan(_) => true,
    }
}

/// Rows for an unfolded span. Work runs are cut at the span's end so a run
/// never crosses into the next turn. A run reaching the end of the timeline is
/// live when `live_tail` is set.
fn plan_span(items: &[ChatItem], span: std::ops::Range<usize>, live_tail: bool, rows: &mut [Row]) {
    let mut index = span.start;
    while index < span.end {
        if is_work(&items[index]) {
            let end = run_end(items, index).min(span.end);
            let live = live_tail && end == items.len();
            rows[index] = Row::Work { end, live };
            index = end;
        } else {
            rows[index] = visible_row(items, index, false);
            index += 1;
        }
    }
}

fn visible_row(items: &[ChatItem], index: usize, live: bool) -> Row {
    match &items[index] {
        // A user message outside any turn cannot happen; draw it as a hidden row.
        ChatItem::UserMessage { .. } => Row::Hidden,
        ChatItem::AssistantMessage { .. } => Row::Assistant { final_of: None },
        ChatItem::ToolCall(_) | ChatItem::Reasoning { .. } | ChatItem::Permission(_) => Row::Work {
            end: index + 1,
            live,
        },
        ChatItem::Plan(_) => Row::Plan,
        ChatItem::Notice { .. } => Row::Notice,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::{ChatEvent, NoticeLevel, ToolCallItem, ToolKind, ToolStatus};

    fn tool(id: &str) -> ChatEvent {
        ChatEvent::ToolCall(ToolCallItem {
            id: id.to_owned(),
            title: id.to_owned(),
            kind: ToolKind::Read,
            status: ToolStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
        })
    }

    fn say(text: &str) -> ChatEvent {
        ChatEvent::AgentMessageChunk {
            text: text.to_owned(),
        }
    }

    fn think(text: &str) -> ChatEvent {
        ChatEvent::ThoughtChunk {
            text: text.to_owned(),
        }
    }

    fn finished(reason: &str) -> ChatEvent {
        ChatEvent::TurnFinished {
            stop_reason: reason.to_owned(),
        }
    }

    /// A live session with one prompt, then `events`.
    fn turn(events: Vec<ChatEvent>) -> ChatTimeline {
        let mut timeline = ChatTimeline::default();
        timeline.apply(ChatEvent::SessionStarted {
            session_id: "s".to_owned(),
            loaded: false,
        });
        timeline.push_user_prompt("go".to_owned(), Vec::new());
        for event in events {
            timeline.apply(event);
        }
        timeline
    }

    fn header(rows: &[Row]) -> &TurnHeader {
        match &rows[0] {
            Row::User { header, .. } => header,
            row => panic!("expected a user row, got {row:?}"),
        }
    }

    #[test]
    fn a_running_turn_groups_thoughts_and_tools_into_one_live_run() {
        let timeline = turn(vec![think("hmm"), tool("a"), tool("b")]);
        let rows = plan_rows(&timeline, &HashSet::new());
        assert!(matches!(header(&rows), TurnHeader::Working { .. }));
        assert_eq!(
            rows[1..],
            [Row::Work { end: 4, live: true }, Row::Hidden, Row::Hidden]
        );
    }

    #[test]
    fn work_followed_by_commentary_is_settled_while_the_tail_stays_live() {
        let timeline = turn(vec![tool("a"), say("checking"), tool("b")]);
        let rows = plan_rows(&timeline, &HashSet::new());
        assert_eq!(
            rows[1..],
            [
                Row::Work {
                    end: 2,
                    live: false
                },
                Row::Assistant { final_of: None },
                Row::Work { end: 4, live: true },
            ]
        );
    }

    #[test]
    fn a_completed_turn_folds_all_but_the_answer_and_notices() {
        let timeline = turn(vec![
            tool("a"),
            say("checking"),
            ChatEvent::Error {
                message: "transient".to_owned(),
            },
            tool("b"),
            say("done"),
            finished("end_turn"),
        ]);
        // The error notice makes the turn fail, so it stays open.
        let rows = plan_rows(&timeline, &HashSet::new());
        assert_eq!(header(&rows), &TurnHeader::None);

        let timeline = turn(vec![
            tool("a"),
            say("checking"),
            tool("b"),
            say("done"),
            finished("end_turn"),
        ]);
        let rows = plan_rows(&timeline, &HashSet::new());
        assert_eq!(
            header(&rows),
            &TurnHeader::Worked {
                elapsed: None,
                folded: true
            }
        );
        assert_eq!(
            rows[1..],
            [
                Row::Hidden,
                Row::Hidden,
                Row::Hidden,
                Row::Assistant { final_of: Some(0) },
            ]
        );
    }

    #[test]
    fn an_unfolded_turn_shows_its_work_settled() {
        let timeline = turn(vec![tool("a"), say("done"), finished("end_turn")]);
        let expanded = HashSet::from([turn_key(0)]);
        let rows = plan_rows(&timeline, &expanded);
        assert_eq!(
            header(&rows),
            &TurnHeader::Worked {
                elapsed: None,
                folded: false
            }
        );
        assert_eq!(
            rows[1..],
            [
                Row::Work {
                    end: 2,
                    live: false
                },
                Row::Assistant { final_of: Some(0) },
            ]
        );
    }

    #[test]
    fn a_plain_answer_has_nothing_to_fold() {
        let timeline = turn(vec![say("hi"), finished("end_turn")]);
        let rows = plan_rows(&timeline, &HashSet::new());
        assert_eq!(header(&rows), &TurnHeader::None);
        assert_eq!(rows[1], Row::Assistant { final_of: Some(0) });
    }

    #[test]
    fn a_stopped_turn_stays_open() {
        let timeline = turn(vec![tool("a"), finished("cancelled")]);
        let rows = plan_rows(&timeline, &HashSet::new());
        assert_eq!(header(&rows), &TurnHeader::Stopped { elapsed: None });
        assert_eq!(
            rows[1],
            Row::Work {
                end: 2,
                live: false
            }
        );
    }

    #[test]
    fn work_runs_stop_at_the_next_turn() {
        let mut timeline = turn(vec![tool("a"), finished("cancelled")]);
        timeline.push_user_prompt("again".to_owned(), Vec::new());
        timeline.apply(tool("b"));
        let rows = plan_rows(&timeline, &HashSet::new());
        assert_eq!(
            rows[1],
            Row::Work {
                end: 2,
                live: false
            }
        );
        assert!(matches!(rows[2], Row::User { turn: 1, .. }));
        assert_eq!(rows[3], Row::Work { end: 4, live: true });
    }

    #[test]
    fn notices_before_any_prompt_are_visible() {
        let mut timeline = ChatTimeline::default();
        timeline.apply(ChatEvent::Error {
            message: "no auth".to_owned(),
        });
        let rows = plan_rows(&timeline, &HashSet::new());
        assert_eq!(rows, vec![Row::Notice]);
        assert!(matches!(
            timeline.items[0],
            ChatItem::Notice {
                level: NoticeLevel::Error,
                ..
            }
        ));
    }

    #[test]
    fn elapsed_time_comes_from_the_stamps() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let mut timeline = turn(Vec::new());
        timeline.stamp(start);
        timeline.apply(tool("a"));
        timeline.apply(say("done"));
        timeline.apply(finished("end_turn"));
        timeline.stamp(start + Duration::from_secs(80));
        let rows = plan_rows(&timeline, &HashSet::new());
        assert_eq!(
            header(&rows),
            &TurnHeader::Worked {
                elapsed: Some(Duration::from_secs(80)),
                folded: true
            }
        );
    }
}
