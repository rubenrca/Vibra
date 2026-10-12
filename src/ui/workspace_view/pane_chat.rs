//! Terminal and chat views of one pane (issue #36). The user picks the view and
//! the agent; the pane's terminal keeps running while its chat is shown. A chat
//! connects only when no terminal agent owns the same session, so each session
//! has one owner at a time. Detection never changes the view or the agent.

use std::path::{Path, PathBuf};

use gpui::{App, AppContext, Context, Entity, FocusHandle, Focusable, Subscription, Window};
use uuid::Uuid;

use crate::domain::agent_chat::{AgentChatBinding, ChatAgent, PaneViewMode};
use crate::domain::usage::{UsageSnapshot, now_timestamp, resource_label, timestamp};
use crate::infrastructure::agent_chat::{AgentChatConnection, SessionTarget};
use crate::ui::agent_chat::{
    AgentChatView, AgentChatViewEvent, ChatViewContext, FileSearch, QuotaMeter,
};

use super::{ChatTarget, ContextMenuKind, WorkspaceView};

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

/// The agents the UI offers: only those whose CLI is installed.
pub(super) fn available_chat_agents() -> Vec<ChatAgent> {
    ChatAgent::ALL
        .into_iter()
        .filter(|agent| crate::infrastructure::agent_chat::launch::is_available(*agent))
        .collect()
}

/// The provider id the usage monitor files an agent's quotas under.
fn usage_provider_id(agent: ChatAgent) -> &'static str {
    match agent {
        ChatAgent::Claude => "claude",
        ChatAgent::Codex => "codex",
        ChatAgent::Grok => "grok",
    }
}

/// Human text for the time left until a reset: "45m", "3h 52m", "1d 2h".
/// `None` when the reset is not in the future.
pub(super) fn format_resets_in(seconds: i64) -> Option<String> {
    if seconds <= 0 {
        return None;
    }
    let minutes = (seconds + 59) / 60;
    Some(if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 24 * 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{}d {}h", minutes / (24 * 60), minutes / 60 % 24)
    })
}

/// Shorter windows first: the session window, then the week, then the rest.
fn quota_rank(key: &str) -> u8 {
    match key {
        "session" => 0,
        "weekly" => 1,
        "daily" => 2,
        "monthly" => 3,
        _ => 4,
    }
}

/// The quota windows of an agent's provider, shortest first. Resources without a
/// percentage (balances, unknown limits) are left out, and overage shows as 100%.
pub(super) fn chat_quota_meters(
    usage: &UsageSnapshot,
    agent: ChatAgent,
    now: i64,
) -> Vec<QuotaMeter> {
    let Some(provider) = usage.providers.get(usage_provider_id(agent)) else {
        return Vec::new();
    };
    let mut windows: Vec<(u8, QuotaMeter)> = provider
        .resources
        .iter()
        .filter_map(|(key, resource)| {
            let used = resource.percent_used()?;
            let resets_in = resource
                .resets_at
                .as_deref()
                .and_then(timestamp)
                .and_then(|reset| format_resets_in(reset - now));
            Some((
                quota_rank(key),
                QuotaMeter {
                    label: resource_label(key).to_owned(),
                    used_percent: used.min(100.0) as f32,
                    resets_in,
                },
            ))
        })
        .collect();
    // The sort is stable, so windows of equal rank keep the provider's key order.
    windows.sort_by_key(|(rank, _)| *rank);
    windows.into_iter().map(|(_, meter)| meter).collect()
}

/// Whether a `.git` file points into a repository's `worktrees/` directory.
/// Submodules point into `modules/` and are not worktrees.
pub(super) fn is_linked_worktree_pointer(content: &str) -> bool {
    content
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))
        .is_some_and(|dir| dir.trim().split(['/', '\\']).rev().nth(1) == Some("worktrees"))
}

/// Whether `cwd` sits in a linked worktree: the nearest `.git` entry is a file
/// that points into `worktrees/`. Reads one small file at most.
pub(super) fn is_linked_worktree(cwd: &Path) -> bool {
    for dir in cwd.ancestors() {
        let marker = dir.join(".git");
        if marker.is_dir() {
            return false;
        }
        if marker.is_file() {
            return std::fs::read_to_string(&marker)
                .is_ok_and(|content| is_linked_worktree_pointer(&content));
        }
    }
    false
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

    /// The chat-capable agent running in the pane's terminal, if any. It only
    /// pre-selects the agent picker and decides when a waiting chat connects.
    pub(super) fn detected_chat_agent(&self, session_id: Uuid) -> Option<ChatAgent> {
        self.resolved_agent_presence(session_id)
            .and_then(|presence| presence.chat_agent())
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
    pub(super) fn agent_running_in_terminal(&self, session_id: Uuid, agent: ChatAgent) -> bool {
        self.detected_chat_agent(session_id) == Some(agent)
    }

    /// Connects a pane that the user switched to chat while its terminal agent
    /// ran, once that agent exits. Called while the pane is drawn. Restored panes
    /// are never pending, so they connect only when the user asks.
    pub(super) fn connect_waiting_pane_chat(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pending_chat_panes.contains(&session_id) {
            return;
        }
        if self.pane_view_mode(session_id) != PaneViewMode::Chat {
            self.pending_chat_panes.remove(&session_id);
            return;
        }
        self.connect_pane_chat(session_id, window, cx);
    }

    /// Starts the pane's bound chat, as the user asked for it. While the same
    /// agent still runs in the terminal, the pane waits for it to exit instead.
    pub(super) fn connect_pane_chat(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(binding) = self.snapshot.agent_chat_binding(session_id).cloned() else {
            return;
        };
        // A chat that ended, or one for another agent, makes way for a new connection.
        let replaced = self
            .agent_chats
            .get(&session_id)
            .is_some_and(|chat| chat.closed || chat.view.read(cx).agent() != binding.agent);
        if replaced && let Some(chat) = self.agent_chats.remove(&session_id) {
            chat.view.update(cx, |view, cx| view.shutdown(cx));
        }
        if self.agent_chats.contains_key(&session_id) {
            // A live chat already owns the session, so showing it is enough.
            self.pending_chat_panes.remove(&session_id);
            return;
        }
        if self.agent_running_in_terminal(session_id, binding.agent) {
            self.pending_chat_panes.insert(session_id);
            return;
        }
        self.pending_chat_panes.remove(&session_id);
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
        self.refresh_chat_context(session_id, cx);
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

    /// Shows a pane's terminal or its bound chat. The terminal keeps running either way.
    pub(super) fn set_pane_view_mode(
        &mut self,
        session_id: Uuid,
        mode: PaneViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match mode {
            PaneViewMode::Chat => self.switch_to_pane_chat(session_id, window, cx),
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

    /// Shows a pane's chat with `agent`, chosen by the user. The pane keeps its
    /// terminal; the chat connects now or once the agent in the terminal exits.
    pub(super) fn show_pane_as_chat(
        &mut self,
        session_id: Uuid,
        agent: ChatAgent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bound_session = self
            .snapshot
            .agent_chat_binding(session_id)
            .filter(|binding| binding.agent == agent)
            .and_then(|binding| binding.session_id.clone());
        self.write_pane_binding(
            session_id,
            AgentChatBinding {
                agent,
                session_id: bound_session,
                mode: PaneViewMode::Chat,
            },
            cx,
        );
        self.set_pane_view_mode(session_id, PaneViewMode::Chat, window, cx);
    }

    fn switch_to_pane_chat(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(binding) = self.snapshot.agent_chat_binding(session_id).cloned() else {
            // Only a pane with a binding has a chat to show.
            return;
        };
        // The session the terminal's hooks report is the newest one; the binding is the fallback.
        let session = self
            .resolved_agent_presence(session_id)
            .filter(|presence| presence.chat_agent() == Some(binding.agent))
            .and_then(|presence| presence.session_id.clone())
            .or(binding.session_id.clone());
        self.write_pane_binding(
            session_id,
            AgentChatBinding {
                agent: binding.agent,
                session_id: session,
                mode: PaneViewMode::Chat,
            },
            cx,
        );
        self.connect_pane_chat(session_id, window, cx);
    }

    fn show_pane_terminal(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        self.pending_chat_panes.remove(&session_id);
        let Some(binding) = self.snapshot.agent_chat_binding(session_id).cloned() else {
            return;
        };
        // A chat with a conversation hands its session to the terminal, so it
        // shuts down. An empty live chat has nothing to resume and stays alive.
        // A pane that never connected (a restored placeholder) launches nothing.
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
            None => None,
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

    /// The Chat half of a pane's Terminal | Chat control. A pane with a binding
    /// switches to its agent; an unbound pane opens the agent picker at `(x, y)`.
    pub(super) fn click_pane_chat_control(
        &mut self,
        session_id: Uuid,
        x: f32,
        y: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_terminal(session_id, window, cx);
        if self.snapshot.agent_chat_binding(session_id).is_some() {
            self.set_pane_view_mode(session_id, PaneViewMode::Chat, window, cx);
        } else {
            // Detection never picks the agent; the user does.
            self.open_context_menu(
                ContextMenuKind::ChatAgentPicker {
                    target: ChatTarget::Pane(session_id),
                },
                x,
                y,
                cx,
            );
        }
    }

    /// Starts the chat chosen in an agent picker at its target.
    pub(super) fn start_chat_at(
        &mut self,
        target: ChatTarget,
        agent: ChatAgent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            ChatTarget::Pane(session_id) => self.show_pane_as_chat(session_id, agent, window, cx),
            ChatTarget::Split {
                session_id,
                direction,
            } => self.split_pane_with_chat(session_id, direction, agent, window, cx),
            ChatTarget::NewTab(project_id) => {
                let cwd = self
                    .snapshot
                    .project(project_id)
                    .and_then(|project| project.directory())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.selected_live_cwd(cx));
                self.open_agent_chat_tab(project_id, agent, cwd, window, cx);
            }
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
        // The user asked for this chat, so it connects now.
        self.connect_pane_chat(session_id, window, cx);
        self.show_terminal_tab(window, cx);
        // The chat is created while the pane is drawn, so focus it on the next frame.
        self.pending_focus_session = Some(session_id);
        cx.notify();
    }

    /// Pushes what each open chat shows (branch, worktree, quota, file search
    /// root). Called when the branch, usage or a working directory changes.
    pub(super) fn refresh_chat_contexts(&mut self, cx: &mut Context<Self>) {
        let session_ids: Vec<Uuid> = self.agent_chats.keys().copied().collect();
        for session_id in session_ids {
            self.refresh_chat_context(session_id, cx);
        }
    }

    fn refresh_chat_context(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let Some(view) = self
            .agent_chats
            .get(&session_id)
            .map(|chat| chat.view.clone())
        else {
            return;
        };
        let agent = view.read(cx).agent();
        let cwd = self.pane_working_directory(session_id, cx);
        let root = self.pane_project_root(session_id, &cwd);
        let context = ChatViewContext {
            branch: self.branch_for_root(&root),
            worktree: self.linked_worktree(&cwd, cx),
            quota: chat_quota_meters(self.usage.snapshot(), agent, now_timestamp()),
        };
        let file_search = FileSearch {
            port: self.file_port.clone(),
            root,
        };
        view.update(cx, |view, cx| {
            view.set_context(context, cx);
            view.set_file_search(Some(file_search));
        });
    }

    /// The pane's project folder, or its working directory for a project without one.
    fn pane_project_root(&self, session_id: Uuid, cwd: &Path) -> PathBuf {
        self.snapshot
            .project_for_session(session_id)
            .and_then(|project| project.directory())
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.to_path_buf())
    }

    /// The branch from the status-bar summary, when that summary is for `root`.
    fn branch_for_root(&self, root: &Path) -> Option<String> {
        self.branch_summary
            .as_ref()
            .filter(|(summary_root, _)| summary_root.as_path() == root)
            .map(|(_, summary)| summary.branch.clone())
    }

    /// Whether `cwd` is a linked worktree. The check runs off the UI thread and
    /// is cached per directory; the answer refreshes the chats when it arrives.
    fn linked_worktree(&mut self, cwd: &Path, cx: &mut Context<Self>) -> bool {
        if let Some(linked) = self.linked_worktrees.get(cwd) {
            return *linked;
        }
        if self.worktree_checks.insert(cwd.to_path_buf()) {
            let path = cwd.to_path_buf();
            cx.spawn(async move |this, cx| {
                let check = path.clone();
                let linked = cx
                    .background_spawn(async move { is_linked_worktree(&check) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.worktree_checks.remove(&path);
                    this.linked_worktrees.insert(path, linked);
                    this.refresh_chat_contexts(cx);
                });
            })
            .detach();
        }
        false
    }
}
