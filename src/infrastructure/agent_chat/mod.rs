//! Agent chat backends (issue #36). One ACP client covers Grok, Codex and
//! Claude; each agent only contributes a launch command (`launch.rs`).
//!
//! A connection owns the agent process. The view talks to it only through
//! `ChatCommand` / `ChatEvent` channels, so a native backend (for example
//! `codex app-server`) can replace ACP later without touching the UI.

mod acp;
pub mod launch;

use std::path::PathBuf;

use crate::domain::agent_chat::{ChatAgent, ChatCommand, ChatEvent};

pub use launch::{AgentLaunch, resolve_launch};

/// Which session a new connection opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTarget {
    New,
    /// Open an existing native session with `session/load`.
    Load(String),
}

/// A running agent connection. Dropping it shuts the agent process down.
pub struct AgentChatConnection {
    pub commands: async_channel::Sender<ChatCommand>,
    pub events: async_channel::Receiver<ChatEvent>,
    pub(crate) _guard: acp::ConnectionGuard,
}

/// Spawns the agent in `cwd` and starts `initialize` + session setup in the
/// background. Errors here mean the process could not start; protocol
/// failures arrive later as `ChatEvent::Error` / `ChatEvent::Exited`.
#[cfg_attr(test, allow(dead_code))]
pub fn connect(
    agent: ChatAgent,
    cwd: PathBuf,
    target: SessionTarget,
) -> anyhow::Result<AgentChatConnection> {
    let launch = resolve_launch(agent)?;
    acp::spawn(agent, launch, cwd, target)
}
