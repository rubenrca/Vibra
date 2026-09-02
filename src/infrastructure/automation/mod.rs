mod cli;
mod hooks;
mod server;
mod types;

pub use cli::*;
pub use hooks::*;
pub use server::*;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use serde_json::Value;
    use uuid::Uuid;

    struct TestHome(PathBuf);

    impl TestHome {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("vibra automation test-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn cli_parser_only_accepts_agent_status_reporting_commands() {
        assert!(matches!(
            parse_cli_command("+agent", &["working".into()]).unwrap(),
            AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Working
            }
        ));
        assert!(matches!(
            parse_cli_command(
                "+agent",
                &[
                    "presence".into(),
                    "codex".into(),
                    "waiting".into(),
                    "permission".into(),
                    "--model".into(),
                    "gpt-5.4".into(),
                    "--session".into(),
                    "session-1".into(),
                ],
            )
            .unwrap(),
            AutomationCommand::SetAgentPresence {
                kind: AgentKind::Codex,
                state: AgentRuntimeState::Waiting,
                attention: Some(AgentAttention::Permission),
                model: Some(model),
                session_id: Some(session_id),
            } if session_id == "session-1" && model == "gpt-5.4"
        ));
        assert!(matches!(
            parse_cli_command(
                "+agent",
                &["attention".into(), "claude".into(), "question".into()]
            )
            .unwrap(),
            AutomationCommand::SetAgentPresence {
                kind: AgentKind::Claude,
                state: AgentRuntimeState::Waiting,
                attention: Some(AgentAttention::Question),
                ..
            }
        ));
        assert!(matches!(
            parse_cli_command("+agent", &["clear".into()]).unwrap(),
            AutomationCommand::ClearAgentPresence { session_id: None }
        ));
        assert!(parse_cli_command("+agent", &["open".into(), "codex".into()]).is_err());
        assert!(parse_cli_command("+agent", &["prompt".into(), "reviewer".into()]).is_err());
        assert!(parse_cli_command("+agent", &["list".into()]).is_err());
        assert!(parse_cli_command("+pane", &["split".into(), "right".into()]).is_err());
        assert!(parse_cli_command("+skill", &[]).is_err());
        assert!(run_cli(&["+pane".into(), "split".into(), "right".into()]).is_err());
        assert!(run_cli(&["+skill".into()]).is_err());
    }

    #[test]
    fn agent_setup_merges_hooks_and_uninstall_leaves_user_hooks() {
        let home = TestHome::new();
        let claude_settings = home.0.join(".claude/settings.json");
        let codex_hooks = home.0.join(".codex/hooks.json");
        let existing =
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo keep"}]}]}}"#;
        write_text_atomically(&claude_settings, existing, 0o600).unwrap();
        write_text_atomically(&codex_hooks, existing, 0o600).unwrap();
        let agents = [AgentKind::Claude, AgentKind::Codex].into_iter().collect();

        let report =
            manage_agent_hooks(&home.0, &agents, AgentHookOperation::Install, false).unwrap();
        assert!(
            report["agents"]
                .as_array()
                .unwrap()
                .iter()
                .all(|agent| agent["changed"] == true)
        );
        assert!(home.0.join(".vibra/agent-hooks/vibra-claude.sh").exists());
        assert!(home.0.join(".vibra/agent-hooks/vibra-codex.sh").exists());
        assert!(!home.0.join(".codex/config.toml").exists());

        for path in [&claude_settings, &codex_hooks] {
            let config: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            let handlers: Vec<_> = config["hooks"]
                .as_object()
                .unwrap()
                .values()
                .flat_map(|groups| groups.as_array().unwrap())
                .flat_map(|group| group["hooks"].as_array().unwrap())
                .collect();
            assert!(
                handlers
                    .iter()
                    .any(|handler| handler["command"] == "echo keep")
            );
            assert!(handlers.iter().any(|handler| {
                handler["command"]
                    .as_str()
                    .is_some_and(|command| command.contains(".vibra/agent-hooks"))
                    && handler["timeout"] == 3
                    && handler.get("async").is_none()
            }));
        }

        let repeat =
            manage_agent_hooks(&home.0, &agents, AgentHookOperation::Install, false).unwrap();
        assert!(
            repeat["agents"]
                .as_array()
                .unwrap()
                .iter()
                .all(|agent| agent["changed"] == false)
        );

        fs::write(
            home.0.join(".vibra/agent-hooks/vibra-codex.sh"),
            "#!/bin/sh\nexit 0\n",
        )
        .unwrap();
        let status =
            manage_agent_hooks(&home.0, &agents, AgentHookOperation::Status, false).unwrap();
        assert_eq!(status["agents"][0]["installed"], true);
        assert_eq!(status["agents"][1]["installed"], false);
        let repaired =
            manage_agent_hooks(&home.0, &agents, AgentHookOperation::Install, false).unwrap();
        assert_eq!(repaired["agents"][1]["changed"], true);

        manage_agent_hooks(&home.0, &agents, AgentHookOperation::Uninstall, false).unwrap();
        assert!(!home.0.join(".vibra/agent-hooks/vibra-claude.sh").exists());
        assert!(!home.0.join(".vibra/agent-hooks/vibra-codex.sh").exists());
        for path in [&claude_settings, &codex_hooks] {
            let config: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(
                config["hooks"]["Stop"][0]["hooks"][0]["command"],
                "echo keep"
            );
        }
    }

    #[test]
    fn agent_hook_status_reads_each_agent_from_a_report() {
        let status = agent_hook_status_from_report(&serde_json::json!({
            "agents": [
                { "agent": "Claude", "installed": true },
                { "agent": "Codex", "installed": false },
            ]
        }));

        assert!(status.claude_installed);
        assert!(!status.codex_installed);
        assert!(status.any_installed());
        assert!(!status.all_installed());
    }

    #[test]
    fn repairing_a_managed_hook_does_not_rewrite_neighboring_user_hooks() {
        let command = "'/tmp/vibra hook.sh' prompt";
        let mut config = serde_json::json!({
            "hooks": {
                "UserPromptSubmit": [{
                    "matcher": "old-matcher",
                    "hooks": [
                        { "type": "command", "command": command, "timeout": 1, "async": true },
                        { "type": "command", "command": "echo user-hook" }
                    ]
                }]
            }
        });

        assert!(
            ensure_hook_entry(
                &mut config,
                "UserPromptSubmit",
                Some("new-matcher"),
                command
            )
            .unwrap()
        );
        let groups = config["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert!(groups.iter().any(|group| {
            group["matcher"] == "old-matcher"
                && group["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|handler| handler["command"] == "echo user-hook")
        }));
        assert!(groups.iter().any(|group| {
            group["matcher"] == "new-matcher"
                && group["hooks"].as_array().unwrap().iter().any(|handler| {
                    handler["command"] == command
                        && handler["timeout"] == 3
                        && handler.get("async").is_none()
                })
        }));
    }

    #[test]
    fn hook_events_normalize_to_presence_commands() {
        let session = serde_json::json!({ "session_id": "session-1" });
        assert!(matches!(
            agent_hook_command(AgentKind::Codex, "prompt", &session).unwrap(),
            AutomationCommand::SetAgentPresence {
                kind: AgentKind::Codex,
                state: AgentRuntimeState::Working,
                attention: None,
                model: None,
                session_id: Some(session_id),
            } if session_id == "session-1"
        ));
        assert!(matches!(
            agent_hook_command(
                AgentKind::Codex,
                "prompt",
                &serde_json::json!({ "session_id": "session-2", "model": { "id": "gpt-5.4" } }),
            )
            .unwrap(),
            AutomationCommand::SetAgentPresence {
                model: Some(model),
                ..
            } if model == "gpt-5.4"
        ));
        assert!(matches!(
            agent_hook_command(
                AgentKind::Claude,
                "notification",
                &serde_json::json!({ "notification_type": "permission_prompt" }),
            )
            .unwrap(),
            AutomationCommand::SetAgentPresence {
                kind: AgentKind::Claude,
                state: AgentRuntimeState::Waiting,
                attention: Some(AgentAttention::Permission),
                ..
            }
        ));
        assert!(matches!(
            agent_hook_command(AgentKind::Codex, "session-end", &session).unwrap(),
            AutomationCommand::ClearAgentPresence {
                session_id: Some(session_id),
            } if session_id == "session-1"
        ));
    }

    #[test]
    fn unix_socket_round_trips_a_capability_request() {
        let server = AutomationServer::start().unwrap();
        let path = server.path().to_path_buf();
        let pane_id = Uuid::new_v4();
        let token = Uuid::new_v4();
        let client = thread::spawn(move || {
            let mut stream = UnixStream::connect(path).unwrap();
            serde_json::to_writer(
                &mut stream,
                &AutomationEnvelope {
                    pane_id,
                    token,
                    command: AutomationCommand::SetAgentState {
                        state: AgentRuntimeState::Idle,
                    },
                },
            )
            .unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            serde_json::from_reader::<_, AutomationResponse>(stream).unwrap()
        });

        let incoming = server.receiver().recv_blocking().unwrap();
        assert_eq!(incoming.envelope.pane_id, pane_id);
        assert_eq!(incoming.envelope.token, token);
        incoming
            .response
            .send(AutomationResponse::success(
                serde_json::json!({ "count": 1 }),
            ))
            .unwrap();

        let response = client.join().unwrap();
        assert!(response.ok);
        assert_eq!(response.data.unwrap()["count"], 1);
    }

    #[test]
    fn automation_server_accepts_a_second_request_while_the_first_is_pending() {
        let server = AutomationServer::start().unwrap();
        let path = server.path().to_path_buf();
        let start_client = |path: PathBuf| {
            thread::spawn(move || {
                let mut stream = UnixStream::connect(path).unwrap();
                serde_json::to_writer(
                    &mut stream,
                    &AutomationEnvelope {
                        pane_id: Uuid::new_v4(),
                        token: Uuid::new_v4(),
                        command: AutomationCommand::SetAgentState {
                            state: AgentRuntimeState::Idle,
                        },
                    },
                )
                .unwrap();
                stream.shutdown(std::net::Shutdown::Write).unwrap();
                serde_json::from_reader::<_, AutomationResponse>(stream).unwrap()
            })
        };

        let first_client = start_client(path.clone());
        let first = server.receiver().recv_blocking().unwrap();
        let second_client = start_client(path);
        let receiver = server.receiver();
        let (incoming_tx, incoming_rx) = mpsc::channel();
        let relay = thread::spawn(move || incoming_tx.send(receiver.recv_blocking()).unwrap());

        let (second, concurrent) = match incoming_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(second) => (second.unwrap(), true),
            Err(_) => {
                first
                    .response
                    .send(AutomationResponse::success(Value::Null))
                    .unwrap();
                (
                    incoming_rx
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap()
                        .unwrap(),
                    false,
                )
            }
        };
        second
            .response
            .send(AutomationResponse::success(Value::Null))
            .unwrap();
        if concurrent {
            first
                .response
                .send(AutomationResponse::success(Value::Null))
                .unwrap();
        }

        relay.join().unwrap();
        assert!(first_client.join().unwrap().ok);
        assert!(second_client.join().unwrap().ok);
        assert!(concurrent, "the first response blocked the second request");
    }

    #[test]
    fn enqueue_fails_closed_when_the_request_channel_is_full() {
        let (sender, receiver) = async_channel::bounded(1);
        let (tx, _rx) = mpsc::channel();
        enqueue_automation_request(
            &sender,
            AutomationIncoming {
                envelope: AutomationEnvelope {
                    pane_id: Uuid::new_v4(),
                    token: Uuid::new_v4(),
                    command: AutomationCommand::SetAgentState {
                        state: AgentRuntimeState::Idle,
                    },
                },
                response: tx,
            },
        )
        .unwrap();
        let (tx, _rx) = mpsc::channel();
        let error = enqueue_automation_request(
            &sender,
            AutomationIncoming {
                envelope: AutomationEnvelope {
                    pane_id: Uuid::new_v4(),
                    token: Uuid::new_v4(),
                    command: AutomationCommand::SetAgentState {
                        state: AgentRuntimeState::Idle,
                    },
                },
                response: tx,
            },
        )
        .unwrap_err();
        assert_eq!(error, "demasiadas solicitudes de automatización");
        drop(receiver);
    }
}
