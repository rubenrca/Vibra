use std::time::{Duration, Instant};

use gpui::Context;
use uuid::Uuid;

use crate::infrastructure::automation::{
    AgentAttention, AgentKind, AgentRuntimeState, AutomationCommand, AutomationIncoming,
    AutomationResponse,
};
use crate::infrastructure::notifications::{
    AgentActivitySnapshot, AgentNotificationDelivery, agent_notification_copy, should_notify_agent,
};
use crate::ports::terminal::{TerminalAgentKindSource, TerminalAgentPresence, TerminalAgentState};

use super::WorkspaceView;

const HOOK_OBSERVATION_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub(super) struct HookAgentPresence {
    kind: Option<AgentKind>,
    state: AgentRuntimeState,
    attention: Option<AgentAttention>,
    model: Option<String>,
    session_id: Option<String>,
    observed_at: Instant,
}

pub(super) struct ResolvedAgentPresence {
    pub(super) kind: String,
    pub(super) state: AgentRuntimeState,
    pub(super) attention: Option<AgentAttention>,
    pub(super) model: Option<String>,
    kind_source: &'static str,
    state_source: &'static str,
    process_id: Option<u32>,
    session_id: Option<String>,
}

fn agent_runtime_state_label(state: AgentRuntimeState) -> &'static str {
    match state {
        AgentRuntimeState::Idle => "idle",
        AgentRuntimeState::Working => "working",
        AgentRuntimeState::Waiting => "waiting",
    }
}

/// Maps the terminal heuristic state to the shared agent activity state.
fn terminal_agent_state_to_runtime_state(state: TerminalAgentState) -> AgentRuntimeState {
    match state {
        TerminalAgentState::Idle => AgentRuntimeState::Idle,
        TerminalAgentState::Working => AgentRuntimeState::Working,
        TerminalAgentState::Waiting => AgentRuntimeState::Waiting,
    }
}

/// Process identity wins over hooks; matching, unexpired hooks supply activity.
fn resolve_agent_presence(
    detected: Option<&TerminalAgentPresence>,
    hook: Option<&HookAgentPresence>,
    now: Instant,
) -> Option<ResolvedAgentPresence> {
    let hook = hook
        .filter(|presence| now.duration_since(presence.observed_at) <= HOOK_OBSERVATION_TTL)
        .filter(|hook| {
            detected.is_none_or(|detected| {
                detected.kind_source != TerminalAgentKindSource::Process
                    || hook
                        .kind
                        .is_none_or(|kind| detected.kind.eq_ignore_ascii_case(kind.display_name()))
            })
        });
    let terminal_kind = detected.map(|presence| (presence.kind.as_str(), presence.kind_source));
    let (kind, kind_source) = match (terminal_kind, hook.and_then(|presence| presence.kind)) {
        (Some((kind, TerminalAgentKindSource::Process)), _) => (kind.to_owned(), "process"),
        (_, Some(kind)) => (kind.display_name().to_owned(), "hook"),
        (Some((kind, TerminalAgentKindSource::Title)), _) => (kind.to_owned(), "title"),
        (Some((kind, TerminalAgentKindSource::Screen)), _) => (kind.to_owned(), "screen"),
        (None, None) => return None,
    };
    let (state, state_source, attention) = if let Some(hook) = hook {
        (hook.state, "hook", hook.attention)
    } else {
        (
            terminal_agent_state_to_runtime_state(detected?.state),
            "heuristic",
            None,
        )
    };
    Some(ResolvedAgentPresence {
        kind,
        state,
        attention,
        model: hook.and_then(|presence| presence.model.clone()),
        kind_source,
        state_source,
        process_id: detected.and_then(|presence| presence.process_id),
        session_id: hook.and_then(|presence| presence.session_id.clone()),
    })
}

impl WorkspaceView {
    pub(super) fn handle_automation_request(
        &mut self,
        request: AutomationIncoming,
        cx: &mut Context<Self>,
    ) {
        let pane_id = request.envelope.pane_id;
        let authorized = self
            .automation_tokens
            .get(&pane_id)
            .is_some_and(|token| *token == request.envelope.token);
        if !authorized {
            let _ = request
                .response
                .send(AutomationResponse::failure("capacidad inválida o expirada"));
            return;
        }

        let changed = match request.envelope.command {
            AutomationCommand::SetAgentState { state } => {
                self.set_hook_agent_presence(pane_id, None, state, None, None, None);
                true
            }
            AutomationCommand::SetAgentPresence {
                kind,
                state,
                attention,
                model,
                session_id,
            } => {
                self.set_hook_agent_presence(
                    pane_id,
                    Some(kind),
                    state,
                    attention,
                    model,
                    session_id,
                );
                true
            }
            AutomationCommand::ClearAgentPresence { session_id } => {
                let clear = self
                    .hook_agent_presence
                    .get(&pane_id)
                    .is_none_or(|presence| {
                        session_id.is_none() || presence.session_id.as_ref() == session_id.as_ref()
                    });
                if clear {
                    self.hook_agent_presence.remove(&pane_id);
                }
                clear
            }
        };
        if changed {
            self.publish_agent_activity(pane_id);
            cx.notify();
        }
        let _ = request.response.send(AutomationResponse::success(
            self.agent_status_value(pane_id),
        ));
    }

    fn set_hook_agent_presence(
        &mut self,
        pane_id: Uuid,
        kind: Option<AgentKind>,
        state: AgentRuntimeState,
        attention: Option<AgentAttention>,
        model: Option<String>,
        session_id: Option<String>,
    ) {
        let now = Instant::now();
        let session_changed = self
            .hook_agent_presence
            .get(&pane_id)
            .and_then(|presence| presence.session_id.as_deref())
            .zip(session_id.as_deref())
            .is_some_and(|(previous, current)| previous != current);
        if session_changed {
            self.agent_names.remove(&pane_id);
        }
        let entry = self
            .hook_agent_presence
            .entry(pane_id)
            .or_insert_with(|| HookAgentPresence {
                kind: None,
                state,
                attention: None,
                model: None,
                session_id: None,
                observed_at: now,
            });
        if kind.is_some() {
            entry.kind = kind;
        }
        entry.state = state;
        entry.attention = attention;
        if model.is_some() || session_changed {
            entry.model = model;
        }
        if session_id.is_some() {
            entry.session_id = session_id;
        }
        entry.observed_at = now;
    }

    pub(super) fn resolved_agent_presence(&self, pane_id: Uuid) -> Option<ResolvedAgentPresence> {
        resolve_agent_presence(
            self.agent_presence.get(&pane_id),
            self.hook_agent_presence.get(&pane_id),
            Instant::now(),
        )
    }

    fn agent_status_value(&self, pane_id: Uuid) -> serde_json::Value {
        let presence = self.resolved_agent_presence(pane_id);
        serde_json::json!({
            "kind": presence.as_ref().map(|presence| presence.kind.as_str()),
            "state": presence.as_ref().map(|presence| agent_runtime_state_label(presence.state)),
            "attention": presence.as_ref().and_then(|presence| presence.attention).map(AgentAttention::label),
            "model": presence.as_ref().and_then(|presence| presence.model.as_deref()),
            "kindSource": presence.as_ref().map(|presence| presence.kind_source),
            "stateSource": presence.as_ref().map(|presence| presence.state_source),
            "source": presence.as_ref().map(|presence| presence.state_source),
            "processId": presence.as_ref().and_then(|presence| presence.process_id),
            "sessionId": presence.as_ref().and_then(|presence| presence.session_id.as_deref()),
        })
    }

    fn session_is_selected(&self, pane_id: Uuid) -> bool {
        self.snapshot
            .selected_session()
            .is_some_and(|session| session.id == pane_id)
    }

    pub(super) fn publish_agent_activity(&mut self, pane_id: Uuid) {
        let current = self
            .resolved_agent_presence(pane_id)
            .map(|presence| AgentActivitySnapshot {
                kind: presence.kind,
                state: presence.state,
                attention: presence.attention,
            });
        let previous = self.agent_activity_seen.get(&pane_id);
        if let Some(notification) = should_notify_agent(
            previous,
            current.as_ref(),
            self.session_is_selected(pane_id),
            self.window_is_active,
            self.settings.agent_notifications,
        ) {
            match notification.delivery {
                AgentNotificationDelivery::Banner => {
                    let agent = current
                        .as_ref()
                        .or(previous)
                        .map(|snapshot| snapshot.kind.as_str())
                        .unwrap_or("Agente");
                    let (title, body) = agent_notification_copy(notification.kind, agent);
                    crate::infrastructure::notifications::deliver(
                        &title,
                        &body,
                        &format!("vibra.agent.{pane_id}"),
                    );
                }
                AgentNotificationDelivery::Sound => {
                    crate::infrastructure::notifications::play_completion_sound();
                }
            }
        }
        match current {
            Some(snapshot) => {
                self.agent_activity_seen.insert(pane_id, snapshot);
            }
            None => {
                self.agent_activity_seen.remove(&pane_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(observed_at: Instant) -> HookAgentPresence {
        HookAgentPresence {
            kind: Some(AgentKind::Codex),
            state: AgentRuntimeState::Waiting,
            attention: Some(AgentAttention::Permission),
            model: Some("gpt-5".into()),
            session_id: Some("session-1".into()),
            observed_at,
        }
    }

    fn detected(kind: &str, kind_source: TerminalAgentKindSource) -> TerminalAgentPresence {
        TerminalAgentPresence {
            kind: kind.into(),
            kind_source,
            state: TerminalAgentState::Working,
            process_id: Some(42),
        }
    }

    #[test]
    fn matching_process_keeps_identity_and_uses_hook_activity() {
        let now = Instant::now();
        let detected = detected("codex", TerminalAgentKindSource::Process);
        let presence = resolve_agent_presence(Some(&detected), Some(&hook(now)), now).unwrap();
        assert_eq!(presence.kind, "codex");
        assert_eq!(presence.kind_source, "process");
        assert_eq!(presence.state, AgentRuntimeState::Waiting);
        assert_eq!(presence.state_source, "hook");
        assert_eq!(presence.attention, Some(AgentAttention::Permission));
        assert_eq!(presence.model.as_deref(), Some("gpt-5"));
        assert_eq!(presence.process_id, Some(42));
        assert_eq!(presence.session_id.as_deref(), Some("session-1"));
    }

    #[test]
    fn mismatched_process_discards_all_hook_metadata() {
        let now = Instant::now();
        let detected = detected("Claude", TerminalAgentKindSource::Process);
        let presence = resolve_agent_presence(Some(&detected), Some(&hook(now)), now).unwrap();
        assert_eq!(presence.kind, "Claude");
        assert_eq!(presence.kind_source, "process");
        assert_eq!(presence.state, AgentRuntimeState::Working);
        assert_eq!(presence.state_source, "heuristic");
        assert_eq!(presence.attention, None);
        assert_eq!(presence.model, None);
        assert_eq!(presence.session_id, None);
    }

    #[test]
    fn hooks_override_title_and_screen_identity() {
        let now = Instant::now();
        for source in [
            TerminalAgentKindSource::Title,
            TerminalAgentKindSource::Screen,
        ] {
            let detected = detected("Claude", source);
            let presence = resolve_agent_presence(Some(&detected), Some(&hook(now)), now).unwrap();
            assert_eq!(presence.kind, "Codex");
            assert_eq!(presence.kind_source, "hook");
            assert_eq!(presence.state_source, "hook");
        }
    }

    #[test]
    fn hook_expiry_has_an_inclusive_boundary_and_falls_back_to_detection() {
        let observed_at = Instant::now();
        let hook = hook(observed_at);
        let boundary = observed_at + HOOK_OBSERVATION_TTL;
        assert!(resolve_agent_presence(None, Some(&hook), boundary).is_some());
        let expired = boundary + Duration::from_nanos(1);
        assert!(resolve_agent_presence(None, Some(&hook), expired).is_none());
        let detected = detected("Codex", TerminalAgentKindSource::Screen);
        let presence = resolve_agent_presence(Some(&detected), Some(&hook), expired).unwrap();
        assert_eq!(presence.kind_source, "screen");
        assert_eq!(presence.state_source, "heuristic");
        assert_eq!(presence.model, None);
    }

    #[test]
    fn state_only_hooks_need_a_detected_identity() {
        let now = Instant::now();
        let hook = HookAgentPresence {
            kind: None,
            ..hook(now)
        };
        assert!(resolve_agent_presence(None, Some(&hook), now).is_none());
        assert!(resolve_agent_presence(None, None, now).is_none());
        let detected = detected("Claude", TerminalAgentKindSource::Process);
        let presence = resolve_agent_presence(Some(&detected), Some(&hook), now).unwrap();
        assert_eq!(presence.kind, "Claude");
        assert_eq!(presence.state, AgentRuntimeState::Waiting);
    }
}
