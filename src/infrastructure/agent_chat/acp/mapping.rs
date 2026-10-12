//! Pure translation between ACP JSON and the normalized chat model. Nothing
//! here does I/O, so the protocol shapes are covered directly by tests.

use serde_json::{Value, json};

use crate::domain::agent_chat::{
    ChatCapabilities, ChatEvent, ConfigCategory, ConfigOption, ConfigValue, ContextUsage,
    PermissionItem, PermissionOption, PermissionOptionKind, PermissionResolution, PlanEntry,
    PlanEntryStatus, PromptFile, SessionMode, SessionModes, SlashCommand, ToolCallItem,
    ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};

use super::rpc::RpcError;

/// Id of the option that stands for the agent's model list when the agent
/// sends `models` but no config option in the model category.
pub(super) const MODEL_OPTION_ID: &str = "__model";

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
/// Mode and config updates also change `controls`. Unknown kinds produce nothing.
pub(super) fn session_update_events(update: &Value, controls: &mut Controls) -> Vec<ChatEvent> {
    match str_field(update, "sessionUpdate") {
        Some("user_message_chunk") => chunk(update, |text| ChatEvent::UserMessageChunk { text }),
        Some("agent_message_chunk") => chunk(update, |text| ChatEvent::AgentMessageChunk { text }),
        Some("agent_thought_chunk") => chunk(update, |text| ChatEvent::ThoughtChunk { text }),
        Some("tool_call") => vec![ChatEvent::ToolCall(tool_call_item(update))],
        Some("tool_call_update") => vec![ChatEvent::ToolCallUpdate(tool_call_update(update))],
        Some("plan") => vec![ChatEvent::Plan(plan_entries(update))],
        Some("current_mode_update") => str_field(update, "currentModeId")
            .map(|mode_id| vec![controls.set_current_mode(mode_id)])
            .unwrap_or_default(),
        Some("config_option_update") => parse_config_options(update)
            .map(|options| vec![controls.replace_options(options)])
            .unwrap_or_default(),
        Some("session_info_update") => title(update)
            .map(|title| vec![ChatEvent::Title(title)])
            .unwrap_or_default(),
        Some("usage_update") => usage(update)
            .map(|usage| vec![ChatEvent::Usage(usage)])
            .unwrap_or_default(),
        Some("available_commands_update") => update
            .get("availableCommands")
            .and_then(Value::as_array)
            .map(|items| vec![ChatEvent::Commands(commands(items))])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Session controls as the agent last described them. Filled from the session
/// setup response and kept current by updates and by the answers to set calls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Controls {
    modes: Option<SessionModes>,
    /// The agent's own config options, without the synthetic model option.
    options: Vec<ConfigOption>,
    /// The unstable `models` list, shown as a select when no option is a model.
    model: Option<ConfigOption>,
}

impl Controls {
    /// Reads `session/new`, `session/load` or `session/resume` results. Returns
    /// the events to send after `SessionStarted`.
    pub fn setup(&mut self, result: &Value) -> Vec<ChatEvent> {
        *self = Self {
            modes: parse_modes(result),
            options: parse_config_options(result).unwrap_or_default(),
            model: parse_models(result),
        };
        let mut events = Vec::new();
        if let Some(modes) = &self.modes {
            events.push(ChatEvent::Modes(modes.clone()));
        }
        let options = self.all_options();
        if !options.is_empty() {
            events.push(ChatEvent::ConfigOptions(options));
        }
        events
    }

    /// The options the view shows: the agent's own, then the synthetic model
    /// option when the agent has no model option of its own.
    pub fn all_options(&self) -> Vec<ConfigOption> {
        let mut options = self.options.clone();
        if let Some(model) = &self.model
            && !options
                .iter()
                .any(|option| option.category == ConfigCategory::Model)
        {
            options.push(model.clone());
        }
        options
    }

    /// `current_mode_update` or the answer to `session/set_mode`. The last known
    /// list of modes is kept.
    pub fn set_current_mode(&mut self, mode_id: &str) -> ChatEvent {
        let modes = self.modes.get_or_insert_with(SessionModes::default);
        mode_id.clone_into(&mut modes.current);
        ChatEvent::Modes(modes.clone())
    }

    /// `config_option_update` or the answer to `session/set_config_option` that
    /// carries the full option list.
    pub fn replace_options(&mut self, options: Vec<ConfigOption>) -> ChatEvent {
        self.options = options;
        ChatEvent::ConfigOptions(self.all_options())
    }

    /// The answer to `session/set_config_option` without options. The new value
    /// is applied locally.
    pub fn set_option_value(&mut self, option_id: &str, value: &str) -> ChatEvent {
        if let Some(option) = self
            .options
            .iter_mut()
            .find(|option| option.id == option_id)
        {
            value.clone_into(&mut option.current);
        }
        ChatEvent::ConfigOptions(self.all_options())
    }

    /// The answer to `session/set_model`, for the synthetic model option.
    pub fn set_model(&mut self, model_id: &str) -> ChatEvent {
        if let Some(model) = self.model.as_mut() {
            model_id.clone_into(&mut model.current);
        }
        ChatEvent::ConfigOptions(self.all_options())
    }

    /// The option's name for messages, or its id when it is unknown.
    pub fn option_label(&self, option_id: &str) -> String {
        self.all_options()
            .into_iter()
            .find(|option| option.id == option_id)
            .map_or_else(|| option_id.to_owned(), |option| option.name)
    }
}

/// `modes` of a session setup result. Absent or null gives `None`.
fn parse_modes(result: &Value) -> Option<SessionModes> {
    let modes = object(result, "modes")?;
    Some(SessionModes {
        current: str_field(modes, "currentModeId")
            .unwrap_or_default()
            .to_owned(),
        available: modes
            .get("availableModes")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(session_mode).collect())
            .unwrap_or_default(),
    })
}

fn session_mode(item: &Value) -> Option<SessionMode> {
    let id = str_field(item, "id")?;
    Some(SessionMode {
        id: id.to_owned(),
        name: str_field(item, "name").unwrap_or(id).to_owned(),
        description: str_field(item, "description").map(str::to_owned),
    })
}

/// `configOptions` of a result or of a `config_option_update`. Returns `None`
/// when the key is missing. Only select options are kept.
pub(super) fn parse_config_options(value: &Value) -> Option<Vec<ConfigOption>> {
    let items = value.get("configOptions")?.as_array()?;
    Some(items.iter().filter_map(config_option).collect())
}

fn config_option(item: &Value) -> Option<ConfigOption> {
    if str_field(item, "type")? != "select" {
        return None;
    }
    let id = str_field(item, "id")?;
    Some(ConfigOption {
        id: id.to_owned(),
        name: str_field(item, "name").unwrap_or(id).to_owned(),
        category: match str_field(item, "category") {
            Some("model") => ConfigCategory::Model,
            Some("thought_level") => ConfigCategory::ThoughtLevel,
            Some("mode") => ConfigCategory::Mode,
            _ => ConfigCategory::Other,
        },
        current: str_field(item, "currentValue")?.to_owned(),
        values: config_values(item.get("options")?),
    })
}

/// Flattens grouped options (`{group, name, options: [...]}`) into one list.
fn config_values(options: &Value) -> Vec<ConfigValue> {
    options
        .as_array()
        .map(|items| {
            items
                .iter()
                .flat_map(|item| -> Vec<ConfigValue> {
                    match item.get("options").and_then(Value::as_array) {
                        Some(group) => group.iter().filter_map(config_value).collect(),
                        None => config_value(item).into_iter().collect(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn config_value(item: &Value) -> Option<ConfigValue> {
    let value = str_field(item, "value")?;
    Some(ConfigValue {
        value: value.to_owned(),
        name: str_field(item, "name").unwrap_or(value).to_owned(),
        description: str_field(item, "description").map(str::to_owned),
    })
}

/// The unstable `models` of a session setup result, as the synthetic model option.
fn parse_models(result: &Value) -> Option<ConfigOption> {
    let models = object(result, "models")?;
    Some(ConfigOption {
        id: MODEL_OPTION_ID.to_owned(),
        name: "Model".to_owned(),
        category: ConfigCategory::Model,
        current: str_field(models, "currentModelId")
            .unwrap_or_default()
            .to_owned(),
        values: models
            .get("availableModels")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = str_field(item, "modelId")?;
                        Some(ConfigValue {
                            value: id.to_owned(),
                            name: str_field(item, "name").unwrap_or(id).to_owned(),
                            description: str_field(item, "description").map(str::to_owned),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// `session/update` with `title`. Null or blank titles are ignored.
fn title(update: &Value) -> Option<String> {
    str_field(update, "title")
        .filter(|title| !title.trim().is_empty())
        .map(str::to_owned)
}

/// `session/update` with `used` and `size`. Either one missing ignores the update.
fn usage(update: &Value) -> Option<ContextUsage> {
    Some(ContextUsage {
        used: update.get("used")?.as_u64()?,
        size: update.get("size")?.as_u64()?,
    })
}

fn commands(items: &[Value]) -> Vec<SlashCommand> {
    items
        .iter()
        .filter_map(|item| {
            Some(SlashCommand {
                name: str_field(item, "name")?.to_owned(),
                description: str_field(item, "description")
                    .unwrap_or_default()
                    .to_owned(),
                input_hint: item
                    .get("input")
                    .and_then(|input| str_field(input, "hint"))
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// The `prompt` array of `session/prompt`: the typed text, then one
/// `resource_link` per attached file. Resource links are baseline ACP, so they
/// need no capability check.
pub(super) fn prompt_blocks(text: &str, files: &[PromptFile]) -> Value {
    let mut blocks = vec![json!({"type": "text", "text": text})];
    blocks.extend(files.iter().map(|file| {
        json!({
            "type": "resource_link",
            "uri": file_uri(&file.path),
            "name": file.label,
        })
    }));
    Value::Array(blocks)
}

/// A `file://` URI for an absolute path. Everything but unreserved characters
/// and `/` is percent-encoded, byte by byte.
pub(super) fn file_uri(path: &str) -> String {
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(char::from(byte));
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
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

/// A nested object; anything else (including null) is `None`.
fn object<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.get(key).filter(|nested| nested.is_object())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(update: Value) -> Vec<ChatEvent> {
        session_update_events(&update, &mut Controls::default())
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
        for kind in ["some_future_update", "_x.ai/models/update"] {
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

    fn option<'a>(options: &'a [ConfigOption], id: &str) -> &'a ConfigOption {
        options
            .iter()
            .find(|option| option.id == id)
            .unwrap_or_else(|| panic!("no config option {id}"))
    }

    fn only_config_options(events: Vec<ChatEvent>) -> Vec<ConfigOption> {
        let [ChatEvent::ConfigOptions(options)] = events.as_slice() else {
            panic!("expected one ConfigOptions event, got {events:?}");
        };
        options.clone()
    }

    /// Trimmed from the real `session/new` of claude-agent-acp 0.89.1
    /// (Claude Code 2.1.296). Claude sends `modes` and `configOptions`, no `models`.
    #[test]
    fn real_claude_session_setup_maps_modes_and_options() {
        let mut controls = Controls::default();
        let events = controls.setup(&json!({
            "sessionId": "1ffa0ebe",
            "modes": {
                "currentModeId": "default",
                "availableModes": [
                    {"id": "default", "name": "Manual", "description": "Always ask before making changes"},
                    {"id": "acceptEdits", "name": "Accept edits", "description": "Automatically accept all file edits"},
                    {"id": "plan", "name": "Plan", "description": "Create a plan before making changes"},
                    {"id": "auto", "name": "Auto", "description": "Claude handles permission decisions"},
                    {"id": "bypassPermissions", "name": "Bypass permissions", "description": "Accepts all permissions"},
                ],
            },
            "configOptions": [
                {"id": "mode", "name": "Mode", "description": "Session permission mode",
                 "category": "mode", "type": "select", "currentValue": "default",
                 "options": [{"value": "default", "name": "Manual"}, {"value": "plan", "name": "Plan"}]},
                {"id": "model", "name": "Model", "description": "AI model to use",
                 "category": "model", "type": "select", "currentValue": "opus",
                 "options": [
                    {"value": "default", "name": "Default (recommended)", "description": "Opus 5.5"},
                    {"value": "opus", "name": "Opus 5.5", "description": "For complex work and everyday tasks"},
                    {"value": "sonnet", "name": "Sonnet 5.5"},
                 ]},
                {"id": "effort", "name": "Effort", "description": "Available effort levels for this model",
                 "category": "thought_level", "type": "select", "currentValue": "high",
                 "options": [{"value": "default", "name": "Default"}, {"value": "high", "name": "High"}]},
                {"id": "fast", "name": "Fast mode", "category": "model_config", "type": "select",
                 "currentValue": "off",
                 "options": [{"value": "on", "name": "On"}, {"value": "off", "name": "Off"}]},
                {"id": "agent", "name": "Agent", "type": "select", "currentValue": "default",
                 "options": [{"value": "default", "name": "Default", "description": "Standard Claude Code agent"}]},
            ],
        }));

        let [ChatEvent::Modes(modes), ChatEvent::ConfigOptions(options)] = events.as_slice() else {
            panic!("expected Modes then ConfigOptions, got {events:?}");
        };
        assert_eq!(modes.current, "default");
        assert_eq!(modes.available.len(), 5);
        assert_eq!(modes.available[2].id, "plan");
        assert_eq!(modes.available[2].name, "Plan");
        assert_eq!(
            modes.available[2].description.as_deref(),
            Some("Create a plan before making changes")
        );

        assert_eq!(
            options.len(),
            5,
            "no synthetic option when a model option exists"
        );
        assert_eq!(option(options, "mode").category, ConfigCategory::Mode);
        let model = option(options, "model");
        assert_eq!(model.category, ConfigCategory::Model);
        assert_eq!(model.current, "opus");
        assert_eq!(model.current_name(), "Opus 5.5");
        assert_eq!(model.values[0].description.as_deref(), Some("Opus 5.5"));
        assert_eq!(
            option(options, "effort").category,
            ConfigCategory::ThoughtLevel
        );
        assert_eq!(option(options, "fast").category, ConfigCategory::Other);
        assert_eq!(option(options, "agent").category, ConfigCategory::Other);
        assert_eq!(
            option(options, "agent").values[0].description.as_deref(),
            Some("Standard Claude Code agent")
        );
    }

    /// Trimmed from the real `session/new` of codex-acp 2.2.2 (codex-cli 0.162.1).
    /// Codex sends `modes`, a `mode` option, and a `models` list that also has a
    /// model option, so no synthetic option is added.
    #[test]
    fn real_codex_session_setup_keeps_its_model_option() {
        let mut controls = Controls::default();
        let events = controls.setup(&json!({
            "sessionId": "01a130bb",
            "models": {
                "availableModels": [
                    {"modelId": "gpt-6.1-sol[low]", "name": "6.1 Sol (low)", "description": "Fast responses"},
                    {"modelId": "gpt-6.1-sol[xhigh]", "name": "6.1 Sol (xhigh)", "description": "Extra high"},
                ],
                "currentModelId": "gpt-6.1-sol[xhigh]",
            },
            "modes": {
                "availableModes": [
                    {"id": "read-only", "name": "Read-only", "description": "Requires approval to edit files."},
                    {"id": "agent", "name": "Auto review", "description": "Only ask for unsafe actions"},
                ],
                "currentModeId": "agent",
            },
            "configOptions": [
                {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "agent",
                 "options": [{"value": "read-only", "name": "Read-only"}, {"value": "agent", "name": "Auto review"}]},
                {"id": "collaboration_mode", "name": "Collaboration mode", "category": "collaboration_mode",
                 "type": "select", "currentValue": "default",
                 "options": [{"value": "default", "name": "Default"}, {"value": "plan", "name": "Plan"}]},
                {"id": "model", "name": "Model", "category": "model", "type": "select",
                 "currentValue": "gpt-6.1-sol",
                 "options": [{"value": "gpt-6.1-sol", "name": "6.1 Sol", "description": "Latest workhorse model."}]},
                {"id": "reasoning_effort", "name": "Reasoning effort", "category": "thought_level", "type": "select",
                 "currentValue": "xhigh",
                 "options": [{"value": "low", "name": "Low"}, {"value": "xhigh", "name": "Xhigh"}]},
                {"id": "fast-mode", "name": "Fast mode", "category": "model_config", "type": "select",
                 "currentValue": "off",
                 "options": [{"value": "off", "name": "Off"}, {"value": "on", "name": "On"}]},
            ],
        }));

        let [ChatEvent::Modes(modes), ChatEvent::ConfigOptions(options)] = events.as_slice() else {
            panic!("expected Modes then ConfigOptions, got {events:?}");
        };
        assert_eq!(modes.current, "agent");
        assert_eq!(modes.available[0].name, "Read-only");
        assert_eq!(options.len(), 5);
        assert!(options.iter().all(|option| option.id != MODEL_OPTION_ID));
        assert_eq!(
            option(options, "collaboration_mode").category,
            ConfigCategory::Other
        );
        assert_eq!(option(options, "model").current_name(), "6.1 Sol");
        assert_eq!(
            option(options, "reasoning_effort").category,
            ConfigCategory::ThoughtLevel
        );
        assert_eq!(option(options, "reasoning_effort").current_name(), "Xhigh");
    }

    /// Trimmed from the real `session/new` of grok 1.0.52. Grok sends no `modes`;
    /// its model and reasoning options carry the whole picker.
    #[test]
    fn real_grok_session_setup_has_no_modes() {
        let mut controls = Controls::default();
        let events = controls.setup(&json!({
            "sessionId": "01a130bb-a147",
            "models": {
                "currentModelId": "grok-4.7",
                "availableModels": [
                    {"modelId": "grok-4.7", "name": "Grok 4.7", "description": "SpaceXAI's latest frontier model",
                     "_meta": {"reasoningEffort": "xhigh"}},
                    {"modelId": "grok-4.7-build-fast", "name": "Grok 4.7 Fast", "description": "Fast variant. 2x the price."},
                ],
            },
            "configOptions": [
                {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "grok-4.7",
                 "options": [{"value": "grok-4.7", "name": "Grok 4.7"}, {"value": "grok-4.7-build-fast", "name": "Grok 4.7 Fast"}]},
                {"id": "reasoning_effort", "name": "Reasoning Effort", "category": "thought_level", "type": "select",
                 "currentValue": "xhigh",
                 "options": [{"value": "xhigh", "name": "Extra High", "description": "Maximum reasoning for the hardest tasks."}]},
            ],
            "_meta": {},
        }));

        let options = only_config_options(events);
        assert_eq!(
            options.len(),
            2,
            "the model list is not added a second time"
        );
        assert_eq!(option(&options, "model").values.len(), 2);
        assert_eq!(
            option(&options, "reasoning_effort").current_name(),
            "Extra High"
        );
    }

    #[test]
    fn models_without_a_model_option_become_the_synthetic_option() {
        let mut controls = Controls::default();
        let events = controls.setup(&json!({
            "sessionId": "s",
            "models": {
                "currentModelId": "m1",
                "availableModels": [
                    {"modelId": "m1", "name": "Model One"},
                    {"modelId": "m2", "name": "Model Two", "description": "Larger"},
                ],
            },
            "configOptions": [
                {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
                 "currentValue": "high", "options": [{"value": "high", "name": "High"}]},
            ],
        }));
        let options = only_config_options(events);
        assert_eq!(options.len(), 2);
        let model = option(&options, MODEL_OPTION_ID);
        assert_eq!(model.name, "Model");
        assert_eq!(model.category, ConfigCategory::Model);
        assert_eq!(model.current_name(), "Model One");
        assert_eq!(
            model.values[1],
            ConfigValue {
                value: "m2".to_owned(),
                name: "Model Two".to_owned(),
                description: Some("Larger".to_owned()),
            }
        );

        // A config_option_update that omits the model keeps the synthetic option.
        let update = json!({
            "sessionUpdate": "config_option_update",
            "configOptions": [
                {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
                 "currentValue": "low", "options": [{"value": "low", "name": "Low"}]},
            ],
        });
        let options = only_config_options(session_update_events(&update, &mut controls));
        assert_eq!(options.len(), 2);
        assert_eq!(option(&options, "effort").current, "low");
        assert_eq!(option(&options, MODEL_OPTION_ID).current, "m1");

        // The answer to session/set_model moves the synthetic option's current value.
        let ChatEvent::ConfigOptions(options) = controls.set_model("m2") else {
            panic!("expected ConfigOptions");
        };
        assert_eq!(
            option(&options, MODEL_OPTION_ID).current_name(),
            "Model Two"
        );
    }

    #[test]
    fn grouped_options_are_flattened_and_boolean_options_skipped() {
        let mut controls = Controls::default();
        let events = controls.setup(&json!({
            "sessionId": "s",
            "configOptions": [
                {"id": "sandbox", "name": "Sandbox", "category": "mode", "type": "select",
                 "currentValue": "b",
                 "options": [
                    {"group": "g1", "name": "Group 1", "options": [
                        {"value": "a", "name": "A"}, {"value": "b", "name": "B"},
                    ]},
                    {"value": "c", "name": "C"},
                 ]},
                {"id": "auto", "name": "Auto", "type": "boolean", "currentValue": true},
            ],
        }));
        let options = only_config_options(events);
        assert_eq!(options.len(), 1);
        let sandbox = &options[0];
        assert_eq!(sandbox.category, ConfigCategory::Mode);
        assert_eq!(
            sandbox
                .values
                .iter()
                .map(|value| value.value.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(sandbox.current_name(), "B");
    }

    #[test]
    fn current_mode_update_keeps_the_available_modes() {
        let mut controls = Controls::default();
        controls.setup(&json!({
            "modes": {
                "currentModeId": "default",
                "availableModes": [
                    {"id": "default", "name": "Manual"},
                    {"id": "plan", "name": "Plan"},
                ],
            },
        }));
        let events = session_update_events(
            &json!({"sessionUpdate": "current_mode_update", "currentModeId": "plan"}),
            &mut controls,
        );
        let [ChatEvent::Modes(modes)] = events.as_slice() else {
            panic!("expected Modes, got {events:?}");
        };
        assert_eq!(modes.current, "plan");
        assert_eq!(modes.available.len(), 2);
        assert_eq!(modes.available[1].name, "Plan");
    }

    #[test]
    fn answers_to_set_calls_update_the_controls() {
        let mut controls = Controls::default();
        controls.setup(&json!({
            "modes": {"currentModeId": "default", "availableModes": [{"id": "default", "name": "Manual"}]},
            "configOptions": [
                {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
                 "currentValue": "high", "options": [{"value": "high", "name": "High"}, {"value": "low", "name": "Low"}]},
            ],
        }));

        // A set_config_option answer without options: the value is applied locally.
        let options = only_config_options(vec![controls.set_option_value("effort", "low")]);
        assert_eq!(option(&options, "effort").current_name(), "Low");
        assert_eq!(controls.option_label("effort"), "Effort");
        assert_eq!(controls.option_label("unknown"), "unknown");

        // A set_mode answer keeps the list and changes the current mode.
        assert_eq!(
            controls.set_current_mode("plan"),
            ChatEvent::Modes(SessionModes {
                current: "plan".to_owned(),
                available: vec![SessionMode {
                    id: "default".to_owned(),
                    name: "Manual".to_owned(),
                    description: None,
                }],
            })
        );
    }

    #[test]
    fn session_info_update_maps_titles_and_ignores_null() {
        assert_eq!(
            events(
                json!({"sessionUpdate": "session_info_update", "title": "Fix the build", "updatedAt": "2026-10-12T03:23:20Z"})
            ),
            vec![ChatEvent::Title("Fix the build".to_owned())]
        );
        // Codex sends status-only updates with no title; a null title is also ignored.
        assert!(
            events(json!({"sessionUpdate": "session_info_update", "_meta": {"codex": {}}}))
                .is_empty()
        );
        assert!(events(json!({"sessionUpdate": "session_info_update", "title": null})).is_empty());
        assert!(events(json!({"sessionUpdate": "session_info_update", "title": "  "})).is_empty());
    }

    /// Real `usage_update` from the Claude and Codex spike logs. Extra fields are ignored.
    #[test]
    fn usage_update_maps_used_and_size() {
        assert_eq!(
            events(json!({
                "sessionUpdate": "usage_update",
                "used": 19821,
                "size": 475000,
                "_meta": {"_claude/model": "claude-opus-5-5"},
            })),
            vec![ChatEvent::Usage(ContextUsage {
                used: 19821,
                size: 475000,
            })]
        );
        assert!(events(json!({"sessionUpdate": "usage_update", "used": 1})).is_empty());
    }

    /// Trimmed from the real `available_commands_update` of codex-acp 2.2.2.
    #[test]
    fn available_commands_map_input_hints() {
        assert_eq!(
            events(json!({
                "sessionUpdate": "available_commands_update",
                "availableCommands": [
                    {"name": "plan", "description": "Turn plan mode on.", "input": null},
                    {"name": "mcp", "description": "Show the status of the MCP servers, or reconnect them.",
                     "input": {"hint": "[reconnect]"}},
                ],
            })),
            vec![ChatEvent::Commands(vec![
                SlashCommand {
                    name: "plan".to_owned(),
                    description: "Turn plan mode on.".to_owned(),
                    input_hint: None,
                },
                SlashCommand {
                    name: "mcp".to_owned(),
                    description: "Show the status of the MCP servers, or reconnect them."
                        .to_owned(),
                    input_hint: Some("[reconnect]".to_owned()),
                },
            ])]
        );
        // An empty list replaces the old one; a missing list changes nothing.
        assert_eq!(
            events(json!({"sessionUpdate": "available_commands_update", "availableCommands": []})),
            vec![ChatEvent::Commands(Vec::new())]
        );
        assert!(events(json!({"sessionUpdate": "available_commands_update"})).is_empty());
    }

    #[test]
    fn prompt_blocks_add_percent_encoded_resource_links() {
        let files = [
            PromptFile {
                path: "/Users/me/My Project/src/main.rs".to_owned(),
                label: "src/main.rs".to_owned(),
            },
            PromptFile {
                path: "/tmp/café.md".to_owned(),
                label: "café.md".to_owned(),
            },
        ];
        assert_eq!(
            prompt_blocks("look", &files),
            json!([
                {"type": "text", "text": "look"},
                {"type": "resource_link", "uri": "file:///Users/me/My%20Project/src/main.rs", "name": "src/main.rs"},
                {"type": "resource_link", "uri": "file:///tmp/caf%C3%A9.md", "name": "café.md"},
            ])
        );
        assert_eq!(
            prompt_blocks("plain", &[]),
            json!([{"type": "text", "text": "plain"}])
        );
        assert_eq!(file_uri("/a%b#c?d+e"), "file:///a%25b%23c%3Fd%2Be");
    }
}
