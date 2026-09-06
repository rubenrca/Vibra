//! Authenticated session commands and screen revisions, independent of the socket.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use uuid::Uuid;
use vibra_remote::protocol::{ErrorCode, Message, ReleaseReason, Size};

use crate::ports::terminal::{RemoteFrame, TerminalHandle};

use super::{Hub, bounded_history, draw, encode_input, terminal_size};

struct ControlledPane {
    id: Uuid,
    handle: Arc<dyn TerminalHandle>,
    previous_frame: Option<RemoteFrame>,
}

#[derive(Default)]
pub(super) struct RemoteSession {
    controller: Option<ControlledPane>,
    revision: u64,
}

impl RemoteSession {
    fn controlled_pane_mut(&mut self, hub: &Hub, pane_id: Uuid) -> Option<&mut ControlledPane> {
        self.controller.as_mut().filter(|pane| {
            pane.id == pane_id && pane.handle.remote_controlled() && hub.handle(pane_id).is_ok()
        })
    }

    pub(super) fn handle(
        &mut self,
        hub: &Hub,
        generation: u64,
        message: Message,
    ) -> Result<Option<Message>> {
        let response = match message {
            Message::Ping { nonce } => Some(Message::Pong { nonce }),
            Message::Pong { .. } => None,
            Message::ListPanes {} => Some(Message::Panes { panes: hub.panes() }),
            Message::Open { pane_id, size } => match hub.handle(pane_id) {
                Ok(handle) => {
                    if let Some(previous) = self.controller.take() {
                        previous.handle.remote_release();
                    }
                    handle.remote_claim(terminal_size(size))?;
                    self.controller = Some(ControlledPane {
                        id: pane_id,
                        handle,
                        previous_frame: None,
                    });
                    hub.describe(
                        generation,
                        "iPhone controla una terminal · menú del pane para recuperar",
                    );
                    None
                }
                Err(_) => Some(Message::Error {
                    code: ErrorCode::NotShared,
                }),
            },
            Message::Close { pane_id } => {
                if self
                    .controller
                    .as_ref()
                    .is_some_and(|pane| pane.id == pane_id)
                    && let Some(pane) = self.controller.take()
                {
                    pane.handle.remote_release();
                }
                Some(Message::ControlReleased {
                    pane_id,
                    reason: ReleaseReason::Closed,
                })
            }
            Message::Resize { pane_id, size } => match self.controlled_pane_mut(hub, pane_id) {
                Some(pane) => {
                    pane.handle.remote_resize(terminal_size(size))?;
                    pane.previous_frame = None;
                    None
                }
                None => Some(Message::Error {
                    code: ErrorCode::NotController,
                }),
            },
            Message::Input { pane_id, input } => {
                match self
                    .controlled_pane_mut(hub, pane_id)
                    .filter(|pane| pane.previous_frame.is_some())
                {
                    Some(pane) => {
                        pane.handle
                            .remote_input(encode_input(input, pane.handle.input_mode()))?;
                        None
                    }
                    None => Some(Message::Error {
                        code: ErrorCode::NotController,
                    }),
                }
            }
            Message::Resync { pane_id } => {
                if let Some(pane) = &mut self.controller
                    && pane.id == pane_id
                {
                    pane.previous_frame = None;
                }
                None
            }
            Message::History { pane_id, lines } => {
                hub.handle(pane_id)
                    .ok()
                    .map(|handle| Message::HistoryResult {
                        pane_id,
                        text: bounded_history(
                            handle.recent_text(lines as usize).unwrap_or_default(),
                        ),
                    })
            }
            _ => Some(Message::Error {
                code: ErrorCode::InvalidMessage,
            }),
        };
        Ok(response)
    }

    pub(super) fn screen_update(&mut self, hub: &Hub) -> Result<Option<Message>> {
        let Some(pane) = &mut self.controller else {
            return Ok(None);
        };
        if !pane.handle.remote_controlled() || hub.handle(pane.id).is_err() {
            pane.handle.remote_release();
            let message = Message::ControlReleased {
                pane_id: pane.id,
                reason: ReleaseReason::Reclaimed,
            };
            self.controller = None;
            return Ok(Some(message));
        }
        let frame = pane.handle.remote_frame()?;
        if pane.previous_frame.as_ref() == Some(&frame) {
            return Ok(None);
        }
        let full = pane.previous_frame.as_ref().is_none_or(|previous| {
            previous.columns != frame.columns || previous.rows != frame.rows
        });
        let ansi = draw(
            &frame,
            if full {
                None
            } else {
                pane.previous_frame.as_ref()
            },
        );
        let base_revision = self.revision;
        self.revision += 1;
        let message = if full {
            Message::Screen {
                pane_id: pane.id,
                revision: self.revision,
                size: Size {
                    columns: frame.columns,
                    rows: frame.rows,
                },
                ansi,
            }
        } else {
            Message::Patch {
                pane_id: pane.id,
                base_revision,
                revision: self.revision,
                ansi,
            }
        };
        pane.previous_frame = Some(frame);
        Ok(Some(message))
    }
}

/// Count complete authenticated envelopes, not individual encrypted fragments.
pub(super) struct RequestGuard {
    last_request: u64,
    window_started: Instant,
    bytes: usize,
    requests: usize,
}

impl RequestGuard {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            last_request: 0,
            window_started: now,
            bytes: 0,
            requests: 0,
        }
    }

    pub(super) fn accept(&mut self, request_id: u64, bytes: usize, now: Instant) -> Result<()> {
        ensure!(request_id > self.last_request, "replayed request");
        self.last_request = request_id;
        if now.duration_since(self.window_started) > Duration::from_secs(1) {
            self.window_started = now;
            self.bytes = 0;
            self.requests = 0;
        }
        self.bytes += bytes;
        self.requests += 1;
        ensure!(
            self.bytes <= 256 * 1024 && self.requests <= 200,
            "input rate exceeded"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_must_increase_across_rate_windows() {
        let now = Instant::now();
        let mut guard = RequestGuard::new(now);
        assert!(guard.accept(0, 1, now).is_err());
        assert!(guard.accept(2, 1, now).is_ok());
        assert!(guard.accept(2, 1, now).is_err());
        assert!(guard.accept(1, 1, now + Duration::from_secs(2)).is_err());
        assert!(guard.accept(3, 1, now + Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn byte_limit_is_inclusive_and_resets_after_one_second() {
        let now = Instant::now();
        let mut guard = RequestGuard::new(now);
        assert!(guard.accept(1, 256 * 1024, now).is_ok());
        assert!(guard.accept(2, 1, now + Duration::from_secs(1)).is_err());
        assert!(
            guard
                .accept(3, 256 * 1024, now + Duration::from_secs(2))
                .is_ok()
        );
    }

    #[test]
    fn request_count_is_limited_independently_of_bytes() {
        let now = Instant::now();
        let mut guard = RequestGuard::new(now);
        for id in 1..=200 {
            assert!(guard.accept(id, 0, now).is_ok());
        }
        assert!(guard.accept(201, 0, now).is_err());
        assert!(guard.accept(202, 0, now + Duration::from_secs(2)).is_ok());
    }
}
