use serde_json::{Value, json};

use crate::domain::workspace::HostedAgentKind;
use crate::ports::harness::{
    HarnessError, HarnessEvent, HarnessPermissionOption, HarnessPermissionRequest,
};

/// Pure vendor codec + unanswered-permission gate. Ingest never writes a reply.
#[derive(Debug, Clone)]
pub struct HostedConversation {
    agent: HostedAgentKind,
    vendor_session_id: Option<String>,
    pending_permission: Option<HarnessPermissionRequest>,
    advertised_commands: Vec<String>,
    advertised_models: Vec<String>,
    next_rpc_id: u64,
    grok_initialized: bool,
    grok_session_ready: bool,
    grok_steering: bool,
    codex_initialized: bool,
    codex_thread_id: Option<String>,
    preferred_model: Option<String>,
    preferred_effort: Option<String>,
}

impl HostedConversation {
    pub fn new(agent: HostedAgentKind, vendor_session_id: Option<String>) -> Self {
        Self {
            agent,
            vendor_session_id,
            pending_permission: None,
            advertised_commands: Vec::new(),
            advertised_models: Vec::new(),
            next_rpc_id: 1,
            grok_initialized: false,
            grok_session_ready: false,
            grok_steering: false,
            codex_initialized: false,
            codex_thread_id: None,
            preferred_model: None,
            preferred_effort: None,
        }
    }

    pub fn apply_launch_args(&mut self, args: &[String]) {
        let mut index = 0;
        while index < args.len() {
            let arg = args[index].as_str();
            if matches!(arg, "-m" | "--model") {
                if let Some(value) = args.get(index + 1) {
                    self.preferred_model = Some(value.clone());
                    index += 2;
                    continue;
                }
            }
            if matches!(arg, "-c" | "--effort" | "--reasoning") {
                if let Some(value) = args.get(index + 1) {
                    let effort = value
                        .strip_prefix("model_reasoning_effort=")
                        .unwrap_or(value);
                    self.preferred_effort = Some(effort.to_owned());
                    index += 2;
                    continue;
                }
            }
            if matches!(
                arg,
                "minimal" | "low" | "medium" | "high" | "xhigh" | "x-high"
            ) {
                self.preferred_effort = Some(arg.replace("x-high", "xhigh"));
            }
            index += 1;
        }
    }

    #[allow(dead_code)]
    pub fn agent(&self) -> HostedAgentKind {
        self.agent
    }

    pub fn vendor_session_id(&self) -> Option<&str> {
        self.vendor_session_id.as_deref()
    }

    pub fn pending_permission(&self) -> Option<&HarnessPermissionRequest> {
        self.pending_permission.as_ref()
    }

    pub fn advertised_commands(&self) -> &[String] {
        &self.advertised_commands
    }

    pub fn advertised_models(&self) -> &[String] {
        &self.advertised_models
    }

    #[allow(dead_code)]
    pub fn grok_supports_mid_turn_steer(&self) -> bool {
        self.grok_steering
    }

    pub fn ingest_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        match self.agent {
            HostedAgentKind::Claude => self.ingest_claude(line),
            HostedAgentKind::Codex => self.ingest_codex(line),
            HostedAgentKind::Grok => self.ingest_grok(line),
        }
    }

    /// Handshake bytes written right after spawn. Never includes policy flags.
    pub fn encode_handshake(&mut self) -> Result<Vec<u8>, HarnessError> {
        match self.agent {
            HostedAgentKind::Claude => Ok(Vec::new()),
            HostedAgentKind::Codex => {
                let id = self.next_id();
                Ok(line(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "initialize",
                    "params": {
                        "clientInfo": { "name": "vibra", "version": "0.3.6" }
                    }
                })))
            }
            HostedAgentKind::Grok => {
                let id = self.next_id();
                Ok(line(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": 1,
                        "clientCapabilities": {},
                        "clientInfo": { "name": "vibra", "version": "0.3.6" }
                    }
                })))
            }
        }
    }

    pub fn encode_user_turn(&mut self, text: &str) -> Result<Vec<u8>, HarnessError> {
        if self.pending_permission.is_some() {
            return Err(HarnessError::Protocol(
                "hay un permiso pendiente; responde antes de enviar".into(),
            ));
        }
        match self.agent {
            HostedAgentKind::Claude => Ok(line(json!({
                "type": "user",
                "message": { "role": "user", "content": text },
                "parent_tool_use_id": Value::Null
            }))),
            HostedAgentKind::Codex => self.encode_codex_turn(text),
            HostedAgentKind::Grok => self.encode_grok_turn(text),
        }
    }

    pub fn encode_permission_reply(&mut self, option_id: &str) -> Result<Vec<u8>, HarnessError> {
        let request = self.pending_permission.clone().ok_or_else(|| {
            HarnessError::Protocol("no hay un permiso pendiente".into())
        })?;
        if !request.options.iter().any(|option| option.id == option_id) {
            return Err(HarnessError::Protocol(format!(
                "la opción {option_id} no la ofreció el agente"
            )));
        }
        let encoded = match self.agent {
            HostedAgentKind::Claude => line(json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request.id,
                    "response": { "behavior": option_id }
                }
            })),
            HostedAgentKind::Codex => line(json!({
                "jsonrpc": "2.0",
                "id": request.id.parse::<u64>().unwrap_or(0),
                "result": { "decision": option_id }
            })),
            HostedAgentKind::Grok => line(json!({
                "jsonrpc": "2.0",
                "id": request.id.parse::<u64>().unwrap_or(0),
                "result": {
                    "outcome": {
                        "outcome": "selected",
                        "optionId": option_id
                    }
                }
            })),
        };
        self.pending_permission = None;
        Ok(encoded)
    }

    pub fn encode_interrupt(&mut self) -> Result<Vec<u8>, HarnessError> {
        match self.agent {
            HostedAgentKind::Claude => {
                let id = format!("int-{}", self.next_id());
                Ok(line(json!({
                    "type": "control_request",
                    "request_id": id,
                    "request": { "subtype": "interrupt" }
                })))
            }
            HostedAgentKind::Codex => {
                let id = self.next_id();
                let Some(thread) = &self.codex_thread_id else {
                    return Ok(Vec::new());
                };
                Ok(line(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "turn/interrupt",
                    "params": { "threadId": thread }
                })))
            }
            HostedAgentKind::Grok => {
                let id = self.next_id();
                let Some(session) = &self.vendor_session_id else {
                    return Ok(Vec::new());
                };
                Ok(line(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "session/cancel",
                    "params": { "sessionId": session }
                })))
            }
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_rpc_id;
        self.next_rpc_id += 1;
        id
    }

    fn remember_session(&mut self, id: Option<&str>) -> Option<String> {
        let id = id.filter(|value| !value.is_empty())?;
        if self.vendor_session_id.as_deref() != Some(id) {
            self.vendor_session_id = Some(id.to_owned());
        }
        Some(id.to_owned())
    }

    fn ingest_claude(&mut self, line: &str) -> Vec<HarnessEvent> {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "system" => {
                let session = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .or_else(|| value.pointer("/session_id").and_then(Value::as_str));
                let mut events = Vec::new();
                if let Some(id) = self.remember_session(session) {
                    events.push(HarnessEvent::Done {
                        vendor_session_id: Some(id),
                    });
                    // Done from init is wrong - we should not emit Done on init.
                    // Use a dedicated path: store id only. Filter this below.
                    events.pop();
                }
                if let Some(model) = value.get("model").and_then(Value::as_str) {
                    push_unique(&mut self.advertised_models, model);
                }
                events
            }
            "assistant" => claude_assistant_events(&value),
            "stream_event" => claude_stream_events(&value),
            "control_request" => self.claude_permission(&value).into_iter().collect(),
            "result" => {
                let id = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .and_then(|id| self.remember_session(Some(id)));
                vec![HarnessEvent::Done {
                    vendor_session_id: id.or_else(|| self.vendor_session_id.clone()),
                }]
            }
            _ => Vec::new(),
        }
    }

    fn claude_permission(&mut self, value: &Value) -> Option<HarnessEvent> {
        let request = value.get("request")?;
        if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
            return None;
        }
        let id = value
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or("can_use_tool")
            .to_owned();
        let tool = request
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("tool");
        let permission = HarnessPermissionRequest {
            id,
            summary: format!("Allow {tool}?"),
            options: vec![
                HarnessPermissionOption {
                    id: "allow".into(),
                    label: "Allow".into(),
                },
                HarnessPermissionOption {
                    id: "deny".into(),
                    label: "Deny".into(),
                },
            ],
        };
        self.pending_permission = Some(permission.clone());
        Some(HarnessEvent::PermissionRequest(permission))
    }

    fn ingest_codex(&mut self, line: &str) -> Vec<HarnessEvent> {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        if value.get("result").is_some() && value.get("method").is_none() {
            if let Some(thread) = value.pointer("/result/thread/id").and_then(Value::as_str) {
                self.codex_thread_id = Some(thread.to_owned());
                self.remember_session(Some(thread));
            }
            if value.pointer("/result/protocolVersion").is_some()
                || value.pointer("/result/serverInfo").is_some()
            {
                self.codex_initialized = true;
            }
            if let Some(models) = value.pointer("/result/data").and_then(Value::as_array) {
                for model in models {
                    if let Some(id) = model.get("id").and_then(Value::as_str) {
                        push_unique(&mut self.advertised_models, id);
                    }
                }
            }
            return Vec::new();
        }
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        if method.ends_with("requestApproval") {
            return self.codex_permission(&value).into_iter().collect();
        }
        match method {
            "item/agentMessage/delta" | "item/agentMessage/textDelta" => {
                if let Some(text) = value
                    .pointer("/params/delta")
                    .and_then(Value::as_str)
                    .or_else(|| value.pointer("/params/text").and_then(Value::as_str))
                {
                    return vec![HarnessEvent::Text(text.to_owned())];
                }
            }
            "item/started" | "item/completed" => {
                if let Some(item) = value.get("params").and_then(|params| params.get("item")) {
                    return codex_item_events(item);
                }
            }
            "turn/completed" | "turn/failed" | "turn/aborted" => {
                return vec![HarnessEvent::Done {
                    vendor_session_id: self.vendor_session_id.clone(),
                }];
            }
            _ => {}
        }
        Vec::new()
    }

    fn codex_permission(&mut self, value: &Value) -> Option<HarnessEvent> {
        let id = value
            .get("id")
            .map(|id| match id {
                Value::Number(number) => number.to_string(),
                Value::String(text) => text.clone(),
                _ => "approval".into(),
            })
            .unwrap_or_else(|| "approval".into());
        let summary = value
            .pointer("/params/command")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/params/item/command").and_then(Value::as_str))
            .unwrap_or("Approve command?");
        let permission = HarnessPermissionRequest {
            id,
            summary: summary.to_owned(),
            options: vec![
                HarnessPermissionOption {
                    id: "accept".into(),
                    label: "Accept".into(),
                },
                HarnessPermissionOption {
                    id: "acceptForSession".into(),
                    label: "Accept for session".into(),
                },
                HarnessPermissionOption {
                    id: "decline".into(),
                    label: "Decline".into(),
                },
                HarnessPermissionOption {
                    id: "cancel".into(),
                    label: "Cancel".into(),
                },
            ],
        };
        self.pending_permission = Some(permission.clone());
        Some(HarnessEvent::PermissionRequest(permission))
    }

    fn encode_codex_turn(&mut self, text: &str) -> Result<Vec<u8>, HarnessError> {
        let mut out = Vec::new();
        if self.codex_thread_id.is_none() {
            let id = self.next_id();
            if let Some(resume) = self.vendor_session_id.clone() {
                out.extend(line(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "thread/resume",
                    "params": { "threadId": resume }
                })));
                self.codex_thread_id = Some(resume);
            } else {
                let mut params = serde_json::Map::new();
                if let Some(model) = &self.preferred_model {
                    params.insert("model".into(), json!(model));
                }
                if let Some(effort) = &self.preferred_effort {
                    params.insert("effort".into(), json!(effort));
                }
                out.extend(line(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "thread/start",
                    "params": Value::Object(params)
                })));
            }
        }
        let id = self.next_id();
        let thread = self
            .codex_thread_id
            .clone()
            .unwrap_or_else(|| "pending".into());
        let mut params = serde_json::Map::new();
        params.insert("threadId".into(), json!(thread));
        params.insert("input".into(), json!([{ "type": "text", "text": text }]));
        if let Some(model) = &self.preferred_model {
            params.insert("model".into(), json!(model));
        }
        if let Some(effort) = &self.preferred_effort {
            params.insert("effort".into(), json!(effort));
        }
        out.extend(line(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "turn/start",
            "params": Value::Object(params)
        })));
        Ok(out)
    }

    fn ingest_grok(&mut self, line: &str) -> Vec<HarnessEvent> {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        if let Some(result) = value.get("result") {
            if result.get("protocolVersion").is_some() {
                self.grok_initialized = true;
                if result
                    .pointer("/_meta/steering/supported")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    self.grok_steering = true;
                }
                if let Some(models) = result.pointer("/agentCapabilities/promptCapabilities") {
                    let _ = models;
                }
            }
            if let Some(session) = result.get("sessionId").and_then(Value::as_str) {
                self.remember_session(Some(session));
                self.grok_session_ready = true;
            }
            if let Some(options) = result
                .get("configOptions")
                .and_then(Value::as_array)
            {
                for option in options {
                    if option.get("category").and_then(Value::as_str) == Some("model")
                        && let Some(choices) = option.get("choices").and_then(Value::as_array)
                    {
                        for choice in choices {
                            if let Some(id) = choice.get("value").and_then(Value::as_str) {
                                push_unique(&mut self.advertised_models, id);
                            }
                        }
                    }
                }
            }
            return Vec::new();
        }
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        match method {
            "session/request_permission" => self.grok_permission(&value).into_iter().collect(),
            "session/update" => grok_update_events(
                value.get("params").unwrap_or(&Value::Null),
                &mut self.advertised_commands,
            ),
            _ => Vec::new(),
        }
    }

    fn grok_permission(&mut self, value: &Value) -> Option<HarnessEvent> {
        let id = value
            .get("id")
            .map(|id| match id {
                Value::Number(number) => number.to_string(),
                Value::String(text) => text.clone(),
                _ => "permission".into(),
            })
            .unwrap_or_else(|| "permission".into());
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        let summary = params
            .pointer("/toolCall/title")
            .and_then(Value::as_str)
            .or_else(|| params.pointer("/toolCall/kind").and_then(Value::as_str))
            .unwrap_or("Permission required");
        let mut options = Vec::new();
        if let Some(entries) = params.get("options").and_then(Value::as_array) {
            for entry in entries {
                let option_id = entry
                    .get("optionId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if option_id.is_empty() {
                    continue;
                }
                options.push(HarnessPermissionOption {
                    id: option_id.to_owned(),
                    label: entry
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(option_id)
                        .to_owned(),
                });
            }
        }
        if options.is_empty() {
            return None;
        }
        let permission = HarnessPermissionRequest {
            id,
            summary: summary.to_owned(),
            options,
        };
        self.pending_permission = Some(permission.clone());
        Some(HarnessEvent::PermissionRequest(permission))
    }

    fn encode_grok_turn(&mut self, text: &str) -> Result<Vec<u8>, HarnessError> {
        let mut out = Vec::new();
        if !self.grok_session_ready {
            let id = self.next_id();
            if let Some(resume) = self.vendor_session_id.clone() {
                out.extend(line(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "session/load",
                    "params": { "sessionId": resume }
                })));
            } else {
                out.extend(line(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "session/new",
                    "params": { "cwd": "." }
                })));
            }
            self.grok_session_ready = true;
        }
        let id = self.next_id();
        let session = self
            .vendor_session_id
            .clone()
            .unwrap_or_else(|| "pending".into());
        out.extend(line(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session,
                "prompt": [{ "type": "text", "text": text }]
            }
        })));
        Ok(out)
    }
}

fn line(value: Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

fn push_unique(items: &mut Vec<String>, value: &str) {
    if !items.iter().any(|item| item == value) {
        items.push(value.to_owned());
    }
}

fn claude_assistant_events(value: &Value) -> Vec<HarnessEvent> {
    let mut events = Vec::new();
    let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) else {
        return events;
    };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    events.push(HarnessEvent::Text(text.to_owned()));
                }
            }
            Some("thinking") => {
                if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                    events.push(HarnessEvent::Reasoning(text.to_owned()));
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_owned();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                events.push(HarnessEvent::ToolCall {
                    id,
                    summary: name.to_owned(),
                });
            }
            _ => {}
        }
    }
    events
}

fn claude_stream_events(value: &Value) -> Vec<HarnessEvent> {
    let delta = value.pointer("/event/delta").unwrap_or(&Value::Null);
    if let Some(text) = delta.get("text").and_then(Value::as_str) {
        return vec![HarnessEvent::Text(text.to_owned())];
    }
    if delta.get("type").and_then(Value::as_str) == Some("thinking_delta")
        && let Some(text) = delta.get("thinking").and_then(Value::as_str)
    {
        return vec![HarnessEvent::Reasoning(text.to_owned())];
    }
    Vec::new()
}

fn codex_item_events(item: &Value) -> Vec<HarnessEvent> {
    match item.get("type").and_then(Value::as_str) {
        Some("agent_message") => item
            .get("text")
            .and_then(Value::as_str)
            .map(|text| vec![HarnessEvent::Text(text.to_owned())])
            .unwrap_or_default(),
        Some("command_execution") => {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("exec");
            vec![HarnessEvent::ToolCall {
                id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(command)
                    .to_owned(),
                summary: command.to_owned(),
            }]
        }
        _ => Vec::new(),
    }
}

fn grok_update_events(params: &Value, commands: &mut Vec<String>) -> Vec<HarnessEvent> {
    let update = params
        .get("update")
        .cloned()
        .unwrap_or_else(|| params.clone());
    let kind = update
        .get("sessionUpdate")
        .or_else(|| update.get("session_update"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match kind {
        "agent_message_chunk" | "agent_thought_chunk" => {
            let text = update
                .pointer("/content/text")
                .and_then(Value::as_str)
                .or_else(|| update.get("text").and_then(Value::as_str));
            match (kind, text) {
                ("agent_thought_chunk", Some(text)) => vec![HarnessEvent::Reasoning(text.to_owned())],
                (_, Some(text)) => vec![HarnessEvent::Text(text.to_owned())],
                _ => Vec::new(),
            }
        }
        "tool_call" | "tool_call_update" => {
            let id = update
                .get("toolCallId")
                .or_else(|| update.get("tool_call_id"))
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_owned();
            if kind == "tool_call_update" {
                let output = update
                    .pointer("/content/text")
                    .and_then(Value::as_str)
                    .or_else(|| update.get("output").and_then(Value::as_str))
                    .or_else(|| update.get("rawOutput").and_then(Value::as_str))
                    .map(ToOwned::to_owned);
                vec![HarnessEvent::ToolResult { id, output }]
            } else {
                let title = update
                    .get("title")
                    .and_then(Value::as_str)
                    .or_else(|| update.get("kind").and_then(Value::as_str))
                    .unwrap_or("tool");
                vec![HarnessEvent::ToolCall {
                    id,
                    summary: title.to_owned(),
                }]
            }
        }
        "available_commands_update" => {
            if let Some(list) = update.get("availableCommands").and_then(Value::as_array) {
                for command in list {
                    if let Some(name) = command.get("name").and_then(Value::as_str) {
                        push_unique(commands, name);
                    }
                }
            }
            vec![HarnessEvent::Commands(commands.clone())]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload_strings(bytes: &[u8]) -> Vec<Value> {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn claude_user_turn_is_the_composer_text() {
        let mut conv = HostedConversation::new(HostedAgentKind::Claude, None);
        let bytes = conv.encode_user_turn("fix the bug").unwrap();
        let json = &payload_strings(&bytes)[0];
        assert_eq!(json["message"]["content"], "fix the bug");
        assert_eq!(json["type"], "user");
    }

    #[test]
    fn claude_permission_is_unanswered_until_host_reply() {
        let mut conv = HostedConversation::new(HostedAgentKind::Claude, None);
        let events = conv.ingest_line(
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash"}}"#,
        );
        assert!(matches!(
            events.first(),
            Some(HarnessEvent::PermissionRequest(request)) if request.id == "req-1"
        ));
        assert!(conv.pending_permission().is_some());
        assert!(conv.encode_user_turn("hi").is_err());
        let denied = conv.encode_permission_reply("yolo");
        assert!(denied.is_err());
        let reply = conv.encode_permission_reply("allow").unwrap();
        let json = &payload_strings(&reply)[0];
        assert_eq!(json["type"], "control_response");
        assert_eq!(json["response"]["request_id"], "req-1");
        assert_eq!(json["response"]["response"]["behavior"], "allow");
        assert!(conv.pending_permission().is_none());
    }

    #[test]
    fn claude_init_and_result_keep_vendor_session_id() {
        let mut conv = HostedConversation::new(HostedAgentKind::Claude, None);
        conv.ingest_line(r#"{"type":"system","subtype":"init","session_id":"sess-9","model":"opus"}"#);
        assert_eq!(conv.vendor_session_id(), Some("sess-9"));
        assert_eq!(conv.advertised_models(), &["opus"]);
        let events = conv.ingest_line(r#"{"type":"result","subtype":"success","session_id":"sess-9"}"#);
        assert_eq!(
            events,
            vec![HarnessEvent::Done {
                vendor_session_id: Some("sess-9".into())
            }]
        );
    }

    #[test]
    fn claude_resume_argv_reuses_stored_id() {
        let argv = crate::infrastructure::harness::official_spawn_argv(
            HostedAgentKind::Claude,
            Some("sess-9"),
        )
        .unwrap();
        assert!(argv.iter().any(|argument| argument == "--resume=sess-9"));
    }

    #[test]
    fn apply_launch_args_sets_codex_model_and_effort() {
        let mut conv = HostedConversation::new(HostedAgentKind::Codex, None);
        conv.apply_launch_args(&[
            "-m".into(),
            "gpt-5.6-sol".into(),
            "--effort".into(),
            "xhigh".into(),
        ]);
        let bytes = conv.encode_user_turn("hi").unwrap();
        let payloads = payload_strings(&bytes);
        let start = payloads
            .iter()
            .find(|value| value["method"] == "thread/start")
            .unwrap();
        assert_eq!(start["params"]["model"], "gpt-5.6-sol");
        assert_eq!(start["params"]["effort"], "xhigh");
        let turn = payloads
            .iter()
            .find(|value| value["method"] == "turn/start")
            .unwrap();
        assert_eq!(turn["params"]["model"], "gpt-5.6-sol");
        assert_eq!(turn["params"]["effort"], "xhigh");
    }

    #[test]
    fn codex_user_turn_carries_composer_text() {
        let mut conv = HostedConversation::new(HostedAgentKind::Codex, None);
        let bytes = conv.encode_user_turn("review this").unwrap();
        let payloads = payload_strings(&bytes);
        let turn = payloads
            .iter()
            .find(|value| value["method"] == "turn/start")
            .unwrap();
        assert_eq!(turn["params"]["input"][0]["text"], "review this");
    }

    #[test]
    fn codex_resume_uses_stored_thread_id() {
        let mut conv = HostedConversation::new(HostedAgentKind::Codex, Some("thr-4".into()));
        let bytes = conv.encode_user_turn("go").unwrap();
        let payloads = payload_strings(&bytes);
        let resume = payloads
            .iter()
            .find(|value| value["method"] == "thread/resume")
            .unwrap();
        assert_eq!(resume["params"]["threadId"], "thr-4");
    }

    #[test]
    fn codex_approval_waits_for_host() {
        let mut conv = HostedConversation::new(HostedAgentKind::Codex, None);
        let events = conv.ingest_line(
            r#"{"jsonrpc":"2.0","id":7,"method":"item/commandExecution/requestApproval","params":{"command":"rm -rf /"}}"#,
        );
        assert!(matches!(events.first(), Some(HarnessEvent::PermissionRequest(_))));
        assert!(conv.encode_permission_reply("yolo").is_err());
        let reply = conv.encode_permission_reply("decline").unwrap();
        let json = &payload_strings(&reply)[0];
        assert_eq!(json["id"], 7);
        assert_eq!(json["result"]["decision"], "decline");
        assert!(conv.pending_permission().is_none());
    }

    #[test]
    fn grok_user_turn_is_the_composer_text() {
        let mut conv = HostedConversation::new(HostedAgentKind::Grok, None);
        let bytes = conv.encode_user_turn("explain main.rs").unwrap();
        let payloads = payload_strings(&bytes);
        let prompt = payloads
            .iter()
            .find(|value| value["method"] == "session/prompt")
            .unwrap();
        assert_eq!(prompt["params"]["prompt"][0]["text"], "explain main.rs");
    }

    #[test]
    fn grok_permission_uses_vendor_options_only() {
        let mut conv = HostedConversation::new(HostedAgentKind::Grok, None);
        let events = conv.ingest_line(
            r#"{"jsonrpc":"2.0","id":3,"method":"session/request_permission","params":{"options":[{"optionId":"allow-once","name":"Once"},{"optionId":"reject","name":"Reject"}]}}"#,
        );
        let Some(HarnessEvent::PermissionRequest(request)) = events.first() else {
            panic!("expected permission");
        };
        assert_eq!(request.options.len(), 2);
        assert!(conv.encode_permission_reply("allow_always").is_err());
        let reply = conv.encode_permission_reply("allow-once").unwrap();
        let json = &payload_strings(&reply)[0];
        assert_eq!(json["result"]["outcome"]["optionId"], "allow-once");
    }

    #[test]
    fn grok_resume_loads_stored_session() {
        let mut conv = HostedConversation::new(HostedAgentKind::Grok, Some("g-1".into()));
        let bytes = conv.encode_user_turn("hi").unwrap();
        let payloads = payload_strings(&bytes);
        let load = payloads
            .iter()
            .find(|value| value["method"] == "session/load")
            .unwrap();
        assert_eq!(load["params"]["sessionId"], "g-1");
    }

    #[test]
    fn grok_does_not_claim_mid_turn_steer_without_extension() {
        let conv = HostedConversation::new(HostedAgentKind::Grok, None);
        assert!(!conv.grok_supports_mid_turn_steer());
    }

    #[test]
    fn handshake_does_not_set_policy() {
        let mut claude = HostedConversation::new(HostedAgentKind::Claude, None);
        let mut codex = HostedConversation::new(HostedAgentKind::Codex, None);
        let mut grok = HostedConversation::new(HostedAgentKind::Grok, None);
        let combined = [
            claude.encode_handshake().unwrap(),
            codex.encode_handshake().unwrap(),
            grok.encode_handshake().unwrap(),
        ]
        .concat();
        let text = String::from_utf8_lossy(&combined);
        assert!(!text.contains("bypassPermissions"));
        assert!(!text.contains("agent-full-access"));
        assert!(!text.contains("yolo"));
    }
}
