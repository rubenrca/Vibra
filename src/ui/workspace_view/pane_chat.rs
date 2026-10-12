//! Terminal and chat views of one pane (issue #36). The pane's terminal keeps
//! running while its chat is shown. A chat connects only when no terminal agent
//! owns the same session, so each session has one owner at a time.

use std::path::PathBuf;

use gpui::{App, AppContext, Context, Entity, FocusHandle, Focusable, Subscription, Window};
use uuid::Uuid;

use crate::domain::agent_chat::{AgentChatBinding, ChatAgent, PaneViewMode};
use crate::infrastructure::agent_chat::{AgentChatConnection, SessionTarget};
use crate::ui::agent_chat::{AgentChatView, AgentChatViewEvent};

use super::WorkspaceView;

/// A pane's chat. `closed` keeps the view on screen after its connection ends,
/// so the error stays readable until the user toggles the pane again.
pub(super) struct PaneChat {
    pub(super) view: Entity<AgentChatView>,
    pub(super) closed: bool,
    _subscription: Subscription,
}

/// A resume command reaches a prompt only when no agent runs in the pane and a
/// shell owns the terminal's foreground.
pub(super) fn may_type_resume_command(agent_running: bool, foreground_is_shell: bool) -> bool {
    !agent_running && foreground_is_shell
}

#[cfg(not(test))]
fn connect_agent_chat(
    agent: ChatAgent,
    cwd: PathBuf,
    target: SessionTarget,
) -> Result<AgentChatConnection, String> {
    crate::infrastructure::agent_chat::connect(agent, cwd, target)
        .map_err(|error| format!("Could not start the chat: {error:#}"))
}

/// Unit tests never start an agent process; they take the failure path instead.
#[cfg(test)]
fn connect_agent_chat(
    agent: ChatAgent,
    cwd: PathBuf,
    target: SessionTarget,
) -> Result<AgentChatConnection, String> {
    let _ = (agent, cwd, target);
    Err("Agent chats cannot start in tests.".to_owned())
}

impl WorkspaceView {
    pub(super) fn pane_view_mode(&self, session_id: Uuid) -> PaneViewMode {
        self.snapshot
            .agent_chat_binding(session_id)
            .map_or(PaneViewMode::Terminal, |binding| binding.mode)
    }

    /// The agent a pane's chat uses: the chat-capable agent running in the
    /// terminal, else the one the pane's binding names.
    pub(super) fn pane_chat_agent(&self, session_id: Uuid) -> Option<ChatAgent> {
        self.resolved_agent_presence(session_id)
            .and_then(|presence| presence.chat_agent())
            .or_else(|| {
                self.snapshot
                    .agent_chat_binding(session_id)
                    .map(|binding| binding.agent)
            })
    }

    /// The element that receives keys for a pane in its current view.
    pub(super) fn pane_focus_handle(&self, session_id: Uuid, cx: &App) -> Option<FocusHandle> {
        match self.pane_view_mode(session_id) {
            PaneViewMode::Chat => self
                .agent_chats
                .get(&session_id)
                .map(|chat| chat.view.read(cx).focus_handle(cx)),
            PaneViewMode::Terminal => self
                .terminals
                .get(&session_id)
                .map(|terminal| terminal.read(cx).focus_handle(cx)),
        }
    }

    /// Whether the pane's current view holds keyboard focus; drives the header highlight.
    pub(super) fn pane_has_focus(&self, session_id: Uuid, window: &Window, cx: &App) -> bool {
        self.pane_focus_handle(session_id, cx)
            .is_some_and(|focus| focus.is_focused(window))
    }

    /// Working directory for a pane's agent: the live shell cwd, else the stored one.
    pub(super) fn pane_working_directory(&self, session_id: Uuid, cx: &App) -> PathBuf {
        self.terminals
            .get(&session_id)
            .map(|terminal| terminal.read(cx).current_working_directory())
            .or_else(|| {
                self.snapshot
                    .terminal_sessions()
                    .find(|session| session.id == session_id)
                    .map(|session| PathBuf::from(&session.working_directory))
            })
            .unwrap_or_else(|| self.launch_directory.clone())
    }

    /// Whether the agent that a chat would resume is still running in the pane's terminal.
    fn agent_running_in_terminal(&self, session_id: Uuid, agent: ChatAgent) -> bool {
        self.resolved_agent_presence(session_id)
            .and_then(|presence| presence.chat_agent())
            == Some(agent)
    }

    /// Connects a pane's chat the first time the pane is drawn in chat view.
    /// While the same agent still runs in the terminal, the pane waits instead.
    pub(super) fn ensure_pane_chat(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_view_mode(session_id) != PaneViewMode::Chat
            || self.agent_chats.contains_key(&session_id)
        {
            return;
        }
        let Some(binding) = self.snapshot.agent_chat_binding(session_id).cloned() else {
            return;
        };
        if self.agent_running_in_terminal(session_id, binding.agent) {
            return;
        }
        self.open_pane_chat(session_id, binding.agent, binding.session_id, window, cx);
    }

    fn open_pane_chat(
        &mut self,
        session_id: Uuid,
        agent: ChatAgent,
        resume: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.pane_working_directory(session_id, cx);
        let target = resume.map_or(SessionTarget::New, SessionTarget::Load);
        let connection = connect_agent_chat(agent, cwd, target);
        // A chat that failed to start is closed too, so the next toggle retries it.
        let closed = connection.is_err();
        let view = cx.new(|cx| AgentChatView::new(agent, connection, window, cx));
        let subscription = cx.subscribe_in(&view, window, move |this, _, event, window, cx| {
            this.handle_pane_chat_event(session_id, event, window, cx);
        });
        self.agent_chats.insert(
            session_id,
            PaneChat {
                view,
                closed,
                _subscription: subscription,
            },
        );
    }

    pub(super) fn handle_pane_chat_event(
        &mut self,
        session_id: Uuid,
        event: &AgentChatViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AgentChatViewEvent::SessionBound {
                agent,
                session_id: agent_session,
            } => self.bind_agent_session(session_id, *agent, agent_session, cx),
            AgentChatViewEvent::SwitchToTerminal => {
                self.set_pane_view_mode(session_id, PaneViewMode::Terminal, window, cx);
            }
            AgentChatViewEvent::Closed => {
                if let Some(chat) = self.agent_chats.get_mut(&session_id) {
                    chat.closed = true;
                }
                cx.notify();
            }
        }
    }

    /// Stores the agent session that a chat or the hooks report, so a later
    /// handoff resumes that session. Other agents' sessions are ignored.
    pub(super) fn bind_agent_session(
        &mut self,
        session_id: Uuid,
        agent: ChatAgent,
        agent_session: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(mut binding) = self.snapshot.agent_chat_binding(session_id).cloned() else {
            return;
        };
        if binding.agent != agent || binding.session_id.as_deref() == Some(agent_session) {
            return;
        }
        binding.session_id = Some(agent_session.to_owned());
        self.write_pane_binding(session_id, binding, cx);
    }

    fn write_pane_binding(
        &mut self,
        session_id: Uuid,
        binding: AgentChatBinding,
        cx: &mut Context<Self>,
    ) {
        if self
            .snapshot
            .set_agent_chat_binding(session_id, Some(binding))
        {
            self.persist(cx);
        }
    }

    /// Shows a pane's terminal or chat. The terminal keeps running either way.
    pub(super) fn set_pane_view_mode(
        &mut self,
        session_id: Uuid,
        mode: PaneViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match mode {
            PaneViewMode::Chat => self.show_pane_chat(session_id, window, cx),
            PaneViewMode::Terminal => self.show_pane_terminal(session_id, cx),
        }
        self.sync_terminal_surface_visibility(cx);
        self.persist(cx);
        // A waiting pane has no chat element yet; focus falls back to the workspace.
        if self
            .snapshot
            .selected_session()
            .is_some_and(|session| session.id == session_id)
        {
            self.focus_selected_terminal(window, cx);
        }
    }

    fn show_pane_chat(&mut self, session_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        // Toggling again reconnects a chat whose connection ended.
        if self
            .agent_chats
            .get(&session_id)
            .is_some_and(|chat| chat.closed)
        {
            self.agent_chats.remove(&session_id);
        }
        let Some(agent) = self.pane_chat_agent(session_id) else {
            return;
        };
        // The session the terminal's hooks report is the newest one; the binding is the fallback.
        let terminal_session = self
            .resolved_agent_presence(session_id)
            .filter(|presence| presence.chat_agent() == Some(agent))
            .and_then(|presence| presence.session_id.clone());
        let bound_session = self
            .snapshot
            .agent_chat_binding(session_id)
            .filter(|binding| binding.agent == agent)
            .and_then(|binding| binding.session_id.clone());
        let session = terminal_session.or(bound_session);
        self.write_pane_binding(
            session_id,
            AgentChatBinding {
                agent,
                session_id: session.clone(),
                mode: PaneViewMode::Chat,
            },
            cx,
        );
        // A live chat already owns the session, so showing it is enough.
        if self.agent_chats.contains_key(&session_id)
            || self.agent_running_in_terminal(session_id, agent)
        {
            return;
        }
        self.open_pane_chat(session_id, agent, session, window, cx);
    }

    fn show_pane_terminal(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let Some(binding) = self.snapshot.agent_chat_binding(session_id).cloned() else {
            return;
        };
        // A chat with a conversation hands its session to the terminal, so it
        // shuts down. An empty live chat has nothing to resume and stays alive.
        let handoff = match self.agent_chats.get(&session_id) {
            Some(chat) => {
                let view = chat.view.read(cx);
                if view.has_conversation() {
                    view.session_id().map(str::to_owned)
                } else if chat.closed {
                    binding.session_id.clone()
                } else {
                    None
                }
            }
            None => binding.session_id.clone(),
        };
        if handoff.is_some()
            && let Some(chat) = self.agent_chats.remove(&session_id)
        {
            chat.view.update(cx, |view, cx| view.shutdown(cx));
        }
        self.write_pane_binding(
            session_id,
            AgentChatBinding {
                agent: binding.agent,
                session_id: handoff.clone().or(binding.session_id),
                mode: PaneViewMode::Terminal,
            },
            cx,
        );
        let Some(session) = handoff else {
            return;
        };
        let Some(terminal) = self.terminals.get(&session_id).cloned() else {
            return;
        };
        let agent_running = self.resolved_agent_presence(session_id).is_some();
        let foreground_is_shell = terminal.read(cx).foreground_is_interactive_shell();
        if may_type_resume_command(agent_running, foreground_is_shell) {
            let command = binding.agent.resume_command(&session);
            terminal.update(cx, |terminal, cx| {
                terminal.run_command(&command, cx);
            });
        }
    }

    /// Opens a new tab in `project_id` whose chat starts a new `agent` session in `cwd`.
    pub(super) fn open_agent_chat_tab(
        &mut self,
        project_id: Uuid,
        agent: ChatAgent,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, session_id)) = self.snapshot.open_tab_in_project(project_id, true) else {
            self.persistence_error = Some("The project has no associated folder.".into());
            cx.notify();
            return;
        };
        self.snapshot
            .update_session_working_directory(session_id, &cwd);
        self.snapshot.set_agent_chat_binding(
            session_id,
            Some(AgentChatBinding {
                agent,
                session_id: None,
                mode: PaneViewMode::Chat,
            }),
        );
        self.reconcile_terminal_views(cx);
        self.show_terminal_tab(window, cx);
        // The chat is created while the pane is drawn, so focus it on the next frame.
        self.pending_focus_session = Some(session_id);
        cx.notify();
    }
}
