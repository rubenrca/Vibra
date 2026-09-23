use crate::domain::agents::{AgentKind, AgentRuntimeState};
use crate::ports::terminal::{TerminalAgentKindSource, TerminalAgentPresence, TerminalSnapshot};

pub(super) fn detect_agent_presence(
    title: &str,
    snapshot: &TerminalSnapshot,
    recent_text: Option<&str>,
    process_name: Option<&str>,
    process_id: Option<u32>,
) -> Option<TerminalAgentPresence> {
    let screen = recent_text
        .map(str::to_lowercase)
        .unwrap_or_else(|| visible_screen_text(snapshot));
    // Identity and activity deliberately use different evidence. Process names
    // are reliable at startup, titles are the next-best structured signal, and
    // screen text remains a fallback for wrappers and unsupported terminals.
    // Once a known shell owns the TTY again, old agent titles and scrollback
    // must not keep the pane looking live or make aliases target the shell.
    if process_name.is_some_and(is_interactive_shell_process_name) {
        return None;
    }
    let (kind, kind_source) = process_name
        .and_then(AgentKind::from_process_name)
        .map(|kind| (kind, TerminalAgentKindSource::Process))
        .or_else(|| AgentKind::from_text(title).map(|kind| (kind, TerminalAgentKindSource::Title)))
        .or_else(|| {
            AgentKind::from_text(&screen).map(|kind| (kind, TerminalAgentKindSource::Screen))
        })?;
    let state = agent_state_from_text(title, &screen);
    Some(TerminalAgentPresence {
        kind: kind.display_name().to_owned(),
        kind_source,
        state,
        process_id: (kind_source == TerminalAgentKindSource::Process)
            .then_some(process_id)
            .flatten(),
    })
}

pub(super) fn is_interactive_shell_process_name(process_name: &str) -> bool {
    let base = std::path::Path::new(process_name)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| process_name.to_ascii_lowercase());
    let base = base.strip_prefix('-').unwrap_or(&base);
    matches!(
        base,
        "sh" | "bash" | "zsh" | "fish" | "nu" | "pwsh" | "powershell" | "cmd" | "dash" | "ksh"
    )
}

fn visible_screen_text(snapshot: &TerminalSnapshot) -> String {
    let start = snapshot.lines.len().saturating_sub(14);
    let mut text = String::new();
    for line in &snapshot.lines[start..] {
        for cell in line.iter().filter(|cell| !cell.wide_spacer) {
            text.push_str(cell.text());
        }
        text.push('\n');
    }
    text.to_lowercase()
}

fn agent_state_from_text(title: &str, screen: &str) -> AgentRuntimeState {
    let mut visible = title.to_lowercase();
    visible.push('\n');
    visible.push_str(screen);
    // Grok paints the permission mode on the footer for the whole session.
    // "always-approve" would otherwise match the "approve" waiting marker.
    let visible = strip_permission_mode_chrome(&visible);
    let waiting_markers = [
        "action required",
        "allow once",
        "allow?",
        "approve",
        "do you want to continue",
        "don't ask again",
        "press enter to confirm",
        "waiting for input",
        "(y/n)",
        "[y/n]",
        "permission required",
        "permission requested",
    ];
    let working_markers = [
        "esc to interrupt",
        "ctrl+c to interrupt",
        "thinking",
        "generating response",
        "running tool",
        "running command",
        "running:",
        "preparing",
        "responding",
        "compacting",
    ];
    if waiting_markers
        .iter()
        .any(|marker| contains_marker(&visible, marker))
    {
        AgentRuntimeState::Waiting
    } else if working_markers
        .iter()
        .any(|marker| contains_marker(&visible, marker))
    {
        AgentRuntimeState::Working
    } else {
        AgentRuntimeState::Idle
    }
}

fn contains_marker(text: &str, marker: &str) -> bool {
    text.match_indices(marker).any(|(index, _)| {
        let before = text[..index].chars().next_back();
        let after = text[index + marker.len()..].chars().next();
        !before.is_some_and(|ch| ch.is_ascii_alphanumeric())
            && !after.is_some_and(|ch| ch.is_ascii_alphanumeric())
    })
}

fn strip_permission_mode_chrome(text: &str) -> String {
    [
        "always-approve",
        "always approve",
        "auto-approve",
        "auto approve",
    ]
    .into_iter()
    .fold(text.to_owned(), |text, badge| text.replace(badge, " "))
}
