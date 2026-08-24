use crate::infrastructure::automation::AgentRuntimeState;
use crate::ports::terminal::TerminalAgentState;

pub(crate) fn agent_runtime_state_label(state: AgentRuntimeState) -> &'static str {
    match state {
        AgentRuntimeState::Idle => "idle",
        AgentRuntimeState::Working => "working",
        AgentRuntimeState::Waiting => "waiting",
    }
}

/// Maps the terminal heuristic state to the shared agent activity state.
pub(crate) fn terminal_agent_state_to_runtime_state(
    state: TerminalAgentState,
) -> AgentRuntimeState {
    match state {
        TerminalAgentState::Idle => AgentRuntimeState::Idle,
        TerminalAgentState::Working => AgentRuntimeState::Working,
        TerminalAgentState::Waiting => AgentRuntimeState::Waiting,
    }
}
