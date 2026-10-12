//! Native agent chat: the normalized model shared by the ACP backend and the
//! chat view (issue #36).
//!
//! The agent's own session stays the source of truth. Vibra keeps only the
//! timeline it has seen while a chat is open and the pane ↔ session binding.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::agents::AgentKind;

/// Agents the chat view supports in the first stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatAgent {
    Grok,
    Codex,
    Claude,
}

impl ChatAgent {
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Grok];

    pub fn from_agent_kind(kind: AgentKind) -> Option<Self> {
        match kind {
            AgentKind::Grok => Some(Self::Grok),
            AgentKind::Codex => Some(Self::Codex),
            AgentKind::Claude => Some(Self::Claude),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Grok => "Grok",
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }

    /// Shell command that resumes `session_id` in the agent's terminal UI.
    /// The id is quoted for POSIX shells.
    pub fn resume_command(self, session_id: &str) -> String {
        let id = shell_quote(session_id);
        match self {
            Self::Grok => format!("grok -r {id}"),
            Self::Codex => format!("codex resume {id}"),
            Self::Claude => format!("claude --resume {id}"),
        }
    }
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A model the user has seen for an agent, remembered so the model picker can
/// list agents that are not running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    /// The value the agent's model option takes.
    pub value: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelCatalog {
    pub agent: ChatAgent,
    pub models: Vec<CatalogModel>,
}

/// Which view a pane shows. Terminal is the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaneViewMode {
    #[default]
    Terminal,
    Chat,
}

/// Persisted pane ↔ agent session binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatBinding {
    pub agent: ChatAgent,
    /// The agent's native session id; `None` until a session exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub mode: PaneViewMode,
}

/// What the backend advertised in `initialize`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatCapabilities {
    pub load_session: bool,
    pub list_sessions: bool,
    pub image_prompts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

impl ToolKind {
    /// Maps an ACP `ToolKind` string; unknown values become `Other`.
    pub fn from_acp(value: &str) -> Self {
        match value {
            "read" => Self::Read,
            "edit" => Self::Edit,
            "delete" => Self::Delete,
            "move" => Self::Move,
            "search" => Self::Search,
            "execute" => Self::Execute,
            "think" => Self::Think,
            "fetch" => Self::Fetch,
            "switch_mode" => Self::SwitchMode,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ToolStatus {
    pub fn from_acp(value: &str) -> Self {
        match value {
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolContent {
    Text(String),
    Diff {
        path: String,
        old_text: Option<String>,
        new_text: String,
    },
    /// Output of a client terminal created through `terminal/create`.
    Terminal {
        terminal_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallItem {
    /// ACP `toolCallId`.
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolStatus,
    pub content: Vec<ToolContent>,
    /// File paths the tool touches.
    pub locations: Vec<String>,
}

/// Partial update for an existing tool call; `None` keeps the current value.
/// `content` replaces the whole list, as in ACP.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCallUpdate {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub content: Option<Vec<ToolContent>>,
    pub locations: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanEntryStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub content: String,
    pub status: PlanEntryStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

/// One answer the agent offers. Reply with `id`, never with `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResolution {
    Selected(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionItem {
    /// Backend-assigned id used to answer the request.
    pub request_id: u64,
    pub tool_call_id: Option<String>,
    pub title: String,
    pub options: Vec<PermissionOption>,
    pub resolution: Option<PermissionResolution>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatItem {
    UserMessage {
        text: String,
        /// Labels of the files attached with `@`.
        files: Vec<String>,
    },
    /// Markdown, accumulated from streamed chunks.
    AssistantMessage {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolCall(ToolCallItem),
    Plan(Vec<PlanEntry>),
    Permission(PermissionItem),
    Notice {
        level: NoticeLevel,
        text: String,
    },
}

/// Lifecycle of the connection, as the view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatConnectionState {
    Starting,
    /// A session is open and no turn is running.
    Ready,
    /// A prompt turn is running.
    Working,
    /// The process ended or failed; the message explains why.
    Closed {
        message: Option<String>,
    },
}

/// A session mode the agent offers (for example "Ask", "Code", "Auto").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMode {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionModes {
    pub current: String,
    pub available: Vec<SessionMode>,
}

/// What a session config option controls. The composer shows `Model` and
/// `ThoughtLevel` together as the model picker, and `Mode` as the mode picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigCategory {
    Model,
    ThoughtLevel,
    Mode,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValue {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

/// A select-style session option (ACP `configOptions`, or the agent's model
/// list normalized into one). Reply with `id` and a `ConfigValue::value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub category: ConfigCategory,
    pub current: String,
    pub values: Vec<ConfigValue>,
}

impl ConfigOption {
    pub fn current_name(&self) -> &str {
        self.values
            .iter()
            .find(|value| value.value == self.current)
            .map_or(self.current.as_str(), |value| value.name.as_str())
    }
}

/// Context window use, from ACP `usage_update`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextUsage {
    pub used: u64,
    pub size: u64,
}

/// A command the agent accepts as `/name` in a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    /// Hint for the text after the command, when it takes one.
    pub input_hint: Option<String>,
}

/// A file attached to a prompt (`@` reference), sent as an ACP resource link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptFile {
    /// Absolute path.
    pub path: String,
    /// Label shown in the chat, usually the path relative to the project.
    pub label: String,
}

/// Backend → view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    Initialized {
        agent_name: Option<String>,
        capabilities: ChatCapabilities,
    },
    /// `loaded` is true when an existing session was opened with `session/load`.
    SessionStarted {
        session_id: String,
        loaded: bool,
    },
    UserMessageChunk {
        text: String,
    },
    AgentMessageChunk {
        text: String,
    },
    ThoughtChunk {
        text: String,
    },
    ToolCall(ToolCallItem),
    ToolCallUpdate(ToolCallUpdate),
    Plan(Vec<PlanEntry>),
    PermissionRequested(PermissionItem),
    /// Output appended to a client terminal; `exit_status` is set once it ends.
    TerminalOutput {
        terminal_id: String,
        output: String,
        exit_status: Option<i32>,
    },
    /// Sent when a `session/prompt` call returns.
    TurnFinished {
        stop_reason: String,
    },
    /// Current and available session modes (session setup or `current_mode_update`).
    Modes(SessionModes),
    /// The full set of session config options; replaces the previous set.
    ConfigOptions(Vec<ConfigOption>),
    /// Session title from `session_info_update`.
    Title(String),
    Usage(ContextUsage),
    /// Replaces the available slash commands.
    Commands(Vec<SlashCommand>),
    /// A recoverable failure; the connection stays open.
    Error {
        message: String,
    },
    /// Something the user should know that is not a failure.
    Notice {
        text: String,
    },
    /// The agent process ended. This is always the last event.
    Exited {
        message: Option<String>,
    },
}

/// View → backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatCommand {
    Prompt {
        text: String,
        files: Vec<PromptFile>,
    },
    Cancel,
    /// `session/set_mode`.
    SetMode {
        mode_id: String,
    },
    /// Changes a `ConfigOption` to one of its values.
    SetConfigOption {
        option_id: String,
        value: String,
    },
    ResolvePermission {
        request_id: u64,
        resolution: PermissionResolution,
    },
    /// Close the session and end the agent process.
    Shutdown,
}

/// The prompt sent locally for the current turn, and the part of its echo
/// the agent has sent back so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PendingEcho {
    prompt: String,
    seen: String,
}

/// How a prompt turn ended, as far as the timeline knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    /// The agent is still answering, or a replayed turn whose end is not known yet.
    Running,
    /// Ended normally (`end_turn`), or a replayed turn followed by another prompt.
    Completed,
    /// The user stopped it.
    Cancelled,
    /// Refused, hit a limit, reported an error, or the agent process ended.
    Failed,
}

/// One prompt turn: a user message and everything after it up to the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRecord {
    /// Index in `ChatTimeline::items` of the user message that starts the turn.
    pub start: usize,
    pub outcome: TurnOutcome,
    /// When the turn ended; set by `ChatTimeline::stamp`, `None` for replayed history.
    pub finished_at: Option<SystemTime>,
    /// An error notice arrived while the turn ran.
    errored: bool,
}

/// Everything the view renders, rebuilt only from `ChatEvent`s.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatTimeline {
    pub items: Vec<ChatItem>,
    pub session_id: Option<String>,
    pub agent_name: Option<String>,
    pub capabilities: ChatCapabilities,
    pub state: Option<ChatConnectionState>,
    /// Terminal output by `terminal_id`, with its exit status once known.
    pub terminals: Vec<(String, String, Option<i32>)>,
    pub title: Option<String>,
    pub modes: Option<SessionModes>,
    pub config_options: Vec<ConfigOption>,
    pub usage: Option<ContextUsage>,
    pub commands: Vec<SlashCommand>,
    /// Prompt turns in order; `turns[i].start` indexes `items`.
    pub turns: Vec<TurnRecord>,
    /// When each item arrived, parallel to `items` up to the last `stamp`.
    /// `None` for history replayed by `session/load`.
    pub item_times: Vec<Option<SystemTime>>,
    /// Set while the agent may still echo the last local prompt.
    pending_echo: Option<PendingEcho>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextKind {
    User,
    Assistant,
    Reasoning,
}

/// Terminal output kept per terminal, in bytes.
const TERMINAL_OUTPUT_LIMIT: usize = 64 * 1024;

impl ChatTimeline {
    /// Folds one backend event into the timeline. Pure; see the tests.
    pub fn apply(&mut self, event: ChatEvent) {
        // The echo window only covers user chunks right after the local prompt.
        if !matches!(event, ChatEvent::UserMessageChunk { .. }) {
            self.pending_echo = None;
        }
        match event {
            ChatEvent::Initialized {
                agent_name,
                capabilities,
            } => {
                self.agent_name = agent_name;
                self.capabilities = capabilities;
                self.state = Some(ChatConnectionState::Starting);
            }
            ChatEvent::SessionStarted { session_id, .. } => {
                self.session_id = Some(session_id);
                self.state = Some(ChatConnectionState::Ready);
                // A loaded session has replayed its history by now.
                self.end_turn(TurnOutcome::Completed);
            }
            ChatEvent::UserMessageChunk { text } => self.apply_user_chunk(text),
            ChatEvent::AgentMessageChunk { text } => self.append_text(TextKind::Assistant, text),
            ChatEvent::ThoughtChunk { text } => self.append_text(TextKind::Reasoning, text),
            ChatEvent::ToolCall(call) => match self.find_tool_call_mut(&call.id) {
                Some(existing) => *existing = call,
                None => self.items.push(ChatItem::ToolCall(call)),
            },
            ChatEvent::ToolCallUpdate(update) => self.update_tool_call(update),
            ChatEvent::Plan(entries) => self.set_plan(entries),
            ChatEvent::PermissionRequested(item) => self.items.push(ChatItem::Permission(item)),
            ChatEvent::TerminalOutput {
                terminal_id,
                output,
                exit_status,
            } => self.append_terminal_output(terminal_id, &output, exit_status),
            ChatEvent::TurnFinished { stop_reason } => {
                self.state = Some(ChatConnectionState::Ready);
                self.cancel_unresolved_permissions();
                self.end_turn(match stop_reason.as_str() {
                    "cancelled" => TurnOutcome::Cancelled,
                    "refusal" | "max_tokens" => TurnOutcome::Failed,
                    _ => TurnOutcome::Completed,
                });
                // A stopped turn says so in its header, so it needs no notice.
                let notice = match stop_reason.as_str() {
                    "refusal" => Some("The agent refused to continue."),
                    "max_tokens" => Some("The agent reached its token limit."),
                    _ => None,
                };
                if let Some(text) = notice {
                    self.push_notice(NoticeLevel::Info, text);
                }
            }
            ChatEvent::Modes(modes) => self.modes = Some(modes),
            ChatEvent::ConfigOptions(options) => self.config_options = options,
            ChatEvent::Title(title) => self.title = Some(title),
            ChatEvent::Usage(usage) => self.usage = Some(usage),
            ChatEvent::Commands(commands) => self.commands = commands,
            ChatEvent::Error { message } => {
                if let Some(turn) = self.running_turn_mut() {
                    turn.errored = true;
                }
                self.push_notice(NoticeLevel::Error, &message);
            }
            ChatEvent::Notice { text } => self.push_notice(NoticeLevel::Info, &text),
            ChatEvent::Exited { message } => {
                self.state = Some(ChatConnectionState::Closed { message });
                self.cancel_unresolved_permissions();
                self.end_turn(TurnOutcome::Failed);
            }
        }
    }

    /// Records a prompt the user sent before the agent echoes anything.
    pub fn push_user_prompt(&mut self, text: String, files: Vec<String>) {
        self.pending_echo = Some(PendingEcho {
            prompt: text.clone(),
            seen: String::new(),
        });
        self.push_user_item(text, files);
        self.state = Some(ChatConnectionState::Working);
    }

    /// Records when new items and finished turns happened. `apply` stays pure;
    /// the view calls this after each change. Items that arrive before the
    /// session has an id are replayed history and get no time.
    pub fn stamp(&mut self, now: SystemTime) {
        let live = self.session_id.is_some();
        while self.item_times.len() < self.items.len() {
            self.item_times.push(live.then_some(now));
        }
        self.item_times.truncate(self.items.len());
        for turn in &mut self.turns {
            let timed = self.item_times.get(turn.start).copied().flatten().is_some();
            if turn.outcome != TurnOutcome::Running && turn.finished_at.is_none() && timed {
                turn.finished_at = Some(now);
            }
        }
    }

    /// When `items[index]` arrived, if known.
    pub fn item_time(&self, index: usize) -> Option<SystemTime> {
        self.item_times.get(index).copied().flatten()
    }

    /// When the turn started (its user message arrived), if known.
    pub fn turn_started_at(&self, turn: &TurnRecord) -> Option<SystemTime> {
        self.item_time(turn.start)
    }

    fn running_turn_mut(&mut self) -> Option<&mut TurnRecord> {
        self.turns
            .last_mut()
            .filter(|turn| turn.outcome == TurnOutcome::Running)
    }

    /// Ends the running turn, if any. A turn that reported an error never counts as completed.
    fn end_turn(&mut self, outcome: TurnOutcome) {
        if let Some(turn) = self.running_turn_mut() {
            turn.outcome = if outcome == TurnOutcome::Completed && turn.errored {
                TurnOutcome::Failed
            } else {
                outcome
            };
        }
    }

    /// Pushes a user message and starts its turn; the previous turn, if still
    /// open (replayed history), ends as completed.
    fn push_user_item(&mut self, text: String, files: Vec<String>) {
        self.end_turn(TurnOutcome::Completed);
        self.turns.push(TurnRecord {
            start: self.items.len(),
            outcome: TurnOutcome::Running,
            finished_at: None,
            errored: false,
        });
        self.items.push(ChatItem::UserMessage { text, files });
    }

    /// True while a prompt turn is running.
    pub fn is_working(&self) -> bool {
        matches!(self.state, Some(ChatConnectionState::Working))
    }

    /// Output and exit status of a client terminal, if it was seen.
    pub fn terminal(&self, id: &str) -> Option<(&str, Option<i32>)> {
        self.terminals
            .iter()
            .find(|(terminal_id, _, _)| terminal_id.as_str() == id)
            .map(|(_, output, status)| (output.as_str(), *status))
    }

    fn apply_user_chunk(&mut self, text: String) {
        if let Some(echo) = self.pending_echo.as_mut() {
            let candidate = format!("{}{}", echo.seen, text);
            if echo.prompt.starts_with(&candidate) {
                // Swallow the echo of the local prompt; the item already shows it.
                if candidate == echo.prompt {
                    self.pending_echo = None;
                } else {
                    echo.seen = candidate;
                }
                return;
            }
            // Not an echo of the local prompt, so it is a separate message.
            self.pending_echo = None;
            self.push_user_item(candidate, Vec::new());
            return;
        }
        self.append_text(TextKind::User, text);
    }

    /// Appends to the last item when it has the same kind, else starts a new one.
    fn append_text(&mut self, kind: TextKind, text: String) {
        if text.is_empty() {
            return;
        }
        let appended = match (self.items.last_mut(), kind) {
            (Some(ChatItem::UserMessage { text: last, .. }), TextKind::User)
            | (Some(ChatItem::AssistantMessage { text: last }), TextKind::Assistant)
            | (Some(ChatItem::Reasoning { text: last }), TextKind::Reasoning) => {
                last.push_str(&text);
                true
            }
            _ => false,
        };
        if !appended {
            match kind {
                TextKind::User => self.push_user_item(text, Vec::new()),
                TextKind::Assistant => self.items.push(ChatItem::AssistantMessage { text }),
                TextKind::Reasoning => self.items.push(ChatItem::Reasoning { text }),
            }
        }
    }

    fn find_tool_call_mut(&mut self, id: &str) -> Option<&mut ToolCallItem> {
        self.items.iter_mut().find_map(|item| match item {
            ChatItem::ToolCall(call) if call.id == id => Some(call),
            _ => None,
        })
    }

    fn update_tool_call(&mut self, update: ToolCallUpdate) {
        if let Some(call) = self.find_tool_call_mut(&update.id) {
            if let Some(title) = update.title {
                call.title = title;
            }
            if let Some(kind) = update.kind {
                call.kind = kind;
            }
            if let Some(status) = update.status {
                call.status = status;
            }
            if let Some(content) = update.content {
                call.content = content;
            }
            if let Some(locations) = update.locations {
                call.locations = locations;
            }
            return;
        }
        // An update can arrive before its tool call; create it so nothing is lost.
        let fallback_title = if update.id.is_empty() {
            "Tool call".to_owned()
        } else {
            update.id.clone()
        };
        self.items.push(ChatItem::ToolCall(ToolCallItem {
            title: update.title.unwrap_or(fallback_title),
            kind: update.kind.unwrap_or(ToolKind::Other),
            status: update.status.unwrap_or(ToolStatus::Pending),
            content: update.content.unwrap_or_default(),
            locations: update.locations.unwrap_or_default(),
            id: update.id,
        }));
    }

    /// The plan is live only until the next user message; later plans start fresh.
    fn set_plan(&mut self, entries: Vec<PlanEntry>) {
        let last_plan = self
            .items
            .iter()
            .rposition(|item| matches!(item, ChatItem::Plan(_)));
        let last_user = self
            .items
            .iter()
            .rposition(|item| matches!(item, ChatItem::UserMessage { .. }));
        match last_plan {
            Some(index) if last_user.is_none_or(|user| index > user) => {
                self.items[index] = ChatItem::Plan(entries);
            }
            _ => self.items.push(ChatItem::Plan(entries)),
        }
    }

    fn permissions_mut(&mut self) -> impl Iterator<Item = &mut PermissionItem> {
        self.items.iter_mut().filter_map(|item| match item {
            ChatItem::Permission(permission) => Some(permission),
            _ => None,
        })
    }

    /// Answers the permission request `request_id` if it is still open.
    /// Returns false when no open request has that id.
    pub fn resolve_permission(
        &mut self,
        request_id: u64,
        resolution: PermissionResolution,
    ) -> bool {
        match self.permissions_mut().find(|permission| {
            permission.request_id == request_id && permission.resolution.is_none()
        }) {
            Some(permission) => {
                permission.resolution = Some(resolution);
                true
            }
            None => false,
        }
    }

    fn cancel_unresolved_permissions(&mut self) {
        for permission in self.permissions_mut() {
            if permission.resolution.is_none() {
                permission.resolution = Some(PermissionResolution::Cancelled);
            }
        }
    }

    fn push_notice(&mut self, level: NoticeLevel, text: &str) {
        self.items.push(ChatItem::Notice {
            level,
            text: text.to_owned(),
        });
    }

    fn append_terminal_output(
        &mut self,
        terminal_id: String,
        output: &str,
        exit_status: Option<i32>,
    ) {
        let index = match self
            .terminals
            .iter()
            .position(|(id, _, _)| *id == terminal_id)
        {
            Some(index) => index,
            None => {
                self.terminals.push((terminal_id, String::new(), None));
                self.terminals.len() - 1
            }
        };
        let (_, buffer, status) = &mut self.terminals[index];
        buffer.push_str(output);
        keep_tail(buffer, TERMINAL_OUTPUT_LIMIT);
        if exit_status.is_some() {
            *status = exit_status;
        }
    }

    /// Permission requests that still need an answer.
    #[cfg(test)]
    pub fn pending_permissions(&self) -> impl Iterator<Item = &PermissionItem> {
        self.items.iter().filter_map(|item| match item {
            ChatItem::Permission(permission) if permission.resolution.is_none() => Some(permission),
            _ => None,
        })
    }
}

/// Drops the oldest bytes so at most `limit` remain, cutting on a char boundary.
fn keep_tail(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.drain(..start);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn replay(events: Vec<ChatEvent>) -> ChatTimeline {
        let mut timeline = ChatTimeline::default();
        for event in events {
            timeline.apply(event);
        }
        timeline
    }

    fn ready() -> ChatEvent {
        ChatEvent::SessionStarted {
            session_id: "s1".to_owned(),
            loaded: false,
        }
    }

    fn agent_chunk(text: &str) -> ChatEvent {
        ChatEvent::AgentMessageChunk {
            text: text.to_owned(),
        }
    }

    fn user_chunk(text: &str) -> ChatEvent {
        ChatEvent::UserMessageChunk {
            text: text.to_owned(),
        }
    }

    fn thought(text: &str) -> ChatEvent {
        ChatEvent::ThoughtChunk {
            text: text.to_owned(),
        }
    }

    fn tool(id: &str, title: &str) -> ToolCallItem {
        ToolCallItem {
            id: id.to_owned(),
            title: title.to_owned(),
            kind: ToolKind::Read,
            status: ToolStatus::Pending,
            content: Vec::new(),
            locations: Vec::new(),
        }
    }

    fn tool_update(id: &str) -> ToolCallUpdate {
        ToolCallUpdate {
            id: id.to_owned(),
            ..Default::default()
        }
    }

    fn permission(request_id: u64) -> PermissionItem {
        PermissionItem {
            request_id,
            tool_call_id: None,
            title: format!("Permission {request_id}"),
            options: Vec::new(),
            resolution: None,
        }
    }

    fn plan_entry(content: &str, status: PlanEntryStatus) -> PlanEntry {
        PlanEntry {
            content: content.to_owned(),
            status,
        }
    }

    fn tool_call<'a>(timeline: &'a ChatTimeline, id: &str) -> &'a ToolCallItem {
        timeline
            .items
            .iter()
            .find_map(|item| match item {
                ChatItem::ToolCall(call) if call.id == id => Some(call),
                _ => None,
            })
            .expect("tool call exists")
    }

    fn permission_item(timeline: &ChatTimeline, request_id: u64) -> &PermissionItem {
        timeline
            .items
            .iter()
            .find_map(|item| match item {
                ChatItem::Permission(item) if item.request_id == request_id => Some(item),
                _ => None,
            })
            .expect("permission exists")
    }

    fn notices(timeline: &ChatTimeline) -> Vec<(NoticeLevel, String)> {
        timeline
            .items
            .iter()
            .filter_map(|item| match item {
                ChatItem::Notice { level, text } => Some((*level, text.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn initialize_and_session_start_drive_connection_state() {
        let mut timeline = ChatTimeline::default();
        timeline.apply(ChatEvent::Initialized {
            agent_name: Some("Agent".to_owned()),
            capabilities: ChatCapabilities {
                load_session: true,
                ..Default::default()
            },
        });
        assert_eq!(timeline.agent_name.as_deref(), Some("Agent"));
        assert!(timeline.capabilities.load_session);
        assert_eq!(timeline.state, Some(ChatConnectionState::Starting));

        timeline.apply(ready());
        assert_eq!(timeline.session_id.as_deref(), Some("s1"));
        assert_eq!(timeline.state, Some(ChatConnectionState::Ready));
        assert!(!timeline.is_working());
    }

    #[test]
    fn user_prompt_starts_a_working_turn() {
        let mut timeline = replay(vec![ready()]);
        timeline.push_user_prompt("hi".to_owned(), Vec::new());
        assert_eq!(
            timeline.items,
            vec![ChatItem::UserMessage {
                files: Vec::new(),
                text: "hi".to_owned()
            }]
        );
        assert!(timeline.is_working());
    }

    #[test]
    fn echo_of_local_prompt_is_not_duplicated() {
        let mut timeline = replay(vec![ready()]);
        timeline.push_user_prompt("hello world".to_owned(), Vec::new());
        timeline.apply(user_chunk("hello"));
        timeline.apply(user_chunk(" world"));
        assert_eq!(
            timeline.items,
            vec![ChatItem::UserMessage {
                files: Vec::new(),
                text: "hello world".to_owned()
            }]
        );

        timeline.apply(agent_chunk("Hi"));
        assert_eq!(timeline.items.len(), 2);
        assert_eq!(
            timeline.items[1],
            ChatItem::AssistantMessage {
                text: "Hi".to_owned()
            }
        );
    }

    #[test]
    fn echo_that_differs_from_local_prompt_is_a_new_message() {
        let mut timeline = replay(vec![ready()]);
        timeline.push_user_prompt("hi".to_owned(), Vec::new());
        timeline.apply(user_chunk("other"));
        assert_eq!(
            timeline.items,
            vec![
                ChatItem::UserMessage {
                    files: Vec::new(),
                    text: "hi".to_owned()
                },
                ChatItem::UserMessage {
                    files: Vec::new(),
                    text: "other".to_owned()
                },
            ]
        );
    }

    #[test]
    fn echo_window_closes_on_any_other_event() {
        let mut timeline = replay(vec![ready()]);
        timeline.push_user_prompt("hi".to_owned(), Vec::new());
        timeline.apply(agent_chunk("ok"));
        timeline.apply(user_chunk("hi"));
        assert_eq!(
            timeline.items,
            vec![
                ChatItem::UserMessage {
                    files: Vec::new(),
                    text: "hi".to_owned()
                },
                ChatItem::AssistantMessage {
                    text: "ok".to_owned()
                },
                ChatItem::UserMessage {
                    files: Vec::new(),
                    text: "hi".to_owned()
                },
            ]
        );
    }

    #[test]
    fn history_user_chunks_merge_into_one_message() {
        let timeline = replay(vec![user_chunk("a"), user_chunk("b")]);
        assert_eq!(
            timeline.items,
            vec![ChatItem::UserMessage {
                files: Vec::new(),
                text: "ab".to_owned()
            }]
        );
    }

    #[test]
    fn chunks_append_only_to_the_same_kind() {
        let timeline = replay(vec![
            agent_chunk("Hel"),
            agent_chunk("lo"),
            thought("think"),
            agent_chunk("!"),
        ]);
        assert_eq!(
            timeline.items,
            vec![
                ChatItem::AssistantMessage {
                    text: "Hello".to_owned()
                },
                ChatItem::Reasoning {
                    text: "think".to_owned()
                },
                ChatItem::AssistantMessage {
                    text: "!".to_owned()
                },
            ]
        );
    }

    #[test]
    fn empty_chunks_do_not_create_items() {
        let timeline = replay(vec![thought(""), agent_chunk("")]);
        assert!(timeline.items.is_empty());
    }

    #[test]
    fn tool_call_with_known_id_replaces_it_in_place() {
        let timeline = replay(vec![
            ChatEvent::ToolCall(tool("a", "Read")),
            ChatEvent::ToolCall(tool("b", "Edit")),
            ChatEvent::ToolCall(tool("a", "Read file")),
        ]);
        assert_eq!(timeline.items.len(), 2);
        assert_eq!(tool_call(&timeline, "a").title, "Read file");
        assert!(matches!(&timeline.items[0], ChatItem::ToolCall(call) if call.id == "a"));
        assert!(matches!(&timeline.items[1], ChatItem::ToolCall(call) if call.id == "b"));
    }

    #[test]
    fn tool_call_update_merges_only_the_fields_present() {
        let mut call = tool("a", "Read");
        call.content = vec![ToolContent::Text("old".to_owned())];
        call.locations = vec!["src/lib.rs".to_owned()];
        let mut timeline = replay(vec![ChatEvent::ToolCall(call)]);

        timeline.apply(ChatEvent::ToolCallUpdate(ToolCallUpdate {
            status: Some(ToolStatus::Completed),
            ..tool_update("a")
        }));
        let merged = tool_call(&timeline, "a");
        assert_eq!(merged.title, "Read");
        assert_eq!(merged.kind, ToolKind::Read);
        assert_eq!(merged.status, ToolStatus::Completed);
        assert_eq!(merged.content, vec![ToolContent::Text("old".to_owned())]);
        assert_eq!(merged.locations, vec!["src/lib.rs".to_owned()]);

        timeline.apply(ChatEvent::ToolCallUpdate(ToolCallUpdate {
            title: Some("Read main.rs".to_owned()),
            kind: Some(ToolKind::Edit),
            content: Some(vec![ToolContent::Text("new".to_owned())]),
            locations: Some(Vec::new()),
            ..tool_update("a")
        }));
        let replaced = tool_call(&timeline, "a");
        assert_eq!(replaced.title, "Read main.rs");
        assert_eq!(replaced.kind, ToolKind::Edit);
        assert_eq!(replaced.status, ToolStatus::Completed);
        assert_eq!(replaced.content, vec![ToolContent::Text("new".to_owned())]);
        assert!(replaced.locations.is_empty());
    }

    #[test]
    fn tool_call_update_before_the_call_creates_it() {
        let mut timeline = replay(vec![ChatEvent::ToolCallUpdate(ToolCallUpdate {
            status: Some(ToolStatus::InProgress),
            ..tool_update("z")
        })]);
        let created = tool_call(&timeline, "z");
        assert_eq!(created.title, "z");
        assert_eq!(created.kind, ToolKind::Other);
        assert_eq!(created.status, ToolStatus::InProgress);
        assert!(created.content.is_empty());

        timeline.apply(ChatEvent::ToolCallUpdate(tool_update("")));
        let untitled = timeline.items.last().expect("item pushed");
        assert_eq!(
            untitled,
            &ChatItem::ToolCall(ToolCallItem {
                id: String::new(),
                title: "Tool call".to_owned(),
                kind: ToolKind::Other,
                status: ToolStatus::Pending,
                content: Vec::new(),
                locations: Vec::new(),
            })
        );
    }

    #[test]
    fn plan_replaces_the_latest_plan_of_the_current_turn() {
        let second = vec![plan_entry("a", PlanEntryStatus::Completed)];
        let third = vec![plan_entry("b", PlanEntryStatus::Pending)];
        let mut timeline = replay(vec![ready()]);
        timeline.push_user_prompt("one".to_owned(), Vec::new());
        timeline.apply(ChatEvent::Plan(vec![plan_entry(
            "a",
            PlanEntryStatus::Pending,
        )]));
        timeline.apply(ChatEvent::Plan(second.clone()));
        assert_eq!(
            timeline.items,
            vec![
                ChatItem::UserMessage {
                    files: Vec::new(),
                    text: "one".to_owned()
                },
                ChatItem::Plan(second.clone()),
            ]
        );

        timeline.push_user_prompt("two".to_owned(), Vec::new());
        timeline.apply(ChatEvent::Plan(third.clone()));
        assert_eq!(timeline.items.len(), 4);
        assert_eq!(timeline.items[1], ChatItem::Plan(second));
        assert_eq!(timeline.items[3], ChatItem::Plan(third));
    }

    #[test]
    fn plan_without_a_user_message_replaces_the_previous_plan() {
        let timeline = replay(vec![
            ChatEvent::Plan(vec![plan_entry("a", PlanEntryStatus::Pending)]),
            ChatEvent::Plan(vec![plan_entry("b", PlanEntryStatus::InProgress)]),
        ]);
        assert_eq!(
            timeline.items,
            vec![ChatItem::Plan(vec![plan_entry(
                "b",
                PlanEntryStatus::InProgress
            )])]
        );
    }

    #[test]
    fn resolve_permission_answers_an_open_request_once() {
        let mut timeline = replay(vec![ChatEvent::PermissionRequested(permission(7))]);
        assert_eq!(timeline.pending_permissions().count(), 1);

        assert!(!timeline.resolve_permission(8, PermissionResolution::Cancelled));
        assert!(timeline.resolve_permission(7, PermissionResolution::Selected("allow".to_owned())));
        assert!(!timeline.resolve_permission(7, PermissionResolution::Cancelled));

        assert_eq!(
            permission_item(&timeline, 7).resolution,
            Some(PermissionResolution::Selected("allow".to_owned()))
        );
        assert_eq!(timeline.pending_permissions().count(), 0);
    }

    #[test]
    fn turn_finished_cancels_only_open_permissions() {
        let mut timeline = replay(vec![
            ChatEvent::PermissionRequested(permission(1)),
            ChatEvent::PermissionRequested(permission(2)),
        ]);
        timeline.resolve_permission(2, PermissionResolution::Selected("ok".to_owned()));
        timeline.apply(ChatEvent::TurnFinished {
            stop_reason: "end_turn".to_owned(),
        });

        assert_eq!(timeline.state, Some(ChatConnectionState::Ready));
        assert_eq!(
            permission_item(&timeline, 1).resolution,
            Some(PermissionResolution::Cancelled)
        );
        assert_eq!(
            permission_item(&timeline, 2).resolution,
            Some(PermissionResolution::Selected("ok".to_owned()))
        );
        assert_eq!(timeline.pending_permissions().count(), 0);
        assert!(notices(&timeline).is_empty());
    }

    #[test]
    fn turn_finished_reports_abnormal_stop_reasons() {
        let cases = [
            ("refusal", Some("The agent refused to continue.")),
            ("max_tokens", Some("The agent reached its token limit.")),
            ("cancelled", None),
            ("end_turn", None),
        ];
        for (stop_reason, expected) in cases {
            let timeline = replay(vec![
                ready(),
                ChatEvent::TurnFinished {
                    stop_reason: stop_reason.to_owned(),
                },
            ]);
            let expected: Vec<(NoticeLevel, String)> = expected
                .map(|text| (NoticeLevel::Info, text.to_owned()))
                .into_iter()
                .collect();
            assert_eq!(notices(&timeline), expected, "stop reason {stop_reason}");
        }
    }

    #[test]
    fn error_adds_an_error_notice_and_keeps_the_state() {
        let mut timeline = replay(vec![ready()]);
        timeline.apply(ChatEvent::Error {
            message: "boom".to_owned(),
        });
        assert_eq!(
            notices(&timeline),
            vec![(NoticeLevel::Error, "boom".to_owned())]
        );
        assert_eq!(timeline.state, Some(ChatConnectionState::Ready));
    }

    #[test]
    fn exit_closes_the_timeline_and_cancels_open_permissions() {
        let mut running = tool("a", "Run");
        running.status = ToolStatus::InProgress;
        let mut timeline = replay(vec![
            ready(),
            ChatEvent::ToolCall(running),
            ChatEvent::PermissionRequested(permission(3)),
        ]);
        timeline.push_user_prompt("go".to_owned(), Vec::new());
        timeline.apply(ChatEvent::Exited {
            message: Some("boom".to_owned()),
        });

        assert_eq!(
            timeline.state,
            Some(ChatConnectionState::Closed {
                message: Some("boom".to_owned())
            })
        );
        assert!(!timeline.is_working());
        assert_eq!(
            permission_item(&timeline, 3).resolution,
            Some(PermissionResolution::Cancelled)
        );
        assert_eq!(tool_call(&timeline, "a").status, ToolStatus::InProgress);
    }

    #[test]
    fn terminal_output_accumulates_and_records_exit_status() {
        let timeline = replay(vec![
            ChatEvent::TerminalOutput {
                terminal_id: "t1".to_owned(),
                output: "ab".to_owned(),
                exit_status: None,
            },
            ChatEvent::TerminalOutput {
                terminal_id: "t2".to_owned(),
                output: "x".to_owned(),
                exit_status: None,
            },
            ChatEvent::TerminalOutput {
                terminal_id: "t1".to_owned(),
                output: "cd".to_owned(),
                exit_status: Some(0),
            },
        ]);
        assert_eq!(timeline.terminals.len(), 2);
        assert_eq!(timeline.terminal("t1"), Some(("abcd", Some(0))));
        assert_eq!(timeline.terminal("t2"), Some(("x", None)));
        assert_eq!(timeline.terminal("missing"), None);
    }

    #[test]
    fn terminal_output_keeps_the_last_64_kib_on_a_char_boundary() {
        // 80_001 bytes of two-byte chars plus one ASCII byte: the cut at byte
        // 14_465 lands inside an `é` and must move forward to the next char.
        let output = format!("{}x", "é".repeat(40_000));
        let timeline = replay(vec![ChatEvent::TerminalOutput {
            terminal_id: "t".to_owned(),
            output,
            exit_status: None,
        }]);
        let (kept, _) = timeline.terminal("t").expect("terminal stored");
        assert_eq!(kept.len(), TERMINAL_OUTPUT_LIMIT - 1);
        assert!(
            kept.strip_suffix('x')
                .is_some_and(|rest| rest.chars().all(|c| c == 'é'))
        );
    }

    #[test]
    fn keep_tail_leaves_short_text_unchanged() {
        let mut text = "short".to_owned();
        keep_tail(&mut text, TERMINAL_OUTPUT_LIMIT);
        assert_eq!(text, "short");
    }

    #[test]
    fn resume_command_uses_each_agent_cli() {
        assert_eq!(
            ChatAgent::Grok.resume_command("abc-123_x.y"),
            "grok -r abc-123_x.y"
        );
        assert_eq!(
            ChatAgent::Codex.resume_command("0191f"),
            "codex resume 0191f"
        );
        assert_eq!(
            ChatAgent::Claude.resume_command("sess1"),
            "claude --resume sess1"
        );
    }

    #[test]
    fn resume_command_quotes_unsafe_session_ids() {
        assert_eq!(
            ChatAgent::Claude.resume_command("has space"),
            "claude --resume 'has space'"
        );
        assert_eq!(
            ChatAgent::Claude.resume_command("it's"),
            r"claude --resume 'it'\''s'"
        );
        assert_eq!(
            ChatAgent::Grok.resume_command("$(rm -rf ~)"),
            "grok -r '$(rm -rf ~)'"
        );
        assert_eq!(ChatAgent::Codex.resume_command(""), "codex resume ''");
    }

    #[test]
    fn binding_serializes_camel_case_and_omits_missing_session() {
        let unbound = AgentChatBinding {
            agent: ChatAgent::Claude,
            session_id: None,
            mode: PaneViewMode::Terminal,
        };
        assert_eq!(
            serde_json::to_value(&unbound).expect("serializes"),
            json!({"agent": "claude", "mode": "terminal"})
        );

        let bound = AgentChatBinding {
            agent: ChatAgent::Codex,
            session_id: Some("s-1".to_owned()),
            mode: PaneViewMode::Chat,
        };
        assert_eq!(
            serde_json::to_value(&bound).expect("serializes"),
            json!({"agent": "codex", "sessionId": "s-1", "mode": "chat"})
        );
    }

    #[test]
    fn binding_round_trips_and_defaults_mode_to_terminal() {
        let bound = AgentChatBinding {
            agent: ChatAgent::Grok,
            session_id: Some("s-2".to_owned()),
            mode: PaneViewMode::Chat,
        };
        let json = serde_json::to_string(&bound).expect("serializes");
        let back: AgentChatBinding = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, bound);

        let minimal: AgentChatBinding =
            serde_json::from_value(json!({"agent": "grok"})).expect("deserializes");
        assert_eq!(
            minimal,
            AgentChatBinding {
                agent: ChatAgent::Grok,
                session_id: None,
                mode: PaneViewMode::Terminal,
            }
        );
    }

    fn outcomes(timeline: &ChatTimeline) -> Vec<(usize, TurnOutcome)> {
        timeline
            .turns
            .iter()
            .map(|turn| (turn.start, turn.outcome))
            .collect()
    }

    #[test]
    fn each_prompt_starts_a_turn_that_ends_with_its_stop_reason() {
        let mut timeline = replay(vec![ready()]);
        timeline.push_user_prompt("one".to_owned(), Vec::new());
        assert_eq!(outcomes(&timeline), vec![(0, TurnOutcome::Running)]);
        timeline.apply(agent_chunk("done"));
        timeline.apply(ChatEvent::TurnFinished {
            stop_reason: "end_turn".to_owned(),
        });
        timeline.push_user_prompt("two".to_owned(), Vec::new());
        timeline.apply(ChatEvent::TurnFinished {
            stop_reason: "cancelled".to_owned(),
        });
        timeline.push_user_prompt("three".to_owned(), Vec::new());
        timeline.apply(ChatEvent::Error {
            message: "boom".to_owned(),
        });
        timeline.apply(ChatEvent::TurnFinished {
            stop_reason: "end_turn".to_owned(),
        });
        assert_eq!(
            outcomes(&timeline),
            vec![
                (0, TurnOutcome::Completed),
                (2, TurnOutcome::Cancelled),
                (3, TurnOutcome::Failed),
            ]
        );
    }

    #[test]
    fn replayed_turns_complete_when_the_next_one_starts_or_the_session_opens() {
        let timeline = replay(vec![
            ChatEvent::UserMessageChunk {
                text: "old".to_owned(),
            },
            agent_chunk("answer"),
            ChatEvent::UserMessageChunk {
                text: "older".to_owned(),
            },
            agent_chunk("again"),
            ready(),
        ]);
        assert_eq!(
            outcomes(&timeline),
            vec![(0, TurnOutcome::Completed), (2, TurnOutcome::Completed)]
        );
    }

    #[test]
    fn an_exit_fails_the_running_turn() {
        let mut timeline = replay(vec![ready()]);
        timeline.push_user_prompt("go".to_owned(), Vec::new());
        timeline.apply(ChatEvent::Exited { message: None });
        assert_eq!(outcomes(&timeline), vec![(0, TurnOutcome::Failed)]);
    }

    #[test]
    fn stamps_time_live_items_and_finished_turns_but_not_history() {
        let start = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100);
        let end = start + std::time::Duration::from_secs(42);
        let mut timeline = replay(vec![
            ChatEvent::UserMessageChunk {
                text: "old".to_owned(),
            },
            agent_chunk("answer"),
        ]);
        timeline.stamp(start);
        timeline.apply(ready());
        timeline.push_user_prompt("new".to_owned(), Vec::new());
        timeline.stamp(start);
        assert_eq!(timeline.item_times, vec![None, None, Some(start)]);
        assert_eq!(timeline.turns[0].finished_at, None);
        assert_eq!(timeline.turn_started_at(&timeline.turns[1]), Some(start));
        timeline.apply(ChatEvent::TurnFinished {
            stop_reason: "end_turn".to_owned(),
        });
        timeline.stamp(end);
        timeline.stamp(end + std::time::Duration::from_secs(5));
        assert_eq!(timeline.turns[1].finished_at, Some(end));
    }
}
