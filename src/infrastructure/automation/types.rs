use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub(crate) const MAX_AUTOMATION_REQUEST_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_AUTOMATION_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
pub(crate) const MAX_AGENT_HOOK_BYTES: u64 = 1024 * 1024;
pub(crate) const AUTOMATION_IO_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) static NEXT_AUTOMATION_SERVER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutomationDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRuntimeState {
    Idle,
    Working,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentKind {
    Aider,
    Amp,
    Claude,
    Codex,
    Cursor,
    Gemini,
    Goose,
    Grok,
    OpenCode,
    Pi,
}

impl AgentKind {
    pub const ALL: [Self; 10] = [
        Self::Aider,
        Self::Amp,
        Self::Claude,
        Self::Codex,
        Self::Cursor,
        Self::Gemini,
        Self::Goose,
        Self::Grok,
        Self::OpenCode,
        Self::Pi,
    ];

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Aider => "Aider",
            Self::Amp => "Amp",
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Gemini => "Gemini",
            Self::Goose => "Goose",
            Self::Grok => "Grok",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
        }
    }

    pub const fn cli_name(self) -> &'static str {
        match self {
            Self::Aider => "aider",
            Self::Amp => "amp",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Gemini => "gemini",
            Self::Goose => "goose",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }

    pub const fn executable(self) -> &'static str {
        match self {
            Self::Aider => "aider",
            Self::Amp => "amp",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor-agent",
            Self::Gemini => "gemini",
            Self::Goose => "goose",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "aider" => Some(Self::Aider),
            "amp" => Some(Self::Amp),
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "cursor" | "cursor-agent" => Some(Self::Cursor),
            "gemini" => Some(Self::Gemini),
            "goose" => Some(Self::Goose),
            "grok" => Some(Self::Grok),
            "opencode" => Some(Self::OpenCode),
            "pi" => Some(Self::Pi),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentPlacement {
    Current,
    #[default]
    Split,
    Tab,
}

pub(crate) const DEFAULT_AGENT_START_TIMEOUT_MS: u64 = 30_000;
pub(crate) const DEFAULT_AGENT_WAIT_TIMEOUT_MS: u64 = 120_000;
pub(crate) const DEFAULT_AGENT_READ_LINES: usize = 80;
pub(crate) const AUTOMATION_RESPONSE_TIMEOUT: Duration = Duration::from_secs(330);
pub const AUTOMATION_QUEUE_CAPACITY: usize = 32;
pub(crate) const AUTOMATION_MAX_CLIENT_THREADS: usize = 8;

fn default_agent_start_timeout_ms() -> u64 {
    DEFAULT_AGENT_START_TIMEOUT_MS
}

fn default_agent_wait_timeout_ms() -> u64 {
    DEFAULT_AGENT_WAIT_TIMEOUT_MS
}

fn default_agent_read_lines() -> usize {
    DEFAULT_AGENT_READ_LINES
}

fn default_wait_true() -> bool {
    true
}

fn default_split_direction() -> AutomationDirection {
    AutomationDirection::Right
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentAttention {
    Permission,
    Question,
    Plan,
    Notification,
}

impl AgentAttention {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "permission" => Some(Self::Permission),
            "question" => Some(Self::Question),
            "plan" => Some(Self::Plan),
            "notification" => Some(Self::Notification),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Plan => "plan",
            Self::Notification => "notification",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum AutomationCommand {
    List,
    Send {
        text: String,
        #[serde(default)]
        newline: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane: Option<Uuid>,
    },
    Split {
        direction: AutomationDirection,
        #[serde(default)]
        no_focus: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
    },
    CreateTab {
        #[serde(default)]
        no_focus: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
    },
    Focus {
        direction: AutomationDirection,
    },
    Close,
    Zoom,
    AgentStatus {
        /// Pane UUID or agent name. Defaults to the calling pane.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    AgentList,
    AgentKinds,
    AgentStart {
        kind: AgentKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default = "default_agent_start_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "default_wait_true")]
        wait: bool,
        #[serde(default)]
        args: Vec<String>,
    },
    AgentOpen {
        kind: AgentKind,
        #[serde(default)]
        placement: AgentPlacement,
        #[serde(default = "default_split_direction")]
        direction: AutomationDirection,
        #[serde(default)]
        no_focus: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(default = "default_agent_start_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "default_wait_true")]
        wait: bool,
        #[serde(default)]
        args: Vec<String>,
    },
    AgentPrompt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        text: String,
        #[serde(default = "default_wait_true")]
        wait: bool,
        #[serde(default = "default_agent_wait_timeout_ms")]
        timeout_ms: u64,
        /// Empty means settled: idle or waiting.
        #[serde(default)]
        until: Vec<AgentRuntimeState>,
    },
    AgentWait {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default = "default_agent_wait_timeout_ms")]
        timeout_ms: u64,
        /// Empty means settled: idle or waiting.
        #[serde(default)]
        until: Vec<AgentRuntimeState>,
    },
    AgentRead {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default = "default_agent_read_lines")]
        lines: usize,
    },
    AgentRename {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        /// `None` clears the name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default)]
        clear: bool,
    },
    SetAgentState {
        state: AgentRuntimeState,
    },
    SetAgentPresence {
        kind: AgentKind,
        state: AgentRuntimeState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attention: Option<AgentAttention>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    ClearAgentPresence {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationEnvelope {
    pub pane_id: Uuid,
    pub token: Uuid,
    pub command: AutomationCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AutomationResponse {
    pub fn success(data: impl Into<Value>) -> Self {
        Self {
            ok: true,
            data: Some(data.into()),
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error.into()),
        }
    }
}

pub struct AutomationIncoming {
    pub envelope: AutomationEnvelope,
    pub response: mpsc::Sender<AutomationResponse>,
}
