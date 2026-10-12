//! The `/` command and `@` file popups above the composer card: which popup
//! the draft opens, how its rows match, and how a chosen row lands in the draft.

use std::cmp::Ordering;
use std::ops::Range;

use gpui::{
    AnyElement, Context, Div, Modifiers, SharedString, Stateful, div, prelude::*, px, relative, svg,
};

use super::super::AgentChatView;
use crate::domain::agent_chat::{PromptFile, SlashCommand};
use crate::ui::menu::{menu_hover, menu_panel};
use crate::ui::theme::{MONO_FONT, colors};

/// Rows a popup shows at once; a longer command list scrolls to the selection.
const POPUP_ROWS: usize = 8;

/// The popup open above the card for the current draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Popup {
    /// Commands whose names start with the text after `/`.
    Commands(Vec<SlashCommand>),
    /// Files matching the text after `@`. `start` is where that `@` token begins
    /// in the draft; `scanning` is set while the first file scan runs.
    Files {
        start: usize,
        matches: Vec<String>,
        scanning: bool,
    },
}

impl Popup {
    fn row_count(&self) -> usize {
        match self {
            Self::Commands(commands) => commands.len(),
            Self::Files { matches, .. } => matches.len(),
        }
    }
}

/// The text after a leading `/` while it is still a single word.
pub(super) fn command_query(draft: &str) -> Option<&str> {
    let query = draft.strip_prefix('/')?;
    (!query.contains(char::is_whitespace)).then_some(query)
}

fn command_name(command: &SlashCommand) -> &str {
    command.name.trim_start_matches('/')
}

/// Commands whose name starts with `query`, ignoring case, in the agent's order.
pub(super) fn filter_commands<'a>(
    commands: &'a [SlashCommand],
    query: &str,
) -> Vec<&'a SlashCommand> {
    let query = query.to_lowercase();
    commands
        .iter()
        .filter(|command| command_name(command).to_lowercase().starts_with(&query))
        .collect()
}

/// The draft after choosing a command: `/name ` ready for an argument.
pub(super) fn complete_command(command: &SlashCommand) -> String {
    format!("/{} ", command_name(command))
}

/// The `@` token at the end of the draft: where it starts, and the text after the `@`.
pub(super) fn active_mention(draft: &str) -> Option<(usize, &str)> {
    let start = draft
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    let token = &draft[start..];
    token.strip_prefix('@').map(|query| (start, query))
}

/// Appends an `@` that opens the file popup, keeping the words before it.
pub(super) fn push_mention(draft: &mut String) {
    if !draft.is_empty() && !draft.ends_with(char::is_whitespace) {
        draft.push(' ');
    }
    draft.push('@');
}

/// How well `path` matches `query`; higher is better and `None` means no match.
/// A filename hit beats a path hit, and a hit near the start beats a later one.
/// Letters of the query may also match in order with gaps, scoring lower.
pub(super) fn match_score(path: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let path = path.to_lowercase();
    let query = query.to_lowercase();
    let name_start = path.rfind('/').map_or(0, |index| index + 1);
    if let Some(index) = path[name_start..].find(query.as_str()) {
        let at_start = if index == 0 { 500 } else { 0 };
        return Some(4000 + at_start - index as i64);
    }
    if let Some(index) = path.find(query.as_str()) {
        return Some(2000 - index.min(1000) as i64);
    }
    // Subsequence: every query letter in order. A tighter span scores higher.
    let mut wanted = query.chars().peekable();
    let mut first = None;
    let mut last = 0;
    for (index, character) in path.chars().enumerate() {
        if wanted.peek() == Some(&character) {
            if first.is_none() {
                first = Some(index);
            }
            last = index;
            wanted.next();
        }
    }
    let first = first?;
    if wanted.peek().is_some() {
        return None;
    }
    let span = (last + 1 - first) as i64;
    let gaps = span - query.chars().count() as i64;
    Some(1000 - gaps.min(900))
}

/// Best matches first: higher score, then shorter path, then alphabetical.
fn rank_order(a: &(i64, &str), b: &(i64, &str)) -> Ordering {
    b.0.cmp(&a.0)
        .then(a.1.len().cmp(&b.1.len()))
        .then(a.1.cmp(b.1))
}

/// Up to `limit` files matching `query`, best match first.
pub(super) fn rank_files<'a>(files: &'a [String], query: &str, limit: usize) -> Vec<&'a str> {
    let mut ranked: Vec<(i64, &str)> = files
        .iter()
        .filter_map(|file| match_score(file, query).map(|score| (score, file.as_str())))
        .collect();
    // Only the top `limit` need full ordering; partition first for long file lists.
    if ranked.len() > limit {
        ranked.select_nth_unstable_by(limit, rank_order);
        ranked.truncate(limit);
    }
    ranked.sort_by(rank_order);
    ranked.into_iter().map(|(_, file)| file).collect()
}

/// Adds `file` unless the same path is already attached. Returns whether it was added.
pub(super) fn add_attachment(attachments: &mut Vec<PromptFile>, file: PromptFile) -> bool {
    if attachments
        .iter()
        .any(|attached| attached.path == file.path)
    {
        return false;
    }
    attachments.push(file);
    true
}

/// The selection after an arrow key, wrapping at either end.
pub(super) fn step(selection: usize, len: usize, down: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if down {
        (selection + 1) % len
    } else {
        (selection + len - 1) % len
    }
}

/// The rows to draw so that `selected` is visible.
pub(super) fn visible_rows(len: usize, selected: usize, rows: usize) -> Range<usize> {
    let selected = selected.min(len.saturating_sub(1));
    let start = if selected >= rows {
        selected + 1 - rows
    } else {
        0
    };
    start..(start + rows).min(len)
}

impl AgentChatView {
    /// The popup for the draft, if one should show. The `/` popup needs a
    /// matching command; the `@` popup needs a file source.
    pub(super) fn active_popup(&self) -> Option<Popup> {
        if !self.accepts_input() || self.composer.popup_dismissed {
            return None;
        }
        if let Some(query) = command_query(&self.draft) {
            let commands: Vec<SlashCommand> = filter_commands(&self.timeline.commands, query)
                .into_iter()
                .cloned()
                .collect();
            return (!commands.is_empty()).then_some(Popup::Commands(commands));
        }
        let (start, query) = active_mention(&self.draft)?;
        self.file_search.as_ref()?;
        let matches = rank_files(&self.composer.files, query, POPUP_ROWS)
            .into_iter()
            .map(str::to_owned)
            .collect();
        Some(Popup::Files {
            start,
            matches,
            scanning: self.composer.files_scanning && self.composer.files.is_empty(),
        })
    }

    /// Keys that move or accept inside an open popup. Returns whether one was used.
    pub(super) fn handle_popup_key(
        &mut self,
        key: &str,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(popup) = self.active_popup() else {
            return false;
        };
        let rows = popup.row_count();
        let selected = self.composer.selection.min(rows.saturating_sub(1));
        match key {
            "escape" | "esc" => self.composer.popup_dismissed = true,
            "down" | "up" if rows > 0 => {
                self.composer.selection = step(selected, rows, key == "down");
            }
            "tab" if rows > 0 => self.accept_popup_row(selected, cx),
            "enter" | "return" if rows > 0 && !modifiers.shift => {
                self.accept_popup_row(selected, cx);
            }
            _ => return false,
        }
        true
    }

    /// Inserts `@` so the file popup opens, as the "+" button does.
    pub(super) fn insert_mention(&mut self, cx: &mut Context<Self>) {
        push_mention(&mut self.draft);
        self.after_edit(cx);
        cx.notify();
    }

    /// Completes the chosen command, or attaches the chosen file and drops its `@` text.
    fn accept_popup_row(&mut self, index: usize, cx: &mut Context<Self>) {
        match self.active_popup() {
            Some(Popup::Commands(commands)) => {
                if let Some(command) = commands.get(index) {
                    self.draft = complete_command(command);
                }
            }
            Some(Popup::Files { start, matches, .. }) => {
                if let (Some(relative), Some(search)) =
                    (matches.get(index), self.file_search.as_ref())
                {
                    self.draft.truncate(start);
                    add_attachment(
                        &mut self.attachments,
                        PromptFile {
                            path: search.root.join(relative).to_string_lossy().into_owned(),
                            label: relative.clone(),
                        },
                    );
                }
            }
            None => {}
        }
        self.composer.selection = 0;
        cx.notify();
    }

    /// The popup rows, drawn above the card. A press outside dismisses it.
    pub(super) fn popup_panel(&self, popup: &Popup, cx: &mut Context<Self>) -> AnyElement {
        let row_count = popup.row_count();
        let selected = self.composer.selection.min(row_count.saturating_sub(1));
        let rows = visible_rows(row_count, selected, POPUP_ROWS);
        let mut children: Vec<AnyElement> = Vec::new();
        match popup {
            Popup::Commands(commands) => {
                for index in rows {
                    let command = &commands[index];
                    children.push(
                        popup_row(index, index == selected, cx)
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(MONO_FONT)
                                    .text_color(colors().foreground)
                                    .child(format!("/{}", command_name(command))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_color(colors().muted)
                                    .child(command.description.clone()),
                            )
                            .into_any_element(),
                    );
                }
            }
            Popup::Files {
                matches, scanning, ..
            } => {
                if matches.is_empty() {
                    let text = if *scanning {
                        "Searching files…"
                    } else {
                        "No matching files"
                    };
                    children.push(empty_row(text));
                }
                for index in rows {
                    children.push(
                        popup_row(index, index == selected, cx)
                            .child(
                                svg()
                                    .path("chrome-icons/files.svg")
                                    .size(px(12.0))
                                    .flex_none()
                                    .text_color(colors().muted),
                            )
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .child(matches[index].clone()),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        menu_panel()
            .absolute()
            .bottom(relative(1.0))
            .mb(px(6.0))
            .left_0()
            .right_0()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.composer.popup_dismissed = true;
                cx.notify();
            }))
            .children(children)
            .into_any_element()
    }
}

/// One clickable popup row; a click accepts it.
fn popup_row(index: usize, selected: bool, cx: &mut Context<AgentChatView>) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("agent-chat-popup-{index}")))
        .h(px(28.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(px(12.5))
        .when(selected, |row| row.bg(menu_hover()))
        .hover(|row| row.bg(menu_hover()))
        .on_click(cx.listener(move |this, _, _, cx| this.accept_popup_row(index, cx)))
}

fn empty_row(text: &str) -> AnyElement {
    div()
        .h(px(28.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .text_size(px(12.5))
        .text_color(colors().subtle)
        .child(text.to_owned())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str) -> SlashCommand {
        SlashCommand {
            name: name.into(),
            description: format!("Runs {name}"),
            input_hint: None,
        }
    }

    #[test]
    fn slash_popup_needs_a_single_word_after_the_slash() {
        assert_eq!(command_query("/"), Some(""));
        assert_eq!(command_query("/co"), Some("co"));
        assert_eq!(command_query("/compact now"), None);
        assert_eq!(command_query("hi /co"), None);
    }

    #[test]
    fn commands_match_by_prefix_ignoring_case() {
        let commands = [command("compact"), command("clear"), command("cost")];
        let names = |found: Vec<&SlashCommand>| {
            found
                .into_iter()
                .map(|command| command.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(filter_commands(&commands, "CO")), ["compact", "cost"]);
        assert_eq!(names(filter_commands(&commands, "c")).len(), 3);
        assert!(filter_commands(&commands, "x").is_empty());
        assert_eq!(complete_command(&command("compact")), "/compact ");
    }

    #[test]
    fn mention_is_the_at_token_after_the_last_space() {
        assert_eq!(active_mention("hola @ma"), Some((5, "ma")));
        assert_eq!(active_mention("@"), Some((0, "")));
        assert_eq!(active_mention("hi\n@x"), Some((3, "x")));
        assert_eq!(active_mention("hola @ma x"), None);
        assert_eq!(active_mention("a@b"), None);
    }

    #[test]
    fn push_mention_adds_a_space_only_after_text() {
        let mut draft = String::new();
        push_mention(&mut draft);
        assert_eq!(draft, "@");
        push_mention(&mut draft);
        assert_eq!(draft, "@ @");
        let mut draft = "hola".to_owned();
        push_mention(&mut draft);
        assert_eq!(draft, "hola @");
        let mut draft = "hola ".to_owned();
        push_mention(&mut draft);
        assert_eq!(draft, "hola @");
    }

    #[test]
    fn filename_hits_rank_above_path_hits_and_gaps() {
        let files: Vec<String> = [
            "src/ui/mod.rs",
            "README.md",
            "docs/composer-notes.md",
            "src/ui/agent_chat/composer.rs",
            "src/ui/agent_chat/timeline.rs",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let ranked = rank_files(&files, "comp", 8);
        assert_eq!(ranked.len(), 2);
        assert!(ranked[0].ends_with("composer.rs") || ranked[0].ends_with("composer-notes.md"));
        assert!(!ranked.contains(&"README.md"));
        assert_eq!(match_score("src/ui/mod.rs", "comp"), None);
        // Letters in order with gaps still match, below a direct hit.
        assert!(match_score("src/agent_chat/timeline.rs", "scr").is_some());
        assert!(match_score("src/agent_chat/timeline.rs", "tlr").unwrap() < 1000);
    }

    #[test]
    fn empty_query_lists_the_shortest_paths_first_and_respects_the_limit() {
        let files: Vec<String> = ["bbbb", "a", "cc"].into_iter().map(str::to_owned).collect();
        assert_eq!(rank_files(&files, "", 2), ["a", "cc"]);
        assert_eq!(rank_files(&files, "", 10).len(), 3);
    }

    #[test]
    fn attachments_are_deduplicated_by_path() {
        let file = PromptFile {
            path: "/repo/a.rs".into(),
            label: "a.rs".into(),
        };
        let mut attachments = Vec::new();
        assert!(add_attachment(&mut attachments, file.clone()));
        assert!(!add_attachment(&mut attachments, file));
        assert_eq!(attachments.len(), 1);
    }

    #[test]
    fn selection_wraps_and_the_window_keeps_the_selection_visible() {
        assert_eq!(step(0, 3, false), 2);
        assert_eq!(step(2, 3, true), 0);
        assert_eq!(step(0, 0, true), 0);
        assert_eq!(visible_rows(3, 0, 8), 0..3);
        let window = visible_rows(20, 19, 8);
        assert_eq!(window, 12..20);
        assert!(window.contains(&19));
    }
}
