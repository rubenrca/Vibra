mod conversation;
mod process;

pub use conversation::HostedConversation;
pub use process::HostedProcess;

use crate::domain::workspace::HostedAgentKind;
use crate::ports::harness::{HarnessError, HarnessPort, HarnessSpawn};

/// Resolves first-party spawn argv. Policy flags are never added here.
pub struct OfficialHarnessPort;

impl HarnessPort for OfficialHarnessPort {
    fn spawn_argv(&self, request: &HarnessSpawn) -> Result<Vec<String>, HarnessError> {
        official_spawn_argv(request.agent, request.vendor_session_id.as_deref())
    }
}

pub fn official_spawn_argv(
    agent: HostedAgentKind,
    vendor_session_id: Option<&str>,
) -> Result<Vec<String>, HarnessError> {
    let mut argv = match agent {
        HostedAgentKind::Claude => vec![
            "claude".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--include-partial-messages".into(),
            "--input-format".into(),
            "stream-json".into(),
        ],
        HostedAgentKind::Codex => vec!["codex".into(), "app-server".into()],
        HostedAgentKind::Grok => vec!["grok".into(), "agent".into(), "stdio".into()],
    };
    if let Some(session_id) = vendor_session_id.filter(|value| !value.is_empty()) {
        match agent {
            HostedAgentKind::Claude => {
                argv.push(format!("--resume={session_id}"));
            }
            HostedAgentKind::Codex | HostedAgentKind::Grok => {}
        }
    }
    assert_no_policy_flags(&argv)?;
    Ok(argv)
}

/// TUI resume of the same vendor session. `None` if the CLI cannot resume.
pub fn tui_resume_argv(agent: HostedAgentKind, vendor_session_id: &str) -> Option<Vec<String>> {
    let id = vendor_session_id.trim();
    if id.is_empty() {
        return None;
    }
    match agent {
        HostedAgentKind::Claude => Some(vec!["claude".into(), format!("--resume={id}")]),
        HostedAgentKind::Codex => Some(vec!["codex".into(), "resume".into(), id.to_owned()]),
        HostedAgentKind::Grok => None,
    }
}

pub fn assert_no_policy_flags(argv: &[String]) -> Result<(), HarnessError> {
    const FORBIDDEN: &[&str] = &[
        "bypassPermissions",
        "bypass-permissions",
        "bypass_permissions",
        "agent-full-access",
        "dangerously-skip-permissions",
        "danger-full-access",
        "yolo",
        "approvalPolicy",
        "--dangerously-skip-permissions",
        "claude-agent-acp",
        "codex-acp",
    ];
    for argument in argv {
        if FORBIDDEN
            .iter()
            .any(|flag| argument == flag || argument.contains(flag))
        {
            return Err(HarnessError::Protocol(format!(
                "flag de política prohibido en el spawn: {argument}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_spawn_is_stream_json_without_policy_flags() {
        let argv = official_spawn_argv(HostedAgentKind::Claude, None).unwrap();
        assert_eq!(argv[0], "claude");
        assert!(argv.contains(&"--output-format".into()));
        assert!(argv.contains(&"stream-json".into()));
        assert!(!argv.iter().any(|argument| argument.contains("bypass")));
        assert!(!argv.iter().any(|argument| argument.contains("acp")));
        assert_no_policy_flags(&argv).unwrap();
    }

    #[test]
    fn claude_resume_uses_equals_form() {
        let argv = official_spawn_argv(HostedAgentKind::Claude, Some("sess-1")).unwrap();
        assert!(argv.iter().any(|argument| argument == "--resume=sess-1"));
    }

    #[test]
    fn grok_uses_official_acp_stdio() {
        let argv = official_spawn_argv(HostedAgentKind::Grok, None).unwrap();
        assert_eq!(argv, vec!["grok", "agent", "stdio"]);
        assert_no_policy_flags(&argv).unwrap();
    }

    #[test]
    fn codex_uses_official_app_server() {
        let argv = official_spawn_argv(HostedAgentKind::Codex, Some("thread-1")).unwrap();
        assert_eq!(argv, vec!["codex", "app-server"]);
        assert_no_policy_flags(&argv).unwrap();
    }

    #[test]
    fn policy_flags_are_rejected() {
        let error = assert_no_policy_flags(&["claude".into(), "bypassPermissions".into()])
            .unwrap_err();
        assert!(matches!(error, HarnessError::Protocol(_)));
    }

    #[test]
    fn tui_resume_is_off_without_an_id_or_for_grok() {
        assert!(tui_resume_argv(HostedAgentKind::Claude, "").is_none());
        assert!(tui_resume_argv(HostedAgentKind::Grok, "sess").is_none());
        assert_eq!(
            tui_resume_argv(HostedAgentKind::Claude, "abc").unwrap()[1],
            "--resume=abc"
        );
    }
}
