//! Plaintext application messages carried *inside* the authenticated encrypted
//! channel. This crate performs no pairing, authentication, or network I/O.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const VERSION: u16 = 1;
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_PANES: usize = 128;
pub const MAX_HISTORY_LINES: u16 = 1000;
pub const MAX_PENDING_MESSAGES: usize = 8;
pub const HEARTBEAT_TIMEOUT_SECONDS: u64 = 15;
pub const MAX_SCREEN_HZ: u8 = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u16,
    pub request_id: u64,
    pub message: Message,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    ListPanes {},
    Panes {
        panes: Vec<Pane>,
    },
    Open {
        pane_id: Uuid,
        size: Size,
    },
    Close {
        pane_id: Uuid,
    },
    Resize {
        pane_id: Uuid,
        size: Size,
    },
    Input {
        pane_id: Uuid,
        input: Input,
    },
    Screen {
        pane_id: Uuid,
        revision: u64,
        size: Size,
        ansi: String,
    },
    Patch {
        pane_id: Uuid,
        base_revision: u64,
        revision: u64,
        ansi: String,
    },
    History {
        pane_id: Uuid,
        lines: u16,
    },
    HistoryResult {
        pane_id: Uuid,
        text: String,
    },
    Resync {
        pane_id: Uuid,
    },
    ControlReleased {
        pane_id: Uuid,
        reason: ReleaseReason,
    },
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    Error {
        code: ErrorCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pane {
    pub id: Uuid,
    pub title: String,
    pub size: Size,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Size {
    pub columns: u16,
    pub rows: u16,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Text { text: String },
    Key { key: Key, modifiers: Vec<Modifier> },
    Paste { text: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Escape,
    Tab,
    Enter,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Character(char),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    Shift,
    Control,
    Alt,
    Super,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
    Closed,
    Disconnected,
    Backgrounded,
    Revoked,
    Reclaimed,
    Expired,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedVersion,
    InvalidMessage,
    NotShared,
    NotController,
    Unavailable,
    ResyncRequired,
    RateLimited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    TooLarge,
    InvalidJson,
    UnsupportedVersion,
    InvalidValue,
}
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProtocolError {}

impl Size {
    fn valid(self) -> bool {
        (1..=500).contains(&self.columns) && (1..=300).contains(&self.rows)
    }
}
impl Envelope {
    pub fn new(request_id: u64, message: Message) -> Self {
        Self {
            version: VERSION,
            request_id,
            message,
        }
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(ProtocolError::TooLarge);
        }
        let message: Self =
            serde_json::from_slice(bytes).map_err(|_| ProtocolError::InvalidJson)?;
        message.validate()?;
        Ok(message)
    }
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| ProtocolError::InvalidJson)?;
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(ProtocolError::TooLarge);
        }
        Ok(bytes)
    }
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != VERSION {
            return Err(ProtocolError::UnsupportedVersion);
        }
        let valid = match &self.message {
            Message::Panes { panes } => {
                panes.len() <= MAX_PANES
                    && panes.iter().all(|p| p.size.valid() && p.title.len() <= 512)
                    && panes
                        .iter()
                        .enumerate()
                        .all(|(i, p)| panes[..i].iter().all(|other| other.id != p.id))
            }
            Message::Open { size, .. }
            | Message::Resize { size, .. }
            | Message::Screen { size, .. } => size.valid(),
            Message::Patch {
                base_revision,
                revision,
                ..
            } => revision > base_revision,
            Message::Input { input, .. } => match input {
                Input::Text { text } | Input::Paste { text } => {
                    !text.is_empty() && text.len() <= MAX_INPUT_BYTES
                }
                Input::Key { key, modifiers } => {
                    modifiers.len() <= 4
                        && modifiers
                            .iter()
                            .enumerate()
                            .all(|(i, m)| !modifiers[..i].contains(m))
                        && !matches!(key,Key::Character(c) if c.is_control())
                }
            },
            Message::History { lines, .. } => (1..=MAX_HISTORY_LINES).contains(lines),
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(ProtocolError::InvalidValue)
        }
    }
}

/// Tracks one pane in one authenticated connection. Create a new tracker after
/// reconnect; a patch can never bootstrap a viewport. This is not authentication
/// or a substitute for Noise's transport nonce validation.
#[derive(Debug)]
pub struct FrameTracker {
    revision: Option<u64>,
    needs_full: bool,
}
impl Default for FrameTracker {
    fn default() -> Self {
        Self {
            revision: None,
            needs_full: true,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum FrameDecision {
    Apply,
    IgnoreStale,
    RequestFull,
}
impl FrameTracker {
    pub fn full(&mut self, revision: u64) -> FrameDecision {
        if self
            .revision
            .is_some_and(|old| revision < old || (revision == old && !self.needs_full))
        {
            return FrameDecision::IgnoreStale;
        }
        self.needs_full = false;
        self.revision = Some(revision);
        FrameDecision::Apply
    }
    pub fn patch(&mut self, base: u64, revision: u64) -> FrameDecision {
        if self.revision.is_some_and(|old| revision <= old) {
            return FrameDecision::IgnoreStale;
        }
        if self.needs_full || revision <= base || self.revision != Some(base) {
            self.needs_full = true;
            return FrameDecision::RequestFull;
        }
        self.revision = Some(revision);
        FrameDecision::Apply
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_preserves_unicode_and_control_sequences() {
        let frame = Envelope::new(
            u64::MAX,
            Message::Screen {
                pane_id: Uuid::nil(),
                revision: 7,
                size: Size {
                    columns: 80,
                    rows: 24,
                },
                ansi: "\x1b[31mEspañol 日本語 🦀\x1b[0m".into(),
            },
        );
        assert_eq!(Envelope::decode(&frame.encode().unwrap()).unwrap(), frame);
    }
    #[test]
    fn rejects_unknown_version_and_fields() {
        assert_eq!(
            Envelope::decode(br#"{"version":2,"request_id":0,"message":{"kind":"list_panes"}}"#),
            Err(ProtocolError::UnsupportedVersion)
        );
        assert_eq!(
            Envelope::decode(
                br#"{"version":1,"request_id":0,"message":{"kind":"list_panes","execute":"oops"}}"#
            ),
            Err(ProtocolError::InvalidJson)
        );
    }
    #[test]
    fn rejects_limits_and_ambiguous_input() {
        let input = Message::Input {
            pane_id: Uuid::nil(),
            input: Input::Key {
                key: Key::Character('c'),
                modifiers: vec![Modifier::Control, Modifier::Control],
            },
        };
        assert!(Envelope::new(1, input).encode().is_err());
        assert!(
            Envelope::new(
                1,
                Message::Open {
                    pane_id: Uuid::nil(),
                    size: Size {
                        columns: 0,
                        rows: 24
                    }
                }
            )
            .encode()
            .is_err()
        );
        assert!(
            Envelope::new(
                1,
                Message::History {
                    pane_id: Uuid::nil(),
                    lines: 1001
                }
            )
            .encode()
            .is_err()
        );
        assert_eq!(
            Envelope::decode(&vec![b' '; MAX_MESSAGE_BYTES + 1]),
            Err(ProtocolError::TooLarge)
        );
    }
    #[test]
    fn resynchronizes_gaps_without_applying_or_replaying_frames() {
        let mut frames = FrameTracker::default();
        assert_eq!(frames.patch(1, 2), FrameDecision::RequestFull);
        assert_eq!(frames.full(10), FrameDecision::Apply);
        assert_eq!(frames.patch(10, 11), FrameDecision::Apply);
        assert_eq!(frames.patch(10, 11), FrameDecision::IgnoreStale);
        assert_eq!(frames.patch(12, 13), FrameDecision::RequestFull);
        assert_eq!(frames.patch(11, 12), FrameDecision::RequestFull);
        assert_eq!(frames.full(14), FrameDecision::Apply);
        assert_eq!(frames.full(9), FrameDecision::IgnoreStale);
        assert_eq!(frames.patch(14, 15), FrameDecision::Apply);
    }
}
