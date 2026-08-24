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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentNotificationDelivery {
    /// A macOS notification banner, used only while Vibra is in the background.
    Banner,
    /// A lightweight completion cue while the user is working in another pane.
    Sound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentNotification {
    pub kind: AgentNotificationKind,
    pub delivery: AgentNotificationDelivery,
}

/// Notify only on a real transition. Background activity gets a system banner;
/// foreground activity never does, but a finished agent in another pane gets a sound.
pub fn should_notify_agent(
    previous: Option<&AgentActivitySnapshot>,
    current: Option<&AgentActivitySnapshot>,
    pane_selected: bool,
    window_active: bool,
    notifications_enabled: bool,
) -> Option<AgentNotification> {
    if !notifications_enabled {
        return None;
    }
    let previous = previous?;
    let kind = if let Some(current) = current {
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
    } else {
        // Process gone / session-end: still a finish if it was working.
        (previous.state == AgentRuntimeState::Working).then_some(AgentNotificationKind::Finished)
    }?;

    let delivery = if !window_active {
        AgentNotificationDelivery::Banner
    } else if !pane_selected && kind == AgentNotificationKind::Finished {
        AgentNotificationDelivery::Sound
    } else {
        return None;
    };
    Some(AgentNotification { kind, delivery })
}

pub fn play_completion_sound() {
    #[cfg(target_os = "macos")]
    unsafe {
        vibra_notification_play_completion_sound();
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
    unsafe {
        vibra_notification_request_authorization();
    }
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
    fn vibra_notification_play_completion_sound();
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
    fn skips_first_sighting_and_the_selected_foreground_pane() {
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
            Some(AgentNotification {
                kind: AgentNotificationKind::Finished,
                delivery: AgentNotificationDelivery::Banner,
            })
        );
        assert_eq!(
            should_notify_agent(Some(&working), None, false, true, true),
            Some(AgentNotification {
                kind: AgentNotificationKind::Finished,
                delivery: AgentNotificationDelivery::Sound,
            })
        );
        assert_eq!(
            should_notify_agent(Some(&idle), None, false, true, true),
            None
        );
    }

    #[test]
    fn permissions_use_banners_only_in_the_background_and_ignore_repeats() {
        let working = snapshot("Codex", AgentRuntimeState::Working, None);
        let permission = snapshot(
            "Codex",
            AgentRuntimeState::Waiting,
            Some(AgentAttention::Permission),
        );
        assert_eq!(
            should_notify_agent(Some(&working), Some(&permission), false, false, true),
            Some(AgentNotification {
                kind: AgentNotificationKind::NeedsPermission,
                delivery: AgentNotificationDelivery::Banner,
            })
        );
        assert_eq!(
            should_notify_agent(Some(&working), Some(&permission), false, true, true),
            None
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
            Some(AgentNotification {
                kind: AgentNotificationKind::NeedsAttention,
                delivery: AgentNotificationDelivery::Banner,
            })
        );
    }

    #[test]
    fn foreground_completion_only_sounds_for_another_pane() {
        let working = snapshot("Codex", AgentRuntimeState::Working, None);
        let idle = snapshot("Codex", AgentRuntimeState::Idle, None);

        assert_eq!(
            should_notify_agent(Some(&working), Some(&idle), false, true, true),
            Some(AgentNotification {
                kind: AgentNotificationKind::Finished,
                delivery: AgentNotificationDelivery::Sound,
            })
        );
        assert_eq!(
            should_notify_agent(Some(&working), Some(&idle), true, true, true),
            None
        );
    }
}
