//! Pure translation between ACP JSON and the normalized chat model. Nothing
//! here does I/O, so the protocol shapes are covered directly by tests.

use serde_json::{Value, json};

use crate::domain::agent_chat::{
    ChatCapabilities, ChatEvent, PermissionItem, PermissionOption, PermissionOptionKind,
    PermissionResolution, PlanEntry, PlanEntryStatus, ToolCallItem, ToolCallUpdate, ToolContent,
    ToolKind, ToolStatus,
};

use super::rpc::RpcError;

/// Which optional session calls the agent advertised in `initialize`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SessionSupport {
    pub resume: bool,
    pub close: bool,
}

/// What `initialize` told us about the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Negotiated {
    pub agent_name: Option<String>,
    pub capabilities: ChatCapabilities,
    pub support: SessionSupport,
    pub protocol_version: Option<i64>,
}

/// Events for the `update` object of one `session/update` notification.
/// Unknown kinds (commands, modes, usage) produce nothing.
pub(super) fn session_update_events(update: &Value) -> Vec<ChatEvent> {
    match str_field(update, "sessionUpdate") {
        Some("user_message_chunk") => chunk(update, |text| ChatEvent::UserMessageChunk { text }),
        Some("agent_message_chunk") => chunk(update, |text| ChatEvent::AgentMessageChunk { text }),
        Some("agent_thought_chunk") => chunk(update, |text| ChatEvent::ThoughtChunk { text }),
        Some("tool_call") => vec![ChatEvent::ToolCall(tool_call_item(update))],
        Some("tool_call_update") => vec![ChatEvent::ToolCallUpdate(tool_call_update(update))],
        Some("plan") => vec![ChatEvent::Plan(plan_entries(update))],
        _ => Vec::new(),
    }
}

fn chunk(update: &Value, event: impl FnOnce(String) -> ChatEvent) -> Vec<ChatEvent> {
    update
        .get("content")
        .and_then(content_block_text)
        .filter(|text| !text.is_empty())
        .map(event)
        .into_iter()
        .collect()
}

/// Text for a content block. Images become a placeholder; other kinds are not
/// shown in the chat.
fn content_block_text(block: &Value) -> Option<String> {
    match str_field(block, "type")? {
        "text" => str_field(block, "text").map(str::to_owned),
        "image" => Some("[image]".to_owned()),
        _ => None,
    }
}

fn tool_call_item(update: &Value) -> ToolCallItem {
    ToolCallItem {
        id: str_field(update, "toolCallId")
            .unwrap_or_default()
            .to_owned(),
        title: str_field(update, "title").unwrap_or("Tool call").to_owned(),
        kind: str_field(update, "kind").map_or(ToolKind::Other, ToolKind::from_acp),
        status: str_field(update, "status").map_or(ToolStatus::Pending, ToolStatus::from_acp),
        content: update
            .get("content")
            .and_then(Value::as_array)
            .map(|items| tool_contents(items))
            .unwrap_or_default(),
        locations: update.get("locations").and_then(paths).unwrap_or_default(),
    }
}

/// A partial update: only the fields the agent sent become `Some`.
fn tool_call_update(update: &Value) -> ToolCallUpdate {
    ToolCallUpdate {
        id: str_field(update, "toolCallId")
            .unwrap_or_default()
            .to_owned(),
        title: str_field(update, "title").map(str::to_owned),
        kind: str_field(update, "kind").map(ToolKind::from_acp),
        status: str_field(update, "status").map(ToolStatus::from_acp),
        content: update
            .get("content")
            .and_then(Value::as_array)
            .map(|items| tool_contents(items)),
        locations: update.get("locations").and_then(paths),
    }
}

fn tool_contents(items: &[Value]) -> Vec<ToolContent> {
    items.iter().filter_map(tool_content).collect()
}

fn tool_content(item: &Value) -> Option<ToolContent> {
    match str_field(item, "type")? {
        "content" => content_block_text(item.get("content")?).map(ToolContent::Text),
        "diff" => Some(ToolContent::Diff {
            path: str_field(item, "path")?.to_owned(),
            old_text: str_field(item, "oldText").map(str::to_owned),
            new_text: str_field(item, "newText")?.to_owned(),
        }),
        "terminal" => Some(ToolContent::Terminal {
            terminal_id: str_field(item, "terminalId")?.to_owned(),
        }),
        _ => None,
    }
}

/// Paths from a `locations` array. `None` when the value is not an array.
fn paths(value: &Value) -> Option<Vec<String>> {
    Some(
        value
            .as_array()?
            .iter()
            .filter_map(|location| str_field(location, "path"))
            .map(str::to_owned)
            .collect(),
    )
}

fn plan_entries(update: &Value) -> Vec<PlanEntry> {
    update
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| PlanEntry {
                    content: str_field(entry, "content").unwrap_or_default().to_owned(),
                    status: match str_field(entry, "status") {
                        Some("in_progress") => PlanEntryStatus::InProgress,
                        Some("completed") => PlanEntryStatus::Completed,
                        _ => PlanEntryStatus::Pending,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Builds the prompt for one `session/request_permission`. Options with a kind
/// this client does not know are dropped rather than guessed at.
pub(super) fn permission_item(params: &Value, request_id: u64) -> Option<PermissionItem> {
    let tool_call = params.get("toolCall")?;
    let options = params.get("options").and_then(Value::as_array)?;
    Some(PermissionItem {
        request_id,
        tool_call_id: str_field(tool_call, "toolCallId").map(str::to_owned),
        title: str_field(tool_call, "title")
            .unwrap_or("Permission requested")
            .to_owned(),
        options: options.iter().filter_map(permission_option).collect(),
        resolution: None,
    })
}

fn permission_option(option: &Value) -> Option<PermissionOption> {
    let id = str_field(option, "optionId")?;
    let kind = match str_field(option, "kind")? {
        "allow_once" => PermissionOptionKind::AllowOnce,
        "allow_always" => PermissionOptionKind::AllowAlways,
        "reject_once" => PermissionOptionKind::RejectOnce,
        "reject_always" => PermissionOptionKind::RejectAlways,
        _ => return None,
    };
    Some(PermissionOption {
        id: id.to_owned(),
        name: str_field(option, "name").unwrap_or(id).to_owned(),
        kind,
    })
}

/// The `outcome` object that answers a permission request. `None` when a
/// selected option is not one the agent offered.
pub(super) fn permission_outcome(
    options: &[PermissionOption],
    resolution: &PermissionResolution,
) -> Option<Value> {
    match resolution {
        PermissionResolution::Cancelled => Some(json!({"outcome": "cancelled"})),
        PermissionResolution::Selected(option_id) => options
            .iter()
            .any(|option| &option.id == option_id)
            .then(|| json!({"outcome": "selected", "optionId": option_id})),
    }
}

pub(super) fn parse_initialize(result: &Value) -> Negotiated {
    let advertised = |pointer: &str| {
        result
            .pointer(pointer)
            .is_some_and(|value| !value.is_null())
    };
    Negotiated {
        agent_name: result
            .pointer("/agentInfo/name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        capabilities: ChatCapabilities {
            load_session: result
                .pointer("/agentCapabilities/loadSession")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            list_sessions: advertised("/agentCapabilities/sessionCapabilities/list"),
            image_prompts: result
                .pointer("/agentCapabilities/promptCapabilities/image")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        support: SessionSupport {
            resume: advertised("/agentCapabilities/sessionCapabilities/resume"),
            close: advertised("/agentCapabilities/sessionCapabilities/close"),
        },
        protocol_version: result.get("protocolVersion").and_then(Value::as_i64),
    }
}

/// The agent wants a sign-in before it can open a session.
pub(super) fn requires_sign_in(error: &RpcError) -> bool {
    let message = error.message.to_ascii_lowercase();
    message.contains("auth_required") || (error.code == -32000 && message.contains("auth"))
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(update: Value) -> Vec<ChatEvent> {
        session_update_events(&update)
    }

    #[test]
    fn message_chunks_map_to_their_events() {
        assert_eq!(
            events(json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "Hi"},
            })),
            vec![ChatEvent::AgentMessageChunk {
                text: "Hi".to_owned()
            }]
        );
        assert_eq!(
            events(json!({
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": "hmm"},
            })),
            vec![ChatEvent::ThoughtChunk {
                text: "hmm".to_owned()
            }]
        );
        assert_eq!(
            events(json!({
                "sessionUpdate": "user_message_chunk",
                "content": {"type": "text", "text": "Fix it"},
            })),
            vec![ChatEvent::UserMessageChunk {
                text: "Fix it".to_owned()
            }]
        );
    }

    #[test]
    fn image_chunks_become_a_placeholder_and_other_blocks_are_ignored() {
        assert_eq!(
            events(json!({
                "sessionUpdate": "user_message_chunk",
                "content": {"type": "image", "data": "...", "mimeType": "image/png"},
            })),
            vec![ChatEvent::UserMessageChunk {
                text: "[image]".to_owned()
            }]
        );
        assert!(
            events(json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "resource_link", "uri": "file:///x"},
            }))
            .is_empty()
        );
    }

    #[test]
    fn tool_call_maps_content_kinds_and_locations() {
        let event = events(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-1",
            "title": "Edit main.rs",
            "kind": "edit",
            "status": "in_progress",
            "content": [
                {"type": "content", "content": {"type": "text", "text": "Working"}},
                {"type": "diff", "path": "/p/main.rs", "oldText": "a", "newText": "b"},
                {"type": "terminal", "terminalId": "term-9"},
                {"type": "content", "content": {"type": "image", "data": "x"}},
                {"type": "unknown"},
            ],
            "locations": [{"path": "/p/main.rs"}, {"line": 3}],
        }));
        assert_eq!(
            event,
            vec![ChatEvent::ToolCall(ToolCallItem {
                id: "tc-1".to_owned(),
                title: "Edit main.rs".to_owned(),
                kind: ToolKind::Edit,
                status: ToolStatus::InProgress,
                content: vec![
                    ToolContent::Text("Working".to_owned()),
                    ToolContent::Diff {
                        path: "/p/main.rs".to_owned(),
                        old_text: Some("a".to_owned()),
                        new_text: "b".to_owned(),
                    },
                    ToolContent::Terminal {
                        terminal_id: "term-9".to_owned(),
                    },
                    ToolContent::Text("[image]".to_owned()),
                ],
                locations: vec!["/p/main.rs".to_owned()],
            })]
        );
    }

    #[test]
    fn tool_call_without_optional_fields_uses_defaults() {
        let event = events(json!({"sessionUpdate": "tool_call", "toolCallId": "t"}));
        let ChatEvent::ToolCall(item) = &event[0] else {
            panic!("expected a tool call, got {event:?}");
        };
        assert_eq!(item.title, "Tool call");
        assert_eq!(item.kind, ToolKind::Other);
        assert_eq!(item.status, ToolStatus::Pending);
        assert!(item.content.is_empty() && item.locations.is_empty());
    }

    #[test]
    fn tool_call_update_keeps_absent_fields_as_none() {
        assert_eq!(
            events(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tc-2",
                "status": "completed",
            })),
            vec![ChatEvent::ToolCallUpdate(ToolCallUpdate {
                id: "tc-2".to_owned(),
                status: Some(ToolStatus::Completed),
                ..ToolCallUpdate::default()
            })]
        );
    }

    #[test]
    fn tool_call_update_with_empty_content_clears_it() {
        let event = events(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-2",
            "content": [],
        }));
        let ChatEvent::ToolCallUpdate(update) = &event[0] else {
            panic!("expected an update, got {event:?}");
        };
        assert_eq!(update.content, Some(Vec::new()));
    }

    #[test]
    fn plan_entries_map_statuses() {
        assert_eq!(
            events(json!({
                "sessionUpdate": "plan",
                "entries": [
                    {"content": "one", "priority": "high", "status": "completed"},
                    {"content": "two", "priority": "low", "status": "in_progress"},
                    {"content": "three", "priority": "low", "status": "pending"},
                ],
            })),
            vec![ChatEvent::Plan(vec![
                PlanEntry {
                    content: "one".to_owned(),
                    status: PlanEntryStatus::Completed,
                },
                PlanEntry {
                    content: "two".to_owned(),
                    status: PlanEntryStatus::InProgress,
                },
                PlanEntry {
                    content: "three".to_owned(),
                    status: PlanEntryStatus::Pending,
                },
            ])]
        );
    }

    #[test]
    fn unknown_update_kinds_are_ignored() {
        for kind in [
            "available_commands_update",
            "current_mode_update",
            "usage_update",
        ] {
            assert!(
                events(json!({"sessionUpdate": kind, "x": 1})).is_empty(),
                "{kind}"
            );
        }
    }

    #[test]
    fn permission_request_keeps_option_ids_and_drops_unknown_kinds() {
        let item = permission_item(
            &json!({
                "sessionId": "s",
                "toolCall": {"toolCallId": "tc-3", "title": "Run tests"},
                "options": [
                    {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                    {"optionId": "always", "name": "Always", "kind": "allow_always"},
                    {"optionId": "weird", "name": "?", "kind": "later"},
                    {"optionId": "no", "name": "Deny", "kind": "reject_once"},
                ],
            }),
            4,
        )
        .expect("valid permission request");
        assert_eq!(item.request_id, 4);
        assert_eq!(item.tool_call_id.as_deref(), Some("tc-3"));
        assert_eq!(item.title, "Run tests");
        assert_eq!(
            item.options,
            vec![
                PermissionOption {
                    id: "yes".to_owned(),
                    name: "Allow".to_owned(),
                    kind: PermissionOptionKind::AllowOnce,
                },
                PermissionOption {
                    id: "always".to_owned(),
                    name: "Always".to_owned(),
                    kind: PermissionOptionKind::AllowAlways,
                },
                PermissionOption {
                    id: "no".to_owned(),
                    name: "Deny".to_owned(),
                    kind: PermissionOptionKind::RejectOnce,
                },
            ]
        );
        assert_eq!(item.resolution, None);
    }

    #[test]
    fn permission_request_without_tool_call_is_rejected() {
        assert!(permission_item(&json!({"options": []}), 1).is_none());
    }

    #[test]
    fn permission_outcome_answers_only_offered_options() {
        let options = vec![PermissionOption {
            id: "yes".to_owned(),
            name: "Allow".to_owned(),
            kind: PermissionOptionKind::AllowOnce,
        }];
        assert_eq!(
            permission_outcome(&options, &PermissionResolution::Selected("yes".to_owned())),
            Some(json!({"outcome": "selected", "optionId": "yes"}))
        );
        assert_eq!(
            permission_outcome(&options, &PermissionResolution::Selected("nope".to_owned())),
            None
        );
        assert_eq!(
            permission_outcome(&options, &PermissionResolution::Cancelled),
            Some(json!({"outcome": "cancelled"}))
        );
    }

    #[test]
    fn initialize_result_reports_capabilities_and_name() {
        let negotiated = parse_initialize(&json!({
            "protocolVersion": 1,
            "agentInfo": {"name": "grok", "version": "1.0"},
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {"image": true},
                "sessionCapabilities": {"list": {}, "resume": {}, "close": null},
            },
        }));
        assert_eq!(negotiated.protocol_version, Some(1));
        assert_eq!(negotiated.agent_name.as_deref(), Some("grok"));
        assert_eq!(
            negotiated.capabilities,
            ChatCapabilities {
                load_session: true,
                list_sessions: true,
                image_prompts: true,
            }
        );
        assert_eq!(
            negotiated.support,
            SessionSupport {
                resume: true,
                close: false,
            }
        );
    }

    #[test]
    fn real_grok_initialize_result_is_understood() {
        // Trimmed from `grok agent stdio` (agent 1.0.52). Grok sends no agentInfo.
        let negotiated = parse_initialize(&json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
                "sessionCapabilities": {"list": {}, "resume": {}, "close": {}},
                "auth": {},
            },
            "authMethods": [{"id": "cached_token", "name": "cached_token"}],
        }));
        assert_eq!(negotiated.protocol_version, Some(1));
        assert_eq!(negotiated.agent_name, None);
        assert_eq!(
            negotiated.capabilities,
            ChatCapabilities {
                load_session: true,
                list_sessions: true,
                image_prompts: false,
            }
        );
        assert_eq!(
            negotiated.support,
            SessionSupport {
                resume: true,
                close: true,
            }
        );
    }

    #[test]
    fn empty_initialize_result_advertises_nothing() {
        let negotiated = parse_initialize(&json!({}));
        assert_eq!(negotiated.agent_name, None);
        assert_eq!(negotiated.capabilities, ChatCapabilities::default());
        assert_eq!(negotiated.support, SessionSupport::default());
    }

    #[test]
    fn sign_in_errors_are_recognized() {
        assert!(requires_sign_in(&RpcError::new(
            -32000,
            "Authentication required"
        )));
        assert!(requires_sign_in(&RpcError::new(-32000, "auth_required")));
        assert!(!requires_sign_in(&RpcError::new(
            -32000,
            "model overloaded"
        )));
        assert!(!requires_sign_in(&RpcError::new(-32603, "auth failed")));
    }
}
