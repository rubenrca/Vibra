//! A user message and, under it, its turn's header line.

use std::time::{Duration, SystemTime};

use gpui::{
    Animation, AnimationExt, AnyElement, ClipboardItem, Context, SharedString, div, prelude::*,
    pulsating_between, px, relative, svg,
};

use super::super::AgentChatView;
use super::super::layout::working::{clock_time, format_elapsed};
use super::super::tooltip::tooltip;
use super::super::turns::{TurnHeader, turn_key};
use crate::ui::theme::{colors, surface_tint};

/// A prompt with more lines than this is collapsed to this many lines.
const COLLAPSED_LINES: usize = 8;
/// A prompt with more characters than this is collapsed to at most this many.
const COLLAPSED_CHARS: usize = 600;

impl AgentChatView {
    /// The prompt bubble, right-aligned, with a copy action under it and the
    /// turn header ("Working for …", "Worked for …", "You stopped after …")
    /// below when it has one. `turn` indexes `ChatTimeline::turns`; `index` is
    /// the item's row.
    pub(in super::super) fn user_row(
        &self,
        index: usize,
        text: &str,
        files: &[String],
        turn: usize,
        header: &TurnHeader,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("user-{index}");
        let preview = collapsed_text(text);
        let unfolded = self.expanded.contains(&key);
        let shown = match &preview {
            Some(preview) if !unfolded => preview.as_str(),
            _ => text,
        };
        let group = format!("chat-user-{index}");
        let copied = text.to_owned();
        let sent_at = self.timeline.item_time(index).and_then(clock_time);
        let header_line = header_label(header, SystemTime::now())
            .map(|label| self.prompt_turn_header(index, turn, header, label, cx));

        div()
            .group(group.clone())
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(prompt_bubble(shown, files))
            .when_some(preview, |row, _| {
                row.child(
                    div().w_full().flex().justify_end().child(
                        div()
                            .id(SharedString::from(format!("chat-user-toggle-{index}")))
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(colors().muted)
                            .hover(|link| link.text_color(colors().foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_expanded(key.clone(), index, cx);
                            }))
                            .child(if unfolded { "Show less" } else { "Show more" }),
                    ),
                )
            })
            // Always takes its space; the time and copy button fade in while the row is hovered.
            .child(
                div()
                    .w_full()
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .opacity(0.0)
                    .group_hover(group, |actions| actions.opacity(1.0))
                    .when_some(sent_at, |actions, time| {
                        actions.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors().subtle)
                                .child(time),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("chat-user-copy-{index}")))
                            .size(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .hover(|button| button.bg(colors().hover))
                            .tooltip(|_, cx| tooltip("Copy", cx))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                            })
                            .child(
                                svg()
                                    .path("chrome-icons/copy.svg")
                                    .size(px(13.0))
                                    .flex_none()
                                    .text_color(colors().muted),
                            ),
                    ),
            )
            .when_some(header_line, |row, line| row.child(line))
            .into_any_element()
    }

    /// The line under a prompt that says what its turn is doing: full column
    /// width, muted, with a hairline under it. A completed turn with folded work
    /// is a toggle; `index` is the prompt's row and `turn` its turn.
    fn prompt_turn_header(
        &self,
        index: usize,
        turn: usize,
        header: &TurnHeader,
        label: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let line = div()
            .w_full()
            .h(px(26.0))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(colors().border_subtle)
            .text_size(px(13.0))
            .text_color(colors().muted);
        match header {
            TurnHeader::Working { .. } => {
                let pulse = Animation::new(Duration::from_millis(1600))
                    .repeat()
                    .with_easing(pulsating_between(0.45, 1.0));
                line.child(div().child(label).with_animation(
                    SharedString::from(format!("chat-turn-working-{index}")),
                    pulse,
                    |text, delta| text.opacity(delta),
                ))
                .into_any_element()
            }
            TurnHeader::Worked { folded, .. } => {
                let key = turn_key(self.timeline.turns[turn].start);
                let chevron = if *folded {
                    "chrome-icons/chevron-right.svg"
                } else {
                    "chrome-icons/chevron-down.svg"
                };
                line.child(
                    div()
                        .id(SharedString::from(format!("chat-turn-header-{index}")))
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .hover(|link| link.text_color(colors().foreground))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_expanded(key.clone(), index, cx);
                        }))
                        .child(label)
                        .child(
                            svg()
                                .path(chevron)
                                .size(px(12.0))
                                .flex_none()
                                .text_color(colors().subtle),
                        ),
                )
                .into_any_element()
            }
            TurnHeader::Stopped { .. } | TurnHeader::None => line.child(label).into_any_element(),
        }
    }
}

/// The file chips above the prompt bubble, both right-aligned.
fn prompt_bubble(text: &str, files: &[String]) -> AnyElement {
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
                .rounded(px(16.0))
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

/// The shortened text of a long prompt: its first `COLLAPSED_LINES` lines,
/// capped at `COLLAPSED_CHARS` characters. `None` when the prompt is short
/// enough to show whole.
fn collapsed_text(text: &str) -> Option<String> {
    let long = text.lines().count() > COLLAPSED_LINES || text.chars().count() > COLLAPSED_CHARS;
    long.then(|| {
        text.lines()
            .take(COLLAPSED_LINES)
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(COLLAPSED_CHARS)
            .collect()
    })
}

/// The turn header's text, or `None` when it draws nothing. `now` is a
/// parameter so the elapsed time can be tested.
fn header_label(header: &TurnHeader, now: SystemTime) -> Option<String> {
    match header {
        TurnHeader::Working { since: Some(since) } => {
            let elapsed = now.duration_since(*since).unwrap_or_default();
            Some(format!("Working for {}", format_elapsed(elapsed.as_secs())))
        }
        TurnHeader::Working { since: None } => Some("Working…".to_owned()),
        TurnHeader::Worked {
            elapsed: Some(elapsed),
            ..
        } => Some(format!("Worked for {}", format_elapsed(elapsed.as_secs()))),
        TurnHeader::Worked { elapsed: None, .. } => Some("Worked".to_owned()),
        TurnHeader::Stopped {
            elapsed: Some(elapsed),
        } => Some(format!(
            "You stopped after {}",
            format_elapsed(elapsed.as_secs())
        )),
        TurnHeader::Stopped { elapsed: None } => Some("You stopped this response".to_owned()),
        TurnHeader::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `count` lines reading "1", "2", …
    fn numbered_lines(count: usize) -> String {
        (1..=count)
            .map(|number| number.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn prompts_within_both_limits_are_shown_whole() {
        assert_eq!(collapsed_text("hello"), None);
        assert_eq!(collapsed_text(&numbered_lines(8)), None);
        assert_eq!(collapsed_text(&"x".repeat(600)), None);
    }

    #[test]
    fn a_prompt_over_eight_lines_keeps_its_first_eight() {
        assert_eq!(collapsed_text(&numbered_lines(9)), Some(numbered_lines(8)));
    }

    #[test]
    fn a_long_prompt_is_capped_at_600_characters() {
        assert_eq!(collapsed_text(&"x".repeat(700)), Some("x".repeat(600)));
        // Eight lines of 100 characters: the line cap keeps all of them, the character cap trims.
        let lines = vec!["y".repeat(100); 8].join("\n");
        let preview = collapsed_text(&lines).unwrap();
        assert_eq!(preview.chars().count(), 600);
        assert!(lines.starts_with(&preview));
    }

    #[test]
    fn the_character_cap_counts_characters_not_bytes() {
        let preview = collapsed_text(&"é".repeat(601)).unwrap();
        assert_eq!(preview, "é".repeat(600));
    }

    #[test]
    fn working_headers_count_from_their_start() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let since = now - Duration::from_secs(12);
        assert_eq!(
            header_label(&TurnHeader::Working { since: Some(since) }, now),
            Some("Working for 12s".to_owned())
        );
        assert_eq!(
            header_label(&TurnHeader::Working { since: None }, now),
            Some("Working…".to_owned())
        );
        // A start after `now` (clock skew) shows zero rather than failing.
        let future = now + Duration::from_secs(5);
        assert_eq!(
            header_label(
                &TurnHeader::Working {
                    since: Some(future)
                },
                now
            ),
            Some("Working for 0s".to_owned())
        );
    }

    #[test]
    fn worked_and_stopped_headers_name_their_elapsed_time() {
        let now = SystemTime::UNIX_EPOCH;
        let worked = |elapsed| TurnHeader::Worked {
            elapsed,
            folded: true,
        };
        assert_eq!(
            header_label(&worked(Some(Duration::from_secs(80))), now),
            Some("Worked for 1m 20s".to_owned())
        );
        assert_eq!(header_label(&worked(None), now), Some("Worked".to_owned()));
        let stopped = |elapsed| TurnHeader::Stopped { elapsed };
        assert_eq!(
            header_label(&stopped(Some(Duration::from_secs(34))), now),
            Some("You stopped after 34s".to_owned())
        );
        assert_eq!(
            header_label(&stopped(None), now),
            Some("You stopped this response".to_owned())
        );
        assert_eq!(header_label(&TurnHeader::None, now), None);
    }
}
