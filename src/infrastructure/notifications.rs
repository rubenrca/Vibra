//! Local macOS notifications for agent activity, plus the decision policy.

use crate::infrastructure::automation::{AgentAttention, AgentRuntimeState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActivitySnapshot {
    pub kind: String,
    pub state: AgentRuntimeState,
    pub attention: Option<AgentAttention>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentNotificationKind {
    Finished,
    NeedsPermission,
    NeedsAttention,
}

/// Rank used to pick the most urgent agent in a workspace or tab.
pub fn agent_activity_rank(state: AgentRuntimeState, attention: Option<AgentAttention>) -> u8 {
    match (state, attention) {
        (AgentRuntimeState::Idle, _) => 0,
        (AgentRuntimeState::Working, _) => 1,
        (AgentRuntimeState::Waiting, None) => 2,
        (AgentRuntimeState::Waiting, Some(AgentAttention::Notification)) => 3,
        (AgentRuntimeState::Waiting, Some(AgentAttention::Question | AgentAttention::Plan)) => 4,
        (AgentRuntimeState::Waiting, Some(AgentAttention::Permission)) => 5,
    }
}

/// Notify only on a real transition, and only when the user cannot see that pane.
pub fn should_notify_agent(
    previous: Option<&AgentActivitySnapshot>,
    current: Option<&AgentActivitySnapshot>,
    session_visible: bool,
    window_active: bool,
    notifications_enabled: bool,
) -> Option<AgentNotificationKind> {
    if !notifications_enabled {
        return None;
    }
    let current = current?;
    let previous = previous?;
    if session_visible && window_active {
        return None;
    }
    if previous.kind.eq_ignore_ascii_case(&current.kind)
        && previous.state == current.state
        && previous.attention == current.attention
    {
        return None;
    }
    match current.state {
        AgentRuntimeState::Waiting
            if current.attention == Some(AgentAttention::Permission)
                && (previous.state != AgentRuntimeState::Waiting
                    || previous.attention != current.attention) =>
        {
            Some(AgentNotificationKind::NeedsPermission)
        }
        AgentRuntimeState::Waiting if previous.state != AgentRuntimeState::Waiting => {
            Some(AgentNotificationKind::NeedsAttention)
        }
        AgentRuntimeState::Idle if previous.state == AgentRuntimeState::Working => {
            Some(AgentNotificationKind::Finished)
        }
        _ => None,
    }
}

pub fn agent_notification_copy(kind: AgentNotificationKind, agent: &str) -> (String, String) {
    match kind {
        AgentNotificationKind::Finished => (
            format!("{agent} terminó"),
            "El agente está en espera.".to_owned(),
        ),
        AgentNotificationKind::NeedsPermission => (
            format!("{agent} pide permiso"),
            "Hay una acción que requiere tu aprobación.".to_owned(),
        ),
        AgentNotificationKind::NeedsAttention => (
            format!("{agent} necesita tu atención"),
            "El agente está esperando una respuesta.".to_owned(),
        ),
    }
}

pub fn request_authorization() {
    #[cfg(target_os = "macos")]
    {
        if !running_in_app_bundle() {
            return;
        }
        unsafe {
            vibra_notification_request_authorization();
        }
    }
}

fn running_in_app_bundle() -> bool {
    std::env::current_exe()
        .ok()
        .is_some_and(|path| {
            path.components()
                .any(|component| component.as_os_str().to_string_lossy().ends_with(".app"))
        })
}

pub fn deliver(title: &str, body: &str, identifier: &str) {
    #[cfg(target_os = "macos")]
    {
        let title = std::ffi::CString::new(title).ok();
        let body = std::ffi::CString::new(body).ok();
        let identifier = std::ffi::CString::new(identifier).ok();
        if let (Some(title), Some(body), Some(identifier)) = (title, body, identifier) {
            unsafe {
                vibra_notification_deliver(title.as_ptr(), body.as_ptr(), identifier.as_ptr());
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (title, body, identifier);
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn vibra_notification_request_authorization();
    fn vibra_notification_deliver(
        title: *const std::ffi::c_char,
        body: *const std::ffi::c_char,
        identifier: *const std::ffi::c_char,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        kind: &str,
        state: AgentRuntimeState,
        attention: Option<AgentAttention>,
    ) -> AgentActivitySnapshot {
        AgentActivitySnapshot {
            kind: kind.to_owned(),
            state,
            attention,
        }
    }

    #[test]
    fn permission_outranks_generic_waiting() {
        assert!(
            agent_activity_rank(AgentRuntimeState::Waiting, Some(AgentAttention::Permission))
                > agent_activity_rank(AgentRuntimeState::Waiting, None)
        );
        assert!(
            agent_activity_rank(AgentRuntimeState::Waiting, None)
                > agent_activity_rank(AgentRuntimeState::Working, None)
        );
    }

    #[test]
    fn skips_first_sighting_and_visible_focused_session() {
        let working = snapshot("Claude", AgentRuntimeState::Working, None);
        let idle = snapshot("Claude", AgentRuntimeState::Idle, None);
        assert_eq!(
            should_notify_agent(None, Some(&idle), false, false, true),
            None
        );
        assert_eq!(
            should_notify_agent(Some(&working), Some(&idle), true, true, true),
            None
        );
        assert_eq!(
            should_notify_agent(Some(&working), Some(&idle), true, false, true),
            Some(AgentNotificationKind::Finished)
        );
    }

    #[test]
    fn notifies_permission_and_ignores_repeats() {
        let working = snapshot("Codex", AgentRuntimeState::Working, None);
        let permission = snapshot(
            "Codex",
            AgentRuntimeState::Waiting,
            Some(AgentAttention::Permission),
        );
        assert_eq!(
            should_notify_agent(Some(&working), Some(&permission), false, true, true),
            Some(AgentNotificationKind::NeedsPermission)
        );
        assert_eq!(
            should_notify_agent(Some(&permission), Some(&permission), false, true, true),
            None
        );
        assert_eq!(
            should_notify_agent(Some(&working), Some(&permission), false, true, false),
            None
        );
    }

    #[test]
    fn waiting_without_permission_is_generic_attention() {
        let idle = snapshot("Grok", AgentRuntimeState::Idle, None);
        let waiting = snapshot("Grok", AgentRuntimeState::Waiting, None);
        assert_eq!(
            should_notify_agent(Some(&idle), Some(&waiting), false, false, true),
            Some(AgentNotificationKind::NeedsAttention)
        );
    }
}
