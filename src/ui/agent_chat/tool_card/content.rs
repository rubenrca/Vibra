//! Tool call content, shown inline when a call line is expanded: text, diffs
//! and client terminal output.

use gpui::{AnyElement, Div, SharedString, Stateful, div, prelude::*, px};

use super::super::AgentChatView;
use super::super::diff::{self, DiffKind, DiffLine};
use crate::domain::agent_chat::ToolContent;
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

const MAX_DIFF_LINES: usize = 400;
const MAX_TERMINAL_LINES: usize = 60;

impl AgentChatView {
    pub(super) fn tool_content(
        &self,
        ix: usize,
        index: usize,
        content: &ToolContent,
    ) -> AnyElement {
        let id = format!("chat-tool-{ix}-{index}");
        match content {
            ToolContent::Text(text) => {
                let lines = text.lines().map(strip_ansi).collect::<Vec<_>>();
                code_frame(id)
                    .children(lines.iter().map(|line| code_line(line)))
                    .into_any_element()
            }
            ToolContent::Diff {
                path,
                old_text,
                new_text,
            } => diff_block(
                id,
                path,
                &diff::changed_lines(old_text.as_deref(), new_text),
            ),
            ToolContent::Terminal { terminal_id } => self.terminal_block(id, terminal_id),
        }
    }

    fn terminal_block(&self, id: String, terminal_id: &str) -> AnyElement {
        let Some((output, exit_status)) = self.timeline.terminal(terminal_id) else {
            return div()
                .text_color(colors().subtle)
                .child("Waiting for output…")
                .into_any_element();
        };
        let lines: Vec<String> = output.lines().map(strip_ansi).collect();
        let (status, status_color) = match exit_status {
            None => ("Running…".to_owned(), colors().accent),
            Some(0) => ("Exited with code 0".to_owned(), colors().success),
            Some(code) => (format!("Exited with code {code}"), colors().danger),
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                code_frame(id)
                    .when(lines.is_empty(), |frame| {
                        frame.child(code_line("No output yet.").text_color(colors().subtle))
                    })
                    .children(
                        tail_lines(&lines, MAX_TERMINAL_LINES)
                            .iter()
                            .map(|line| code_line(line)),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(status_color)
                    .child(status),
            )
            .into_any_element()
    }
}

/// Scrollable monospace frame shared by text, diffs and terminal output.
fn code_frame(id: String) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .max_h(px(240.0))
        .overflow_y_scroll()
        .py_1()
        .rounded(px(6.0))
        .bg(surface_tint(colors().terminal, colors().elevated))
        .font_family(MONO_FONT)
        .text_size(px(11.5))
        .line_height(px(16.0))
        .text_color(colors().foreground)
}

fn code_line(text: &str) -> Div {
    div()
        .w_full()
        .min_h(px(16.0))
        .px_2()
        .child(if text.is_empty() {
            " ".to_owned()
        } else {
            text.to_owned()
        })
}

/// The file's path, then its changed lines. Counts are on the call line.
fn diff_block(id: String, path: &str, lines: &[DiffLine]) -> AnyElement {
    let hidden = lines.len().saturating_sub(MAX_DIFF_LINES);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .min_w(px(0.0))
                .truncate()
                .font_family(MONO_FONT)
                .text_color(colors().subtle)
                .child(path.to_owned()),
        )
        .child(
            code_frame(id)
                .children(lines.iter().take(MAX_DIFF_LINES).map(diff_row))
                .when(hidden > 0, |frame| {
                    frame.child(
                        code_line(&format!("… {hidden} more lines")).text_color(colors().subtle),
                    )
                }),
        )
        .into_any_element()
}

fn diff_row(line: &DiffLine) -> Div {
    let (sign, background, color) = match line.kind {
        DiffKind::Added => ("+", colors().diff_added_bg, colors().diff_added),
        DiffKind::Removed => ("-", colors().diff_deleted_bg, colors().diff_deleted),
    };
    code_line(&format!("{sign} {}", line.text))
        .bg(background)
        .text_color(color)
}

/// The last `count` lines, so long command output keeps its end visible.
fn tail_lines(lines: &[String], count: usize) -> &[String] {
    &lines[lines.len().saturating_sub(count)..]
}

/// Removes ANSI escape sequences (colors, cursor moves) from one line of output.
/// Only CSI sequences are handled; anything else is kept as text.
fn strip_ansi(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            // A CSI sequence ends at the first byte in 0x40..=0x7e.
            for next in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&next) {
                    break;
                }
            }
            continue;
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_color_sequences_only() {
        assert_eq!(strip_ansi("\u{1b}[31merror\u{1b}[0m: x"), "error: x");
        assert_eq!(strip_ansi("plain [text]"), "plain [text]");
        assert_eq!(strip_ansi("\u{1b}[2K"), "");
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let lines: Vec<String> = ["a", "b", "c"].map(str::to_owned).to_vec();
        assert_eq!(tail_lines(&lines, 2), ["b".to_owned(), "c".to_owned()]);
        assert_eq!(tail_lines(&lines, 10).len(), 3);
        assert!(tail_lines(&[], 2).is_empty());
    }
}
