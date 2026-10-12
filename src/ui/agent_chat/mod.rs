//! Native chat view for a pane (issue #36). It renders a `ChatTimeline` and
//! sends `ChatCommand`s; it never talks to the agent process directly.

mod composer;
mod diff;
mod layout;
mod timeline;
mod tool_card;
mod tooltip;
mod turns;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_channel::{Receiver, Sender, TrySendError};
use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, Window, prelude::*, px,
};

use crate::domain::agent_chat::{
    AgentModelCatalog, CatalogModel, ChatAgent, ChatCommand, ChatConnectionState, ChatEvent,
    ChatItem, ChatTimeline, ConfigCategory, ConfigOption, PromptFile, TurnOutcome,
};
use crate::infrastructure::agent_chat::AgentChatConnection;
use crate::ports::files::FileSystemPort;
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
    /// The agent named the session; the pane's tab shows it.
    TitleChanged(String),
    /// Before the first message the user picked another agent's model; the
    /// workspace restarts the chat with `agent` and applies `model` if given.
    SwitchAgent {
        agent: ChatAgent,
        model: Option<String>,
    },
    /// The running agent's model list, for the workspace to remember.
    ModelsKnown {
        agent: ChatAgent,
        models: Vec<CatalogModel>,
    },
}

/// Workspace facts the composer shows; the workspace refreshes them with
/// `AgentChatView::set_context`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatViewContext {
    /// Branch checked out in the pane's working directory.
    pub branch: Option<String>,
    /// Whether the working directory is a linked Git worktree.
    pub worktree: bool,
    /// The agent provider's quota windows, shortest first (for example 5 h, then weekly).
    pub quota: Vec<QuotaMeter>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaMeter {
    /// Short window name, for example "5h" or "Week".
    pub label: String,
    /// 0.0 … 100.0.
    pub used_percent: f32,
    /// Human text such as "3h 52m"; `None` when unknown.
    pub resets_in: Option<String>,
}

/// Where `@` references search for files.
#[derive(Clone)]
pub struct FileSearch {
    pub port: Arc<dyn FileSystemPort>,
    pub root: PathBuf,
}

pub struct AgentChatView {
    agent: ChatAgent,
    timeline: ChatTimeline,
    focus_handle: FocusHandle,
    list_state: ListState,
    /// Composer text. It is kept while the composer is locked.
    draft: String,
    /// Expanded reasoning and tool cards, and unfolded turns (`turns::turn_key`),
    /// by a key that outlives list rows.
    expanded: HashSet<String>,
    /// What each list row draws; rebuilt by `sync_list`.
    rows: Vec<turns::Row>,
    /// Prompts written while a turn runs; the first one is sent when it ends.
    queued: Vec<composer::QueuedPrompt>,
    /// Sends commands to the agent. `None` once the session has ended.
    commands: Option<Sender<ChatCommand>>,
    /// Owns the agent process. Dropping it ends the process.
    connection: Option<AgentChatConnection>,
    /// Forwards backend events into the timeline. Dropping it stops the forwarding.
    events_task: Option<gpui::Task<()>>,
    /// Redraws once a second while a turn runs, for "Working for …".
    tick_task: Option<gpui::Task<()>>,
    context: ChatViewContext,
    file_search: Option<FileSearch>,
    /// Files referenced with `@` for the next prompt.
    attachments: Vec<PromptFile>,
    /// Popups and selection state owned by `composer.rs`.
    composer: composer::ComposerState,
    /// Remembered models of every agent, for the picker's provider tabs.
    model_catalog: Vec<AgentModelCatalog>,
    /// Agents the user can switch to before the first message.
    available_agents: Vec<ChatAgent>,
    /// Model to select once the agent reports its options.
    pending_model: Option<String>,
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

    /// Updates the branch, worktree and quota the composer shows.
    pub fn set_context(&mut self, context: ChatViewContext, cx: &mut Context<Self>) {
        if self.context != context {
            self.context = context;
            cx.notify();
        }
    }

    /// Sets the remembered models and the agents the picker may offer.
    pub fn set_model_catalog(
        &mut self,
        catalog: Vec<AgentModelCatalog>,
        available_agents: Vec<ChatAgent>,
        cx: &mut Context<Self>,
    ) {
        self.model_catalog = catalog;
        self.available_agents = available_agents;
        cx.notify();
    }

    /// Selects `model` as soon as the agent reports its model option.
    pub fn apply_model_on_start(&mut self, model: String) {
        self.pending_model = Some(model);
    }

    /// The agent is fixed once the conversation has a message.
    pub fn agent_locked(&self) -> bool {
        self.has_conversation()
    }

    /// The model picker chose `model` of `agent`. The running agent changes
    /// its option; another agent restarts the chat, before the first message only.
    fn choose_model(&mut self, agent: ChatAgent, model: String, cx: &mut Context<Self>) {
        if agent == self.agent {
            if let Some(option) = model_option(&self.timeline.config_options) {
                let option_id = option.id.clone();
                self.send(
                    ChatCommand::SetConfigOption {
                        option_id,
                        value: model,
                    },
                    cx,
                );
            }
        } else if !self.agent_locked() {
            cx.emit(AgentChatViewEvent::SwitchAgent {
                agent,
                model: Some(model),
            });
        }
    }

    /// Sets where `@` references look for files; `None` disables them.
    pub fn set_file_search(&mut self, file_search: Option<FileSearch>) {
        self.file_search = file_search;
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
            rows: Vec::new(),
            queued: Vec::new(),
            commands: None,
            connection: None,
            events_task: None,
            tick_task: None,
            context: ChatViewContext::default(),
            file_search: None,
            attachments: Vec::new(),
            composer: composer::ComposerState::default(),
            model_catalog: Vec::new(),
            available_agents: Vec::new(),
            pending_model: None,
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
        let was_working = self.is_working();
        let session_started = match &event {
            ChatEvent::SessionStarted { session_id, .. } => Some(session_id.clone()),
            _ => None,
        };
        let exited = matches!(event, ChatEvent::Exited { .. });
        let title = match &event {
            ChatEvent::Title(title) => Some(title.clone()),
            _ => None,
        };
        let options_changed = matches!(event, ChatEvent::ConfigOptions(_));
        self.timeline.apply(event);
        self.timeline.stamp(SystemTime::now());
        if let Some(title) = title {
            cx.emit(AgentChatViewEvent::TitleChanged(title));
        }
        if options_changed {
            self.on_config_options(cx);
        }
        if !self.is_working() {
            self.tick_task = None;
        }
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
        // A finished turn hands over to the next queued prompt. A stopped turn
        // holds the queue, since the user asked the agent to stop; the queue
        // moves again after the next turn. A closed session keeps the queue.
        let stopped = self
            .timeline
            .turns
            .last()
            .is_some_and(|turn| turn.outcome == TurnOutcome::Cancelled);
        if was_working && self.is_idle() && !stopped {
            self.send_next_queued(cx);
        }
        cx.notify();
    }

    /// Remembers the agent's models and applies a model chosen before it started.
    fn on_config_options(&mut self, cx: &mut Context<Self>) {
        let Some(option) = model_option(&self.timeline.config_options) else {
            return;
        };
        let models = option
            .values
            .iter()
            .map(|value| CatalogModel {
                value: value.value.clone(),
                name: value.name.clone(),
            })
            .collect();
        let option_id = option.id.clone();
        let current = option.current.clone();
        cx.emit(AgentChatViewEvent::ModelsKnown {
            agent: self.agent,
            models,
        });
        if let Some(model) = self.pending_model.take()
            && model != current
        {
            self.send(
                ChatCommand::SetConfigOption {
                    option_id,
                    value: model,
                },
                cx,
            );
        }
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

    /// Enter or the action button: sends the composer text as a prompt, or
    /// queues it while a turn runs. The draft and its attachments move either way.
    fn submit_draft(&mut self, cx: &mut Context<Self>) {
        let text = self.draft.trim().to_owned();
        if text.is_empty() || !self.accepts_input() {
            return;
        }
        self.draft.clear();
        let files = std::mem::take(&mut self.attachments);
        if self.is_working() {
            self.queued.push(composer::QueuedPrompt { text, files });
            cx.notify();
        } else {
            self.send_prompt(text, files, cx);
        }
    }

    /// Echoes a prompt as a new turn and sends it to the agent.
    fn send_prompt(&mut self, text: String, files: Vec<PromptFile>, cx: &mut Context<Self>) {
        let labels = files.iter().map(|file| file.label.clone()).collect();
        self.timeline.push_user_prompt(text.clone(), labels);
        self.timeline.stamp(SystemTime::now());
        // A prompt means the reader wants the newest message, so pin to the bottom.
        self.rows = turns::plan_rows(&self.timeline, &self.expanded);
        self.list_state.reset(self.timeline.items.len());
        self.send(ChatCommand::Prompt { text, files }, cx);
        self.start_turn_clock(cx);
        cx.notify();
    }

    /// Sends the first queued prompt; the rest wait for the next turn to end.
    fn send_next_queued(&mut self, cx: &mut Context<Self>) {
        if self.queued.is_empty() {
            return;
        }
        let next = self.queued.remove(0);
        self.send_prompt(next.text, next.files, cx);
    }

    fn start_turn_clock(&mut self, cx: &mut Context<Self>) {
        self.tick_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let working = this
                    .update(cx, |view, cx| {
                        cx.notify();
                        view.is_working()
                    })
                    .unwrap_or(false);
                if !working {
                    return;
                }
            }
        }));
    }

    fn cancel_turn(&mut self, cx: &mut Context<Self>) {
        if self.is_working() {
            self.send(ChatCommand::Cancel, cx);
        }
    }

    fn is_working(&self) -> bool {
        self.timeline.is_working()
    }

    /// The session is open and no turn runs. The model and mode menus need this.
    fn is_idle(&self) -> bool {
        matches!(self.timeline.state, Some(ChatConnectionState::Ready))
    }

    fn is_closed(&self) -> bool {
        matches!(
            self.timeline.state,
            Some(ChatConnectionState::Closed { .. })
        )
    }
}

/// The option that picks the agent's model, if it has one.
fn model_option(options: &[ConfigOption]) -> Option<&ConfigOption> {
    options
        .iter()
        .find(|option| option.category == ConfigCategory::Model)
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
    use gpui::{EntityInputHandler, KeyDownEvent, Keystroke, Modifiers, TestAppContext};

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
            view.replace_text_in_range(None, "h", window, cx);
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
            // Typed text arrives through the input handler, as on macOS.
            view.replace_text_in_range(None, "hi", window, cx);
            view.on_key_down(&key("enter", None), window, cx);
        });
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(ChatCommand::Prompt { text, .. }) if text == "hi"
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

    /// Puts `text` in the draft and presses Enter, as the composer does.
    fn enter_text(view: &mut AgentChatView, text: &str, cx: &mut Context<AgentChatView>) {
        view.draft = text.into();
        view.submit_draft(cx);
    }

    #[gpui::test]
    fn finished_turn_sends_the_first_queued_prompt(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (events_tx, events_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Ready);
            view.commands = Some(commands_tx);
            view.start_events(events_rx, cx);
            view
        });
        view.update_in(cx, |view, _, cx| {
            enter_text(view, "first", cx);
            enter_text(view, "second", cx);
            enter_text(view, "third", cx);
        });
        // Only the first prompt goes out while its turn runs.
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(ChatCommand::Prompt { text, .. }) if text == "first"
        ));
        assert!(commands_rx.try_recv().is_err());
        events_tx
            .try_send(ChatEvent::TurnFinished {
                stop_reason: "end_turn".into(),
            })
            .unwrap();
        cx.run_until_parked();
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(ChatCommand::Prompt { text, files }) if text == "second" && files.is_empty()
        ));
        view.read_with(cx, |view, _| {
            assert!(view.is_working());
            assert_eq!(
                view.queued,
                vec![composer::QueuedPrompt {
                    text: "third".into(),
                    files: Vec::new(),
                }]
            );
            assert_eq!(view.timeline.items.len(), 2);
        });
    }

    #[gpui::test]
    fn stopping_a_turn_holds_the_queue(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (events_tx, events_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Ready);
            view.commands = Some(commands_tx);
            view.start_events(events_rx, cx);
            view
        });
        view.update_in(cx, |view, _, cx| {
            enter_text(view, "first", cx);
            enter_text(view, "later", cx);
        });
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(ChatCommand::Prompt { text, .. }) if text == "first"
        ));
        events_tx
            .try_send(ChatEvent::TurnFinished {
                stop_reason: "cancelled".into(),
            })
            .unwrap();
        cx.run_until_parked();
        assert!(commands_rx.try_recv().is_err());
        view.read_with(cx, |view, _| {
            assert!(!view.is_working());
            assert_eq!(view.queued.len(), 1);
        });
    }

    #[gpui::test]
    fn closing_the_session_keeps_queued_prompts_unsent(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (events_tx, events_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Ready);
            view.commands = Some(commands_tx);
            view.start_events(events_rx, cx);
            view
        });
        view.update_in(cx, |view, _, cx| {
            enter_text(view, "first", cx);
            enter_text(view, "later", cx);
        });
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(ChatCommand::Prompt { text, .. }) if text == "first"
        ));
        events_tx
            .try_send(ChatEvent::Exited { message: None })
            .unwrap();
        cx.run_until_parked();
        // The process is gone, so the queued prompt has no channel to go out on.
        assert!(commands_rx.try_recv().is_err());
        view.read_with(cx, |view, _| {
            assert!(view.is_closed());
            assert_eq!(view.queued.len(), 1);
        });
    }
}
