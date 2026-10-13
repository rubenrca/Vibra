use crate::domain::agents::{AgentKind, AgentRuntimeState, contains_marker};
use crate::ports::terminal::{TerminalAgentKindSource, TerminalAgentPresence, TerminalSnapshot};

/// Prompts, spinners and footers live in the last few lines of an agent's UI.
/// Everything above is conversation, where an agent may quote any marker.
const CHROME_LINES: usize = 8;

/// `pinned` is the presence previously detected for the same foreground
/// process. Its kind outlives the text that revealed it: agents print each
/// other's names, and a banner scrolling away does not mean the agent left.
pub(super) fn detect_agent_presence(
    title: &str,
    snapshot: &TerminalSnapshot,
    recent_text: Option<&str>,
    process_name: Option<&str>,
    process_id: Option<u32>,
    pinned: Option<&TerminalAgentPresence>,
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
    let detected = process_name
        .and_then(AgentKind::from_process_name)
        .map(|kind| {
            (
                kind.display_name().to_owned(),
                TerminalAgentKindSource::Process,
            )
        })
        .or_else(|| {
            AgentKind::from_text(title).map(|kind| {
                (
                    kind.display_name().to_owned(),
                    TerminalAgentKindSource::Title,
                )
            })
        })
        .or_else(|| {
            AgentKind::from_text(&screen).map(|kind| {
                (
                    kind.display_name().to_owned(),
                    TerminalAgentKindSource::Screen,
                )
            })
        });
    let pinned = pinned
        .filter(|pinned| pinned.kind_source != TerminalAgentKindSource::Process)
        .map(|pinned| (pinned.kind.clone(), pinned.kind_source));
    let (kind, kind_source) = match (detected, pinned) {
        (Some(process @ (_, TerminalAgentKindSource::Process)), _) => process,
        (
            Some(title @ (_, TerminalAgentKindSource::Title)),
            Some((_, TerminalAgentKindSource::Screen)),
        ) => title,
        (_, Some(pinned)) => pinned,
        (detected, None) => detected?,
    };
    let state = agent_state_from_text(title, &screen);
    Some(TerminalAgentPresence {
        kind,
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
    // Grok paints the permission mode on the footer for the whole session.
    // "always-approve" would otherwise match the "approve" waiting marker.
    let title = strip_permission_mode_chrome(&title.to_lowercase());
    let chrome = strip_permission_mode_chrome(&last_non_empty_lines(screen, CHROME_LINES));
    let screen = strip_permission_mode_chrome(screen);
    let waiting_markers = [
        "action required",
        "allow once",
        "allow?",
        "approve this",
        "do you want to continue",
        "don't ask again",
        "press enter to confirm",
        "waiting for input",
        "(y/n)",
        "[y/n]",
        "permission required",
        "permission requested",
    ];
    // Interrupt hints only appear in a busy agent's footer, so they count
    // anywhere in the window even when a tall todo list pushes them up.
    let interrupt_markers = ["esc to interrupt", "ctrl+c to interrupt"];
    let working_markers = [
        "thinking",
        "generating response",
        "running tool",
        "running command",
        "running:",
        "preparing",
        "responding",
        "compacting",
    ];
    let any =
        |text: &str, markers: &[&str]| markers.iter().any(|marker| contains_marker(text, marker));
    if any(&title, &waiting_markers) || any(&chrome, &waiting_markers) {
        AgentRuntimeState::Waiting
    } else if any(&screen, &interrupt_markers)
        || any(&title, &working_markers)
        || any(&chrome, &working_markers)
    {
        AgentRuntimeState::Working
    } else {
        AgentRuntimeState::Idle
    }
}

fn last_non_empty_lines(text: &str, count: usize) -> String {
    let lines: Vec<_> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
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
