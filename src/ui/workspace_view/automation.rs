use crate::domain::workspace::{PaneFocusDirection, PaneSplitDirection};
use crate::infrastructure::automation::{AgentRuntimeState, AutomationDirection};
use crate::ports::terminal::TerminalAgentState;

pub(crate) fn automation_split_direction(direction: AutomationDirection) -> PaneSplitDirection {
    match direction {
        AutomationDirection::Left => PaneSplitDirection::Left,
        AutomationDirection::Right => PaneSplitDirection::Right,
        AutomationDirection::Up => PaneSplitDirection::Up,
        AutomationDirection::Down => PaneSplitDirection::Down,
    }
}

pub(crate) fn automation_focus_direction(direction: AutomationDirection) -> PaneFocusDirection {
    match direction {
        AutomationDirection::Left => PaneFocusDirection::Left,
        AutomationDirection::Right => PaneFocusDirection::Right,
        AutomationDirection::Up => PaneFocusDirection::Up,
        AutomationDirection::Down => PaneFocusDirection::Down,
    }
}

pub(crate) fn agent_runtime_state_label(state: AgentRuntimeState) -> &'static str {
    match state {
        AgentRuntimeState::Idle => "idle",
        AgentRuntimeState::Working => "working",
        AgentRuntimeState::Waiting => "waiting",
    }
}

/// One label row inside a sessions sidebar tab (fixed width, ellipsis).
pub(crate) fn terminal_agent_state_to_runtime_state(
    state: TerminalAgentState,
) -> AgentRuntimeState {
    match state {
        TerminalAgentState::Idle => AgentRuntimeState::Idle,
        TerminalAgentState::Working => AgentRuntimeState::Working,
        TerminalAgentState::Waiting => AgentRuntimeState::Waiting,
    }
}

pub(crate) fn agent_wait_matches(
    current: Option<AgentRuntimeState>,
    until: &[AgentRuntimeState],
    require_activity: bool,
    saw_activity: bool,
) -> bool {
    current.is_some_and(|state| until.contains(&state)) && (!require_activity || saw_activity)
}

pub(crate) fn ensure_prompt_wait_supported(
    wait: bool,
    state_source: Option<&str>,
) -> Result<(), String> {
    if wait && state_source != Some("hook") {
        return Err(
            "el agente usa seguimiento heurístico; +agent prompt --wait requiere hooks de actividad. Usa --no-wait y luego +agent read"
                .to_owned(),
        );
    }
    Ok(())
}
