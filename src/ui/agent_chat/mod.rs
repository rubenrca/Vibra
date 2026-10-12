//! Native chat view for a pane (issue #36). It renders a `ChatTimeline` and
//! sends `ChatCommand`s; it never talks to the agent process directly.

mod composer;
mod diff;
mod layout;
mod timeline;
mod tool_card;
mod tooltip;

use std::collections::HashSet;

use async_channel::{Receiver, Sender, TrySendError};
use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, Window, prelude::*, px,
};

use crate::domain::agent_chat::{
    ChatAgent, ChatCommand, ChatConnectionState, ChatEvent, ChatItem, ChatTimeline,
};
use crate::infrastructure::agent_chat::AgentChatConnection;
use timeline::dirty_indices;

/// Events the workspace reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentChatViewEvent {
    /// A session id became known (new or loaded); persist the binding.
    SessionBound {
        agent: ChatAgent,
        session_id: String,
    },
    /// The user asked to continue this session in the terminal.
    SwitchToTerminal,
    /// The connection closed (process exit or failure).
    Closed,
}

pub struct AgentChatView {
    agent: ChatAgent,
    timeline: ChatTimeline,
    focus_handle: FocusHandle,
    list_state: ListState,
    /// Composer text. It is kept while the composer is locked.
    draft: String,
    /// Expanded reasoning and tool cards, by a key that outlives list rows.
    expanded: HashSet<String>,
    /// Sends commands to the agent. `None` once the session has ended.
    commands: Option<Sender<ChatCommand>>,
    /// Owns the agent process. Dropping it ends the process.
    connection: Option<AgentChatConnection>,
    /// Forwards backend events into the timeline. Dropping it stops the forwarding.
    events_task: Option<gpui::Task<()>>,
}

impl AgentChatView {
    /// Starts showing `connection`'s events. `Err` shows the message as a
    /// closed session (for example a missing adapter) without a running agent.
    pub fn new(
        agent: ChatAgent,
        connection: Result<AgentChatConnection, String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::empty(agent, cx);
        match connection {
            Ok(connection) => {
                view.timeline.state = Some(ChatConnectionState::Starting);
                view.commands = Some(connection.commands.clone());
                view.start_events(connection.events.clone(), cx);
                view.connection = Some(connection);
            }
            Err(message) => {
                view.timeline.state = Some(ChatConnectionState::Closed {
                    message: Some(message),
                });
            }
        }
        view
    }

    pub fn agent(&self) -> ChatAgent {
        self.agent
    }

    pub fn session_id(&self) -> Option<&str> {
        self.timeline.session_id.as_deref()
    }

    /// Whether the session has a message, so the agent has saved it where a
    /// terminal resume can find it.
    pub fn has_conversation(&self) -> bool {
        self.timeline
            .items
            .iter()
            .any(|item| matches!(item, ChatItem::UserMessage { .. }))
    }

    /// Sends `Shutdown` and drops the connection; used before a handoff.
    pub fn shutdown(&mut self, cx: &mut Context<Self>) {
        if let Some(commands) = self.commands.take() {
            // Unbounded in practice; if the send fails, dropping the connection still ends the process.
            let _ = commands.try_send(ChatCommand::Shutdown);
        }
        self.events_task = None;
        if let Some(connection) = self.connection.take() {
            // Give the agent a moment to close its session before the guard
            // ends the process; neither wait may block the UI thread.
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while std::time::Instant::now() < deadline {
                    match connection.events.try_recv() {
                        Err(async_channel::TryRecvError::Closed) => break,
                        Ok(_) => {}
                        Err(async_channel::TryRecvError::Empty) => {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                    }
                }
                drop(connection);
            });
        }
        if !self.is_closed() {
            self.timeline.state = Some(ChatConnectionState::Closed { message: None });
        }
        cx.notify();
    }

    fn empty(agent: ChatAgent, cx: &mut Context<Self>) -> Self {
        Self {
            agent,
            timeline: ChatTimeline::default(),
            focus_handle: cx.focus_handle(),
            list_state: ListState::new(0, ListAlignment::Bottom, px(400.0)),
            draft: String::new(),
            expanded: HashSet::new(),
            commands: None,
            connection: None,
            events_task: None,
        }
    }

    fn start_events(&mut self, events: Receiver<ChatEvent>, cx: &mut Context<Self>) {
        self.events_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |view, cx| view.on_event(event, cx))
                    .is_err()
                {
                    // The view is gone; nothing left to update.
                    return;
                }
            }
            // The backend dropped its sender without `Exited`.
            let _ = this.update(cx, |view, cx| view.on_events_closed(cx));
        }));
    }

    fn on_event(&mut self, event: ChatEvent, cx: &mut Context<Self>) {
        // Computed before the reducer runs, because the reducer may change items in place.
        let count = self.timeline.items.len();
        let dirty = dirty_indices(&self.timeline, &event);
        let was_closed = self.is_closed();
        let session_started = match &event {
            ChatEvent::SessionStarted { session_id, .. } => Some(session_id.clone()),
            _ => None,
        };
        let exited = matches!(event, ChatEvent::Exited { .. });
        self.timeline.apply(event);
        self.sync_list(count, &dirty);
        if let Some(session_id) = session_started {
            cx.emit(AgentChatViewEvent::SessionBound {
                agent: self.agent,
                session_id,
            });
        }
        if exited && !was_closed {
            self.commands = None;
            cx.emit(AgentChatViewEvent::Closed);
        }
        cx.notify();
    }

    /// The backend stopped without saying why; treat it as an exit.
    fn on_events_closed(&mut self, cx: &mut Context<Self>) {
        if self.is_closed() {
            return;
        }
        self.on_event(ChatEvent::Exited { message: None }, cx);
    }

    /// Sends a command without blocking the UI thread.
    fn send(&self, command: ChatCommand, cx: &mut Context<Self>) {
        let Some(commands) = self.commands.as_ref() else {
            return;
        };
        match commands.try_send(command) {
            Ok(()) | Err(TrySendError::Closed(_)) => {}
            Err(TrySendError::Full(command)) => {
                let commands = commands.clone();
                cx.background_spawn(async move {
                    let _ = commands.send(command).await;
                })
                .detach();
            }
        }
    }

    /// Sends the composer text as a prompt. Enter in the composer calls this.
    fn submit_draft(&mut self, cx: &mut Context<Self>) {
        let text = self.draft.trim().to_owned();
        if text.is_empty() || !self.accepts_input() {
            return;
        }
        self.draft.clear();
        self.timeline.push_user_prompt(text.clone());
        // A prompt means the reader wants the newest message, so pin to the bottom.
        self.list_state.reset(self.timeline.items.len());
        self.send(ChatCommand::Prompt { text }, cx);
        cx.notify();
    }

    fn cancel_turn(&mut self, cx: &mut Context<Self>) {
        if self.is_working() {
            self.send(ChatCommand::Cancel, cx);
        }
    }

    fn is_working(&self) -> bool {
        self.timeline.is_working()
    }

    fn is_closed(&self) -> bool {
        matches!(
            self.timeline.state,
            Some(ChatConnectionState::Closed { .. })
        )
    }
}

impl EventEmitter<AgentChatViewEvent> for AgentChatView {}

impl Focusable for AgentChatView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::ChatItem;
    use gpui::{KeyDownEvent, Keystroke, Modifiers, TestAppContext};

    fn key(key: &str, key_char: Option<&str>) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers::none(),
                key: key.into(),
                key_char: key_char.map(Into::into),
            },
            is_held: false,
        }
    }

    #[gpui::test]
    fn failed_start_shows_the_message_as_a_closed_session(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            AgentChatView::new(
                ChatAgent::Codex,
                Err("Codex is not installed.".into()),
                window,
                cx,
            )
        });
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.timeline.state,
                Some(ChatConnectionState::Closed {
                    message: Some("Codex is not installed.".into()),
                })
            );
            assert!(view.session_id().is_none());
            assert!(view.commands.is_none());
        });
    }

    #[gpui::test]
    fn closed_session_ignores_typing(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            AgentChatView::new(
                ChatAgent::Claude,
                Err("Node.js not found.".into()),
                window,
                cx,
            )
        });
        view.update_in(cx, |view, window, cx| {
            view.on_key_down(&key("h", Some("h")), window, cx);
            assert!(view.draft.is_empty());
        });
    }

    #[gpui::test]
    fn enter_sends_the_draft_and_echoes_it_as_a_working_turn(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (_events_tx, events_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Ready);
            view.commands = Some(commands_tx);
            view.start_events(events_rx, cx);
            view
        });
        view.update_in(cx, |view, window, cx| {
            view.on_key_down(&key("h", Some("h")), window, cx);
            view.on_key_down(&key("i", Some("i")), window, cx);
            view.on_key_down(&key("enter", None), window, cx);
        });
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(ChatCommand::Prompt { text }) if text == "hi"
        ));
        view.read_with(cx, |view, _| {
            assert!(view.draft.is_empty());
            assert!(view.is_working());
            assert_eq!(view.timeline.items.len(), 1);
        });
    }

    #[gpui::test]
    fn backend_events_fill_the_list_and_exit_closes_the_session(cx: &mut TestAppContext) {
        let (_commands_tx, _commands_rx) = async_channel::unbounded::<ChatCommand>();
        let (events_tx, events_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Grok, cx);
            view.timeline.state = Some(ChatConnectionState::Starting);
            view.start_events(events_rx, cx);
            view
        });
        events_tx
            .try_send(ChatEvent::SessionStarted {
                session_id: "session-1".into(),
                loaded: false,
            })
            .unwrap();
        events_tx
            .try_send(ChatEvent::AgentMessageChunk {
                text: "Hola".into(),
            })
            .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.session_id(), Some("session-1"));
            assert!(matches!(
                view.timeline.items.as_slice(),
                [ChatItem::AssistantMessage { text }] if text == "Hola"
            ));
            assert_eq!(view.list_state.item_count(), 1);
        });
        events_tx
            .try_send(ChatEvent::Exited {
                message: Some("Grok exited.".into()),
            })
            .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.timeline.state,
                Some(ChatConnectionState::Closed {
                    message: Some("Grok exited.".into()),
                })
            );
        });
    }
}
