//! Shared agent identity and activity types.
//!
//! Detection (process name, title, screen) and automation hooks both speak this
//! vocabulary so a new agent cannot land in only one of those tables.

use std::path::Path;

use serde::{Deserialize, Serialize};

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

    /// Process-name scan order. Longer / more specific names stay first so a
    /// wrapper like `cursor-agent` is not classified as a generic `cursor` miss.
    const PROCESS_SCAN_ORDER: [Self; 10] = [
        Self::OpenCode,
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::Goose,
        Self::Grok,
        Self::Aider,
        Self::Amp,
        Self::Pi,
        Self::Cursor,
    ];

    /// Screen/title scan order. Distinctive phrases are preferred over short
    /// tokens that appear in unrelated output.
    const TEXT_SCAN_ORDER: [Self; 10] = Self::PROCESS_SCAN_ORDER;

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

    fn process_aliases(self) -> &'static [&'static str] {
        match self {
            Self::Aider => &["aider"],
            Self::Amp => &["amp"],
            Self::Claude => &["claude"],
            Self::Codex => &["codex"],
            Self::Cursor => &["cursor-agent", "cursor"],
            Self::Gemini => &["gemini"],
            Self::Goose => &["goose"],
            Self::Grok => &["grok"],
            Self::OpenCode => &["opencode"],
            Self::Pi => &["pi"],
        }
    }

    fn text_markers(self) -> &'static [&'static str] {
        match self {
            Self::OpenCode => &["opencode"],
            Self::Claude => &["claude code", "claude"],
            Self::Codex => &["openai codex", "codex"],
            Self::Gemini => &["gemini cli", "gemini"],
            Self::Goose => &["goose session", "block goose", "goose"],
            Self::Grok => &["grok cli", "grok"],
            Self::Cursor => &["cursor agent"],
            Self::Aider => &["aider"],
            Self::Amp => &["sourcegraph amp", "amp thread"],
            Self::Pi => &["pi coding agent", "pi agent"],
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

    pub fn from_process_name(process_name: &str) -> Option<Self> {
        let base = Path::new(process_name)
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|| process_name.to_ascii_lowercase());
        Self::PROCESS_SCAN_ORDER.into_iter().find(|kind| {
            kind.process_aliases()
                .iter()
                .any(|alias| process_name_matches(&base, alias))
        })
    }

    pub fn from_text(text: &str) -> Option<Self> {
        let text = text.to_lowercase();
        Self::TEXT_SCAN_ORDER.into_iter().find(|kind| {
            kind.text_markers()
                .iter()
                .any(|marker| text.contains(marker))
        })
    }
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

fn process_name_matches(process_name: &str, agent_name: &str) -> bool {
    process_name == agent_name
        || process_name
            .strip_prefix(agent_name)
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(|separator| matches!(separator, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_names_match_known_clis_and_reject_lookalikes() {
        assert_eq!(
            AgentKind::from_process_name("/usr/local/bin/codex"),
            Some(AgentKind::Codex)
        );
        assert_eq!(
            AgentKind::from_process_name("codex-code-mode-host"),
            Some(AgentKind::Codex)
        );
        assert_eq!(
            AgentKind::from_process_name("/usr/local/bin/cursor-agent"),
            Some(AgentKind::Cursor)
        );
        assert_eq!(
            AgentKind::from_process_name("goose"),
            Some(AgentKind::Goose)
        );
        assert!(AgentKind::from_process_name("codexical").is_none());
        assert!(AgentKind::from_process_name("zsh").is_none());
    }

    #[test]
    fn text_markers_prefer_distinctive_phrases() {
        assert_eq!(
            AgentKind::from_text("Claude Code — allow?"),
            Some(AgentKind::Claude)
        );
        assert_eq!(AgentKind::from_text("OpenAI Codex"), Some(AgentKind::Codex));
        assert_eq!(
            AgentKind::from_text("cursor agent ready"),
            Some(AgentKind::Cursor)
        );
        assert!(AgentKind::from_text("amp").is_none());
        assert_eq!(AgentKind::from_text("amp thread"), Some(AgentKind::Amp));
    }
}
