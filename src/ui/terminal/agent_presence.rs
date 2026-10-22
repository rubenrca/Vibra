use crate::domain::agents::{AgentAttention, AgentKind, AgentRuntimeState, contains_marker};
use crate::ports::terminal::{
    ProgramBlock, ProgramState, TerminalAgentKindSource, TerminalAgentPresence,
    TerminalAgentStateSource, TerminalProgramStatus, TerminalSnapshot,
};

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
    // Process and program identities are re-read every time; only names read
    // off the title or screen outlive the evidence that revealed them.
    let pinned = pinned
        .filter(|pinned| {
            matches!(
                pinned.kind_source,
                TerminalAgentKindSource::Title | TerminalAgentKindSource::Screen
            )
        })
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
        state_source: TerminalAgentStateSource::Heuristic,
        attention: None,
        process_id: (kind_source == TerminalAgentKindSource::Process)
            .then_some(process_id)
            .flatten(),
    })
}

/// Lays the program's own status report over what detection inferred. The
/// report decides activity outright; its `app` names the agent only when no
/// process already did. `boot_work` marks work the program started on its
/// own right after launch (Codex reports a working blip while it boots).
pub(super) fn apply_program_status(
    detected: Option<TerminalAgentPresence>,
    program: Option<&TerminalProgramStatus>,
    process_id: Option<u32>,
    boot_work: bool,
) -> Option<TerminalAgentPresence> {
    let Some(program) = program else {
        return detected;
    };
    let reported = program.app.as_deref().and_then(AgentKind::from_program_app);
    let (kind, kind_source) = match (&detected, reported) {
        (Some(detected), reported) if detected.kind_source == TerminalAgentKindSource::Process => {
            // A report naming another agent belongs to someone else's TTY.
            if reported.is_some_and(|kind| !detected.kind.eq_ignore_ascii_case(kind.display_name()))
            {
                return Some(detected.clone());
            }
            (detected.kind.clone(), TerminalAgentKindSource::Process)
        }
        (_, Some(kind)) => (
            kind.display_name().to_owned(),
            TerminalAgentKindSource::Program,
        ),
        // Not an agent's report: a build tool's progress is not agent activity.
        (_, None) => return detected,
    };
    let (state, attention) = match program.state {
        ProgramState::Working if boot_work => (AgentRuntimeState::Idle, None),
        ProgramState::Working => (AgentRuntimeState::Working, None),
        ProgramState::Blocked => (
            AgentRuntimeState::Waiting,
            Some(match program.block {
                Some(ProgramBlock::Permission) => AgentAttention::Permission,
                Some(ProgramBlock::Question) => AgentAttention::Question,
                Some(ProgramBlock::Auth) | None => AgentAttention::Notification,
            }),
        ),
        ProgramState::Idle | ProgramState::Done | ProgramState::Error => {
            (AgentRuntimeState::Idle, None)
        }
    };
    Some(TerminalAgentPresence {
        kind,
        kind_source,
        state,
        state_source: TerminalAgentStateSource::Program,
        attention,
        process_id: detected
            .and_then(|detected| detected.process_id)
            .or(process_id),
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
    let glyph = title_glyph_state(&title);
    if glyph == Some(AgentRuntimeState::Waiting)
        || any(&title, &waiting_markers)
        || any(&chrome, &waiting_markers)
    {
        AgentRuntimeState::Waiting
    } else if let Some(state) = glyph {
        state
    } else if any(&screen, &interrupt_markers)
        || any(&title, &working_markers)
        || any(&chrome, &working_markers)
    {
        AgentRuntimeState::Working
    } else {
        AgentRuntimeState::Idle
    }
}

/// Agents that paint their state into the window title say it more reliably
/// than any screen text: Claude Code (`✳` at rest, a braille or quarter-circle
/// spinner at work, until it reports OSC 7501 instead) and Gemini CLI (`◇` at
/// rest, `✦` or `⏲` at work, `✋` on a permission prompt).
fn title_glyph_state(title: &str) -> Option<AgentRuntimeState> {
    if title.contains('✋') {
        Some(AgentRuntimeState::Waiting)
    } else if title
        .chars()
        .any(|ch| matches!(ch, '\u{2801}'..='\u{28ff}' | '\u{25d0}'..='\u{25d3}' | '✦' | '⏲'))
    {
        Some(AgentRuntimeState::Working)
    } else if title.trim_start().starts_with('✳') || title.contains('◇') {
        Some(AgentRuntimeState::Idle)
    } else {
        None
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
