//! Words for tool work: what one call did, what a settled run adds up to, and
//! what a live run is doing now. Pure, so the view and the tests share one
//! wording.

use crate::domain::agent_chat::{ChatItem, ToolCallItem, ToolKind, ToolStatus};

use super::{file_name, is_running};

/// Icon for reasoning, and for a run that only thinks.
pub(super) const THOUGHT_ICON: &str = "chrome-icons/brain.svg";

/// How a call kind reads in a label: present, past and base verbs, and the word
/// that joins the verb to its subject.
struct Words {
    present: &'static str,
    past: &'static str,
    base: &'static str,
    connector: &'static str,
}

fn words(kind: ToolKind) -> Words {
    let (present, past, base, connector) = match kind {
        ToolKind::Read => ("Reading", "Read", "read", ""),
        ToolKind::Edit => ("Editing", "Edited", "edit", ""),
        ToolKind::Delete => ("Deleting", "Deleted", "delete", ""),
        ToolKind::Move => ("Moving", "Moved", "move", ""),
        ToolKind::Search => ("Searching", "Searched", "search", "for"),
        ToolKind::Execute => ("Running", "Ran", "run", ""),
        ToolKind::Fetch => ("Fetching", "Fetched", "fetch", ""),
        ToolKind::Think => ("Thinking", "Thought", "think", ""),
        ToolKind::SwitchMode => ("Switching mode", "Switched mode", "switch mode", ""),
        ToolKind::Other => ("Using", "Used", "use", ""),
    };
    Words {
        present,
        past,
        base,
        connector,
    }
}

/// What a call did, in words. Running calls, and every call in a live run
/// (`active`), use the present tense; settled calls use the past tense. A failed
/// call reads "Failed to …" in the base form.
pub(super) fn call_label(call: &ToolCallItem, active: bool) -> String {
    let words = words(call.kind);
    let subject = subject(call);
    let connector = if subject.is_empty() {
        ""
    } else {
        words.connector
    };
    let phrase = |verb: &str| join_words(&[verb, connector, subject.as_str()]);
    if call.status == ToolStatus::Failed {
        format!("Failed to {}", phrase(words.base))
    } else if active || is_running(call.status) {
        phrase(words.present)
    } else {
        phrase(words.past)
    }
}

/// The full command of an execute call, when it says more than its program.
pub(super) fn command_detail(call: &ToolCallItem) -> Option<String> {
    if call.kind != ToolKind::Execute {
        return None;
    }
    let command = unwrap_command(&call.title);
    let program = command_program(&call.title)?;
    (command != program.as_str()).then(|| command.to_owned())
}

/// The locations a call line shows as file chips. A file label already names
/// the first location, so only the others are chipped.
pub(super) fn extra_locations(call: &ToolCallItem) -> &[String] {
    match call.kind {
        ToolKind::Read | ToolKind::Edit | ToolKind::Delete | ToolKind::Move
            if !call.locations.is_empty() =>
        {
            &call.locations[1..]
        }
        _ => &call.locations,
    }
}

/// The object of a call's verb: a file, a program, a query or a target.
fn subject(call: &ToolCallItem) -> String {
    match call.kind {
        ToolKind::Read | ToolKind::Edit | ToolKind::Delete | ToolKind::Move => {
            match call.locations.as_slice() {
                [] => or_default(title_subject(call), "file"),
                [path] => file_name(path).to_owned(),
                [path, rest @ ..] => format!("{} +{} more", file_name(path), rest.len()),
            }
        }
        ToolKind::Execute => command_program(&call.title).unwrap_or_else(|| "command".to_owned()),
        ToolKind::Search => title_subject(call),
        ToolKind::Fetch => fetch_target(&title_subject(call)),
        ToolKind::Other => or_default(title_subject(call), "tool"),
        ToolKind::Think | ToolKind::SwitchMode => String::new(),
    }
}

/// The call's title without backticks, and without a leading verb (and the
/// connector after it) that the label already says.
fn title_subject(call: &ToolCallItem) -> String {
    let words = words(call.kind);
    let title = call.title.trim().trim_matches('`').trim();
    let Some((first, rest)) = split_word(title) else {
        return String::new();
    };
    let repeats_verb = [words.present, words.past, words.base]
        .iter()
        .any(|verb| first.eq_ignore_ascii_case(verb));
    if !repeats_verb {
        return title.to_owned();
    }
    match split_word(rest) {
        Some((connector, after)) if connector.eq_ignore_ascii_case(words.connector) => {
            after.to_owned()
        }
        _ => rest.to_owned(),
    }
}

/// A URL shown as its host and path, without the scheme.
fn fetch_target(target: &str) -> String {
    let target = target
        .strip_prefix("https://")
        .or_else(|| target.strip_prefix("http://"))
        .unwrap_or(target);
    target.trim_end_matches('/').to_owned()
}

/// The program a shell command runs: its first word after leading `NAME=value`
/// assignments and any shell wrapper, without its directories.
pub(super) fn command_program(command: &str) -> Option<String> {
    let mut rest = unwrap_command(command);
    while let Some((word, after)) = split_word(rest) {
        if !is_assignment(word) {
            let word = word.trim_matches(|c: char| c == '"' || c == '\'');
            let name = word.rsplit('/').next().unwrap_or(word);
            return (!name.is_empty()).then(|| name.to_owned());
        }
        rest = after;
    }
    None
}

/// The script inside `bash -lc '…'`-style wrappers, otherwise the command
/// itself, with backticks removed.
fn unwrap_command(command: &str) -> &str {
    let mut text = command.trim().trim_matches('`').trim();
    // A wrapper may wrap another; two levels cover what agents send.
    for _ in 0..2 {
        match shell_script(text) {
            Some(script) => text = script,
            None => break,
        }
    }
    text
}

/// The script of `sh`, `bash`, `zsh` or `dash` run with a `-c` flag.
fn shell_script(text: &str) -> Option<&str> {
    let (program, mut rest) = split_word(text)?;
    if !matches!(program.rsplit('/').next()?, "sh" | "bash" | "zsh" | "dash") {
        return None;
    }
    loop {
        let (flag, after) = split_word(rest)?;
        if !flag.starts_with('-') || flag.starts_with("--") {
            return None;
        }
        if flag.contains('c') {
            return Some(unquote(after));
        }
        rest = after;
    }
}

/// Removes one pair of matching quotes around `text`, if it has them.
fn unquote(text: &str) -> &str {
    let text = text.trim();
    for quote in ['\'', '"'] {
        if let Some(inner) = text.strip_prefix(quote) {
            return inner.rfind(quote).map_or(inner, |end| &inner[..end]);
        }
    }
    text
}

/// `NAME=value`, as a shell assignment before a command.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// The first whitespace-separated word and the text after it.
fn split_word(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if text.is_empty() {
        return None;
    }
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    Some((&text[..end], text[end..].trim_start()))
}

fn or_default(text: String, fallback: &str) -> String {
    if text.is_empty() {
        fallback.to_owned()
    } else {
        text
    }
}

/// Joins the non-empty parts with single spaces.
fn join_words(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a settled run's categories count. Edit and command outrank the rest;
/// other tools come last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Read,
    Edit,
    Command,
    Search,
    Fetch,
    Other,
}

impl Category {
    fn of(kind: ToolKind) -> Self {
        match kind {
            ToolKind::Read => Self::Read,
            ToolKind::Edit | ToolKind::Delete | ToolKind::Move => Self::Edit,
            ToolKind::Execute => Self::Command,
            ToolKind::Search => Self::Search,
            ToolKind::Fetch => Self::Fetch,
            ToolKind::Think | ToolKind::SwitchMode | ToolKind::Other => Self::Other,
        }
    }

    /// Lower values are chosen first for the summary.
    fn priority(self) -> u8 {
        match self {
            Self::Command | Self::Edit => 0,
            Self::Read | Self::Search | Self::Fetch => 1,
            Self::Other => 2,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Read => "chrome-icons/eye.svg",
            Self::Edit => "chrome-icons/square-pen.svg",
            Self::Command => "chrome-icons/terminal.svg",
            Self::Search => "chrome-icons/search.svg",
            Self::Fetch => "chrome-icons/globe.svg",
            Self::Other => "chrome-icons/wrench.svg",
        }
    }

    /// The category's label for `count` units (files for edits, calls otherwise).
    fn label(self, count: usize) -> String {
        match self {
            Self::Read => format!("Read {count} {}", plural(count, "file", "files")),
            Self::Edit => format!("Changed {count} {}", plural(count, "file", "files")),
            Self::Command => format!("Ran {count} {}", plural(count, "command", "commands")),
            Self::Search => format!("Searched code {count} {}", plural(count, "time", "times")),
            Self::Fetch => format!("Fetched {count} {}", plural(count, "page", "pages")),
            Self::Other => format!("Used {count} {}", plural(count, "tool", "tools")),
        }
    }
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

/// The calls of one category in a run, and the files they touch.
struct Group<'a> {
    category: Category,
    /// Position of the category's first call in the run.
    first: usize,
    calls: usize,
    files: Vec<&'a str>,
    /// Edits that name no file; each counts once.
    unnamed: usize,
}

impl<'a> Group<'a> {
    fn new(category: Category, first: usize) -> Self {
        Self {
            category,
            first,
            calls: 0,
            files: Vec::new(),
            unnamed: 0,
        }
    }

    fn add(&mut self, call: &'a ToolCallItem) {
        self.calls += 1;
        if call.locations.is_empty() {
            self.unnamed += 1;
        }
        for path in &call.locations {
            if !self.files.contains(&path.as_str()) {
                self.files.push(path);
            }
        }
    }

    /// Distinct files for edits, calls for everything else.
    fn units(&self) -> usize {
        if self.category == Category::Edit {
            self.files.len() + self.unnamed
        } else {
            self.calls
        }
    }
}

/// The summary line of a run and whether any of its calls failed.
pub(super) struct RunSummary {
    pub text: String,
    pub failed: bool,
}

/// Sums up a settled run in one sentence, for example "Ran 2 commands and
/// changed 1 file". At most two categories are named; the calls they leave out
/// become "performed N other actions". Permission items are not part of it.
pub(super) fn summarize(items: &[ChatItem]) -> RunSummary {
    let calls = tool_calls(items);
    if calls.is_empty() {
        let thoughts = items
            .iter()
            .filter(|item| matches!(item, ChatItem::Reasoning { .. }))
            .count();
        let text = match thoughts {
            0 => String::new(),
            1 => "Thought".to_owned(),
            count => format!("Thought (×{count})"),
        };
        return RunSummary {
            text,
            failed: false,
        };
    }

    let mut groups: Vec<Group> = Vec::new();
    for (position, &call) in calls.iter().enumerate() {
        let category = Category::of(call.kind);
        let index = match groups.iter().position(|group| group.category == category) {
            Some(index) => index,
            None => {
                groups.push(Group::new(category, position));
                groups.len() - 1
            }
        };
        groups[index].add(call);
    }

    let mut chosen: Vec<&Group> = groups.iter().collect();
    chosen.sort_by_key(|group| (group.category.priority(), group.first));
    chosen.truncate(2);
    chosen.sort_by_key(|group| group.first);

    let mut labels: Vec<String> = chosen
        .iter()
        .map(|group| group.category.label(group.units()))
        .collect();
    let covered: usize = chosen.iter().map(|group| group.calls).sum();
    let other = calls.len() - covered;
    if other > 0 {
        labels.push(format!(
            "Performed {other} other {}",
            plural(other, "action", "actions")
        ));
    }
    RunSummary {
        text: sentence(&labels),
        failed: calls.iter().any(|call| call.status == ToolStatus::Failed),
    }
}

/// Joins labels as a sentence. The first keeps its capital; the rest start lower case.
fn sentence(labels: &[String]) -> String {
    let parts: Vec<String> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            if index == 0 {
                label.clone()
            } else {
                lower_first(label)
            }
        })
        .collect();
    match parts.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [init @ .., last] => format!("{}, and {last}", init.join(", ")),
    }
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Icon for a settled run: the category its calls share, a wrench when they
/// mix, and the brain when the run is only thoughts.
pub(super) fn summary_icon(items: &[ChatItem]) -> &'static str {
    let mut categories = tool_calls(items)
        .into_iter()
        .map(|call| Category::of(call.kind));
    let Some(first) = categories.next() else {
        return THOUGHT_ICON;
    };
    if categories.all(|category| category == first) {
        first.icon()
    } else {
        Category::Other.icon()
    }
}

/// What a live run is doing now: thinking, or one tool call.
pub(super) enum Activity<'a> {
    Thinking,
    Call(&'a ToolCallItem),
}

/// The live activity: thinking when the newest item is a thought, otherwise the
/// last running call, or the last call when none runs.
pub(super) fn activity(items: &[ChatItem]) -> Activity<'_> {
    let calls = tool_calls(items);
    let current = calls
        .iter()
        .rev()
        .find(|call| is_running(call.status))
        .or(calls.last())
        .copied();
    match (items.last(), current) {
        (Some(ChatItem::Reasoning { .. }), _) | (_, None) => Activity::Thinking,
        (_, Some(call)) => Activity::Call(call),
    }
}

/// The text of the newest thought in a run.
pub(super) fn latest_thought(items: &[ChatItem]) -> Option<&str> {
    items.iter().rev().find_map(|item| match item {
        ChatItem::Reasoning { text } => Some(text.as_str()),
        _ => None,
    })
}

/// A thought as one line: its first sentence, or its first 240 characters when
/// the sentence runs longer. Markdown emphasis and code ticks are dropped.
pub(super) fn thought_preview(text: &str) -> String {
    const LIMIT: usize = 240;
    let flat = text.replace("**", "").replace('`', "");
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = flat
        .char_indices()
        .find_map(|(index, c)| {
            let after = index + c.len_utf8();
            (matches!(c, '.' | '!' | '?')
                && flat[after..].chars().next().is_none_or(char::is_whitespace))
            .then_some(after)
        })
        .unwrap_or(flat.len());
    let sentence = &flat[..end];
    if sentence.chars().count() <= LIMIT {
        return sentence.to_owned();
    }
    let cut: String = sentence.chars().take(LIMIT).collect();
    format!("{}…", cut.trim_end())
}

/// The first non-empty line of `text`, trimmed.
pub(super) fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

fn tool_calls(items: &[ChatItem]) -> Vec<&ToolCallItem> {
    items
        .iter()
        .filter_map(|item| match item {
            ChatItem::ToolCall(call) => Some(call),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::PermissionItem;

    fn call(kind: ToolKind, status: ToolStatus, title: &str, locations: &[&str]) -> ToolCallItem {
        ToolCallItem {
            id: format!("{title}-{kind:?}"),
            title: title.to_owned(),
            kind,
            status,
            content: Vec::new(),
            locations: locations.iter().map(|path| (*path).to_owned()).collect(),
        }
    }

    fn tool(kind: ToolKind, title: &str) -> ChatItem {
        ChatItem::ToolCall(call(kind, ToolStatus::Completed, title, &[]))
    }

    fn tool_at(kind: ToolKind, title: &str, locations: &[&str]) -> ChatItem {
        ChatItem::ToolCall(call(kind, ToolStatus::Completed, title, locations))
    }

    fn running(kind: ToolKind, title: &str) -> ChatItem {
        ChatItem::ToolCall(call(kind, ToolStatus::InProgress, title, &[]))
    }

    fn failed(kind: ToolKind, title: &str) -> ChatItem {
        ChatItem::ToolCall(call(kind, ToolStatus::Failed, title, &[]))
    }

    fn thought(text: &str) -> ChatItem {
        ChatItem::Reasoning {
            text: text.to_owned(),
        }
    }

    fn permission() -> ChatItem {
        ChatItem::Permission(PermissionItem {
            request_id: 1,
            tool_call_id: None,
            title: "Run tests?".to_owned(),
            options: Vec::new(),
            resolution: None,
        })
    }

    #[test]
    fn settled_calls_use_the_past_tense_and_running_ones_the_present() {
        let main = ["/repo/src/main.rs"];
        let read = call(ToolKind::Read, ToolStatus::Completed, "Read", &main);
        assert_eq!(call_label(&read, false), "Read main.rs");
        let reading = call(ToolKind::Read, ToolStatus::InProgress, "Read", &main);
        assert_eq!(call_label(&reading, false), "Reading main.rs");
        assert_eq!(call_label(&read, true), "Reading main.rs");

        let edit = call(
            ToolKind::Edit,
            ToolStatus::Completed,
            "Edit",
            &["layout.rs"],
        );
        assert_eq!(call_label(&edit, false), "Edited layout.rs");
        let delete = call(ToolKind::Delete, ToolStatus::Pending, "Delete", &["x"]);
        assert_eq!(call_label(&delete, false), "Deleting x");
        let mv = call(ToolKind::Move, ToolStatus::Completed, "Move", &["x"]);
        assert_eq!(call_label(&mv, false), "Moved x");

        let command = call(
            ToolKind::Execute,
            ToolStatus::Completed,
            "`cargo test`",
            &[],
        );
        assert_eq!(call_label(&command, false), "Ran cargo");
        let running_command = call(ToolKind::Execute, ToolStatus::InProgress, "cargo test", &[]);
        assert_eq!(call_label(&running_command, false), "Running cargo");

        let search = call(ToolKind::Search, ToolStatus::Completed, "foo", &[]);
        assert_eq!(call_label(&search, false), "Searched for foo");
        let searching = call(ToolKind::Search, ToolStatus::InProgress, "foo", &[]);
        assert_eq!(call_label(&searching, false), "Searching for foo");

        let fetch = call(
            ToolKind::Fetch,
            ToolStatus::Completed,
            "https://example.com/",
            &[],
        );
        assert_eq!(call_label(&fetch, false), "Fetched example.com");
        let fetching = call(ToolKind::Fetch, ToolStatus::InProgress, "example.com", &[]);
        assert_eq!(call_label(&fetching, false), "Fetching example.com");

        let other = call(ToolKind::Other, ToolStatus::Completed, "mcp__tool", &[]);
        assert_eq!(call_label(&other, false), "Used mcp__tool");
        assert_eq!(call_label(&other, true), "Using mcp__tool");
    }

    #[test]
    fn failed_calls_say_so_in_the_base_form() {
        let run = call(ToolKind::Execute, ToolStatus::Failed, "cargo test", &[]);
        assert_eq!(call_label(&run, false), "Failed to run cargo");
        assert_eq!(call_label(&run, true), "Failed to run cargo");

        let search = call(ToolKind::Search, ToolStatus::Failed, "foo", &[]);
        assert_eq!(call_label(&search, false), "Failed to search for foo");

        let edit = call(ToolKind::Edit, ToolStatus::Failed, "", &[]);
        assert_eq!(call_label(&edit, false), "Failed to edit file");
    }

    #[test]
    fn file_labels_name_the_first_file_and_count_the_rest() {
        let edit = call(
            ToolKind::Edit,
            ToolStatus::Completed,
            "Edit",
            &["/repo/src/a.rs", "/repo/src/b.rs", "/repo/c.rs"],
        );
        assert_eq!(call_label(&edit, false), "Edited a.rs +2 more");
        let chips = extra_locations(&edit);
        assert_eq!(chips.len(), 2);
        assert_eq!(chips[0], "/repo/src/b.rs");
        assert_eq!(chips[1], "/repo/c.rs");
    }

    #[test]
    fn file_labels_fall_back_to_the_title_without_its_verb() {
        let read = call(
            ToolKind::Read,
            ToolStatus::Completed,
            "Read src/main.rs",
            &[],
        );
        assert_eq!(call_label(&read, false), "Read src/main.rs");
        let reading = call(ToolKind::Read, ToolStatus::InProgress, "src/main.rs", &[]);
        assert_eq!(call_label(&reading, false), "Reading src/main.rs");
        let untitled = call(ToolKind::Edit, ToolStatus::Completed, "", &[]);
        assert_eq!(call_label(&untitled, false), "Edited file");
        assert!(extra_locations(&untitled).is_empty());
    }

    #[test]
    fn a_title_that_repeats_the_verb_is_not_said_twice() {
        let search = call(
            ToolKind::Search,
            ToolStatus::Completed,
            "Search for foo",
            &[],
        );
        assert_eq!(call_label(&search, false), "Searched for foo");
        let bare = call(ToolKind::Search, ToolStatus::InProgress, "Search", &[]);
        assert_eq!(call_label(&bare, false), "Searching");
        let fetch = call(
            ToolKind::Fetch,
            ToolStatus::Completed,
            "Fetch https://a.dev",
            &[],
        );
        assert_eq!(call_label(&fetch, false), "Fetched a.dev");
    }

    #[test]
    fn command_program_skips_assignments_and_shell_wrappers() {
        let cases = [
            ("echo hi", Some("echo")),
            ("`cargo test -p vibra`", Some("cargo")),
            ("RUST_LOG=debug FOO=1 cargo test", Some("cargo")),
            ("/usr/bin/git status", Some("git")),
            ("bash -lc 'cargo test --quiet'", Some("cargo")),
            ("sh -c \"FOO=1 npm run build\"", Some("npm")),
            ("/bin/zsh -lc 'ls -la'", Some("ls")),
            ("bash script.sh", Some("bash")),
            ("A=1", None),
            ("", None),
        ];
        for (command, expected) in cases {
            assert_eq!(
                command_program(command).as_deref(),
                expected,
                "command {command:?}"
            );
        }
    }

    #[test]
    fn command_detail_shows_arguments_only_when_they_add_something() {
        let plain = call(ToolKind::Execute, ToolStatus::Completed, "cargo", &[]);
        assert_eq!(command_detail(&plain), None);
        let wrapped = call(
            ToolKind::Execute,
            ToolStatus::Completed,
            "bash -lc 'cargo test'",
            &[],
        );
        assert_eq!(command_detail(&wrapped).as_deref(), Some("cargo test"));
        let read = call(ToolKind::Read, ToolStatus::Completed, "cargo test", &[]);
        assert_eq!(command_detail(&read), None);
    }

    #[test]
    fn summary_counts_each_category_with_singular_and_plural_forms() {
        let cases = [
            (vec![tool(ToolKind::Read, "a")], "Read 1 file"),
            (
                vec![tool(ToolKind::Read, "a"), tool(ToolKind::Read, "b")],
                "Read 2 files",
            ),
            (vec![tool(ToolKind::Execute, "ls")], "Ran 1 command"),
            (
                vec![
                    tool(ToolKind::Execute, "ls"),
                    tool(ToolKind::Execute, "pwd"),
                ],
                "Ran 2 commands",
            ),
            (vec![tool(ToolKind::Search, "x")], "Searched code 1 time"),
            (
                vec![tool(ToolKind::Search, "x"), tool(ToolKind::Search, "y")],
                "Searched code 2 times",
            ),
            (vec![tool(ToolKind::Fetch, "a")], "Fetched 1 page"),
            (
                vec![tool(ToolKind::Fetch, "a"), tool(ToolKind::Fetch, "b")],
                "Fetched 2 pages",
            ),
            (vec![tool(ToolKind::Other, "a")], "Used 1 tool"),
            (
                vec![tool(ToolKind::Think, "a"), tool(ToolKind::SwitchMode, "b")],
                "Used 2 tools",
            ),
            (
                vec![tool_at(ToolKind::Edit, "Edit", &["a.rs"])],
                "Changed 1 file",
            ),
        ];
        for (items, expected) in cases {
            assert_eq!(summarize(&items).text, expected);
        }
    }

    #[test]
    fn edits_count_distinct_files_and_unnamed_edits_once_each() {
        let same = [
            tool_at(ToolKind::Edit, "Edit", &["a.rs"]),
            tool_at(ToolKind::Delete, "Delete", &["a.rs"]),
        ];
        assert_eq!(summarize(&same).text, "Changed 1 file");

        let mixed = [
            tool_at(ToolKind::Edit, "Edit", &["a.rs", "b.rs"]),
            tool(ToolKind::Move, "Move"),
        ];
        assert_eq!(summarize(&mixed).text, "Changed 3 files");
    }

    #[test]
    fn summary_leads_with_commands_and_edits_and_counts_the_rest() {
        let run = [
            tool(ToolKind::Read, "a"),
            tool(ToolKind::Execute, "cargo test"),
            tool_at(ToolKind::Edit, "Edit", &["x.rs"]),
            tool(ToolKind::Read, "b"),
        ];
        assert_eq!(
            summarize(&run).text,
            "Ran 1 command, changed 1 file, and performed 2 other actions"
        );
    }

    #[test]
    fn other_tools_come_last_and_drop_out_when_two_categories_lead() {
        let run = [
            tool(ToolKind::Other, "x"),
            tool(ToolKind::Search, "foo"),
            tool(ToolKind::Fetch, "u"),
        ];
        assert_eq!(
            summarize(&run).text,
            "Searched code 1 time, fetched 1 page, and performed 1 other action"
        );
    }

    #[test]
    fn a_run_of_two_categories_keeps_the_order_of_its_first_calls() {
        let run = [tool(ToolKind::Think, "t"), tool(ToolKind::Read, "a")];
        assert_eq!(summarize(&run).text, "Used 1 tool and read 1 file");

        let run = [
            tool(ToolKind::Read, "a"),
            tool(ToolKind::Read, "b"),
            tool(ToolKind::Execute, "c"),
            tool(ToolKind::Other, "d"),
        ];
        assert_eq!(
            summarize(&run).text,
            "Read 2 files, ran 1 command, and performed 1 other action"
        );
    }

    #[test]
    fn a_run_of_only_thoughts_reads_thought_with_a_count() {
        assert_eq!(summarize(&[thought("a")]).text, "Thought");
        let three = [thought("a"), thought("b"), thought("c")];
        assert_eq!(summarize(&three).text, "Thought (×3)");
    }

    #[test]
    fn thoughts_and_permissions_stay_out_of_a_run_with_tools() {
        let run = [thought("hm"), tool(ToolKind::Read, "a"), permission()];
        assert_eq!(summarize(&run).text, "Read 1 file");
        assert_eq!(summarize(&[permission()]).text, "");
    }

    #[test]
    fn a_run_with_a_failed_call_is_flagged() {
        let run = [failed(ToolKind::Read, "a"), tool(ToolKind::Read, "b")];
        let summary = summarize(&run);
        assert!(summary.failed);
        assert_eq!(summary.text, "Read 2 files");
        assert!(!summarize(&[tool(ToolKind::Read, "a")]).failed);
    }

    #[test]
    fn summary_icon_follows_the_shared_category() {
        assert_eq!(
            summary_icon(&[tool(ToolKind::Read, "a"), tool(ToolKind::Read, "b")]),
            "chrome-icons/eye.svg"
        );
        assert_eq!(
            summary_icon(&[tool_at(ToolKind::Edit, "Edit", &["a"])]),
            "chrome-icons/square-pen.svg"
        );
        assert_eq!(
            summary_icon(&[tool(ToolKind::Execute, "ls")]),
            "chrome-icons/terminal.svg"
        );
        assert_eq!(
            summary_icon(&[tool(ToolKind::Search, "x")]),
            "chrome-icons/search.svg"
        );
        assert_eq!(
            summary_icon(&[tool(ToolKind::Fetch, "x")]),
            "chrome-icons/globe.svg"
        );
        assert_eq!(
            summary_icon(&[tool(ToolKind::Read, "a"), tool(ToolKind::Execute, "b")]),
            "chrome-icons/wrench.svg"
        );
        assert_eq!(summary_icon(&[thought("a")]), THOUGHT_ICON);
    }

    #[test]
    fn activity_is_the_running_call_or_else_the_last_one() {
        let items = [
            running(ToolKind::Execute, "cargo test"),
            tool(ToolKind::Read, "a"),
        ];
        let Activity::Call(call) = activity(&items) else {
            panic!("expected the running call");
        };
        assert_eq!(call.title, "cargo test");

        let items = [tool(ToolKind::Read, "a"), tool(ToolKind::Execute, "ls")];
        let Activity::Call(call) = activity(&items) else {
            panic!("expected the last call");
        };
        assert_eq!(call.title, "ls");
    }

    #[test]
    fn activity_is_thinking_when_the_newest_item_is_a_thought() {
        let items = [tool(ToolKind::Read, "a"), thought("hm")];
        assert!(matches!(activity(&items), Activity::Thinking));
        assert!(matches!(activity(&[thought("hm")]), Activity::Thinking));
        assert!(matches!(activity(&[permission()]), Activity::Thinking));
    }

    #[test]
    fn the_latest_thought_is_the_last_reasoning_item() {
        let items = [thought("one"), tool(ToolKind::Read, "a"), thought("two")];
        assert_eq!(latest_thought(&items), Some("two"));
        assert_eq!(latest_thought(&[tool(ToolKind::Read, "a")]), None);
    }

    #[test]
    fn thought_previews_keep_the_first_sentence_or_240_characters() {
        assert_eq!(
            thought_preview("Looking at the layout.  Then I will\nedit it."),
            "Looking at the layout."
        );
        assert_eq!(
            thought_preview("**Plan**\nCheck `files`"),
            "Plan Check files"
        );
        assert_eq!(thought_preview("Done!"), "Done!");
        let long = "a".repeat(300);
        assert_eq!(thought_preview(&long), format!("{}…", "a".repeat(240)));
    }

    #[test]
    fn first_line_skips_blank_lines() {
        assert_eq!(first_line("\n  hi there\nmore"), "hi there");
        assert_eq!(first_line("  \n "), "");
    }
}
