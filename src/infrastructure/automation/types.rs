use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub use crate::domain::agents::{AgentAttention, AgentKind, AgentRuntimeState};

pub(crate) const MAX_AUTOMATION_REQUEST_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_AUTOMATION_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
pub(crate) const MAX_AGENT_HOOK_BYTES: u64 = 1024 * 1024;
pub(crate) const AUTOMATION_IO_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) static NEXT_AUTOMATION_SERVER_ID: AtomicU64 = AtomicU64::new(1);

pub const AUTOMATION_QUEUE_CAPACITY: usize = 32;
pub(crate) const AUTOMATION_MAX_CLIENT_THREADS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum AutomationCommand {
    SetAgentState {
        state: AgentRuntimeState,
    },
    SetAgentPresence {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_title: Option<String>,
        kind: AgentKind,
        state: AgentRuntimeState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attention: Option<AgentAttention>,
        /// Model selected by the CLI when it exposes one. This remains optional:
        /// Vibra must not guess a provider's default model from screen text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
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
