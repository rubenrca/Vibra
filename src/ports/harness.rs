#![allow(dead_code)]

use std::path::PathBuf;

use crate::domain::workspace::HostedAgentKind;

/// Official vendor wire only. The host never invents policy, prompts, or tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessSpawn {
    pub agent: HostedAgentKind,
    pub working_directory: PathBuf,
    pub vendor_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessPermissionRequest {
    pub id: String,
    pub summary: String,
    pub options: Vec<HarnessPermissionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessPermissionOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HarnessEvent {
    Text(String),
    Reasoning(String),
    ToolCall { id: String, summary: String },
    ToolResult { id: String, output: Option<String> },
    PermissionRequest(HarnessPermissionRequest),
    Question { id: String, prompt: String },
    Commands(Vec<String>),
    Done { vendor_session_id: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessError {
    KindNotHosted,
    Protocol(String),
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KindNotHosted => {
                formatter.write_str("no hay superficie alojada para este agente")
            }
            Self::Protocol(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for HarnessError {}

/// Viewport over a first-party agent protocol. Implementations must not add
/// policy flags or rewrite the user turn.
pub trait HarnessPort: Send + Sync {
    fn spawn_argv(&self, request: &HarnessSpawn) -> Result<Vec<String>, HarnessError>;
}
