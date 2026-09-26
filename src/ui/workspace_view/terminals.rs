//! Terminal lifecycle, visibility, focus, pane labels, and event delivery.
//! This module coordinates live PTYs with the single workspace snapshot.

use gpui::{AppContext, Context, Focusable, Window};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::domain::workspace::SessionSnapshot;
use crate::ports::terminal::TerminalAgentKindSource;
use crate::ui::terminal::{TerminalInsertStatus, TerminalView, TerminalViewEvent};

use super::chrome::{pane_detail_title, tab_display_title};
use super::{ContextMenuKind, PaneIdentity, WorkspaceSection, WorkspaceView};

impl WorkspaceView {
    pub(super) fn pane_identity_with_cwd(
        &self,
        session: &SessionSnapshot,
        index: usize,
        working_directory: &str,
    ) -> PaneIdentity {
        let alias = self.pane_names.get(&session.id).map(String::as_str);
        let presence = self.resolved_agent_presence(session.id);
        PaneIdentity {
            title: tab_display_title(
                alias.or(session.agent_task_title.as_deref()),
                Some(session.title.as_str()),
                Some(working_directory),
                index,
            ),
            detail: pane_detail_title(
                alias,
                Some(session.title.as_str()),
                Some(working_directory),
                self.home_directory.as_deref(),
            ),
            agent_kind: presence.as_ref().map(|presence| presence.kind.clone()),
            agent_state: presence.as_ref().map(|presence| presence.state),
            agent_attention: presence.as_ref().and_then(|presence| presence.attention),
        }
    }

    pub(super) fn pane_identity(
        &self,
        session: &SessionSnapshot,
        index: usize,
        cx: &Context<Self>,
    ) -> PaneIdentity {
        let live_cwd = self
            .terminals
            .get(&session.id)
            .map(|terminal| {
                terminal
                    .read(cx)
                    .cached_working_directory()
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| session.working_directory.clone());
        self.pane_identity_with_cwd(session, index, &live_cwd)
    }

    pub(super) fn pane_identity_by_id(
        &self,
        session_id: Uuid,
        cx: &Context<Self>,
    ) -> Option<PaneIdentity> {
        let session = self
            .snapshot
            .terminal_sessions()
            .find(|session| session.id == session_id)?;
        Some(self.pane_identity(session, 0, cx))
    }

    /// Active console directory: live PTY cwd when available, else snapshot / launch dir.
    pub(super) fn selected_live_cwd(&self, cx: &Context<Self>) -> PathBuf {
        if let Some(session) = self.snapshot.selected_session() {
            if let Some(terminal) = self.terminals.get(&session.id) {
                return terminal.read(cx).current_working_directory();
            }
            return PathBuf::from(&session.working_directory);
        }
        self.snapshot
            .selected_project()
            .and_then(|project| project.directory().map(PathBuf::from))
            .unwrap_or_else(|| self.launch_directory.clone())
    }

    pub(super) fn reconcile_terminal_views(&mut self, cx: &mut Context<Self>) {
        let sessions: Vec<_> = self.snapshot.terminal_sessions().cloned().collect();
        let live_ids: HashSet<_> = sessions.iter().map(|session| session.id).collect();

        let stale_ids: Vec<_> = self
            .terminals
            .keys()
            .filter(|session_id| !live_ids.contains(session_id))
            .copied()
            .collect();
        for session_id in stale_ids {
            self.fail_pending_review_for_session(session_id, cx);
            if let Some(terminal) = self.terminals.remove(&session_id) {
                terminal.read(cx).shutdown();
            }
            self.terminal_subscriptions.remove(&session_id);
            self.automation_tokens.remove(&session_id);
            self.agent_presence.remove(&session_id);
            self.hook_agent_presence.remove(&session_id);
            self.pane_names.remove(&session_id);
            self.agent_activity_seen.remove(&session_id);
        }
        self.inbox.forget_closed_panes(&live_ids);

        for session in sessions {
            if self.terminals.contains_key(&session.id) {
                continue;
            }
            let session_id = session.id;
            let terminal_port = self.terminal_port.clone();
            let working_directory = PathBuf::from(session.working_directory);
            let title = session.title;
            let token = *self
                .automation_tokens
                .entry(session_id)
                .or_insert_with(Uuid::new_v4);
            let mut environment = HashMap::new();
            environment.insert("VIBRA_PANE_ID".into(), session_id.to_string());
            environment.insert("VIBRA_AUTOMATION_TOKEN".into(), token.to_string());
            if let Ok(executable) = std::env::current_exe() {
                environment.insert(
                    "VIBRA_CLI".into(),
                    executable.to_string_lossy().into_owned(),
                );
            }
            if let Some(socket) = &self.automation_socket {
                environment.insert(
                    "VIBRA_AUTOMATION_SOCKET".into(),
                    socket.to_string_lossy().into_owned(),
                );
            }
            let terminal = cx.new(|cx| {
                TerminalView::new_with_environment(
                    session_id,
                    title,
                    Path::new(&working_directory),
                    terminal_port,
                    environment,
                    cx,
                )
            });
            terminal.update(cx, |terminal, cx| {
                terminal.apply_font_size(self.settings.terminal_font_size, cx);
            });
            let subscription = cx.subscribe(
                &terminal,
                |this, _terminal, event: &TerminalViewEvent, cx| {
                    this.handle_terminal_view_event(event, cx);
                },
            );
            self.terminals.insert(session_id, terminal);
            self.terminal_subscriptions.insert(session_id, subscription);
        }
        self.sync_terminal_surface_visibility(cx);
    }

    pub(super) fn visible_terminal_ids(&self, cx: &Context<Self>) -> HashSet<Uuid> {
        if self.workspace_section == WorkspaceSection::Workspace && !self.review_covers_terminal(cx)
        {
            self.snapshot.painted_session_ids()
        } else {
            HashSet::new()
        }
    }

    pub(super) fn sync_terminal_surface_visibility(&self, cx: &mut Context<Self>) {
        let visible = self.visible_terminal_ids(cx);
        for (session_id, terminal) in &self.terminals {
            let shown = visible.contains(session_id);
            terminal.update(cx, |terminal, _| terminal.set_surface_visible(shown));
        }
    }

    pub(super) fn handle_terminal_view_event(
        &mut self,
        event: &TerminalViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalViewEvent::ExternalPasteResolved {
                session_id,
                token,
                accepted,
            } => {
                if self
                    .pending_review_pastes
                    .get(token)
                    .is_some_and(|target| target == session_id)
                {
                    self.pending_review_pastes.remove(token);
                    self.diff_view.update(cx, |view, cx| {
                        view.resolve_review_delivery(*token, *accepted, cx);
                    });
                }
            }
            TerminalViewEvent::TitleChanged { session_id, title } => {
                if self.snapshot.update_session_title(*session_id, title) {
                    self.persist(cx);
                }
            }
            TerminalViewEvent::WorkingDirectoryChanged { session_id, path } => {
                let is_selected = self
                    .snapshot
                    .selected_session()
                    .is_some_and(|session| session.id == *session_id);
                let previous_files_root = is_selected.then(|| self.project_root());
                let changed = self
                    .snapshot
                    .update_session_working_directory(*session_id, path);
                if is_selected {
                    self.sync_diff_root(cx);
                    // Unassociated legacy spaces temporarily follow their existing terminal.
                    let new_root = self.project_root();
                    if previous_files_root.as_ref() != Some(&new_root) {
                        self.expanded_directories.retain(|entry| {
                            entry.starts_with(&new_root) || new_root.starts_with(entry)
                        });
                        self.expanded_directories.insert(new_root);
                        self.refresh_project_files(cx);
                    }
                }
                if changed {
                    self.persist(cx);
                } else {
                    cx.notify();
                }
            }
            TerminalViewEvent::Exited {
                session_id,
                code: _code,
            } => {
                self.fail_pending_review_for_session(*session_id, cx);
                self.agent_presence.remove(session_id);
                self.hook_agent_presence.remove(session_id);
                self.publish_agent_activity(*session_id, cx);
                cx.notify();
            }
            TerminalViewEvent::ContextMenuRequested { session_id, x, y } => {
                let session_id = *session_id;
                let was_selected = self
                    .snapshot
                    .selected_session()
                    .is_some_and(|session| session.id == session_id);
                if self.snapshot.select_terminal_global(session_id) && !was_selected {
                    self.sync_terminal_surface_visibility(cx);
                    self.sync_diff_root(cx);
                    self.refresh_project_files(cx);
                    self.persist(cx);
                }
                self.open_context_menu(ContextMenuKind::Pane { session_id }, *x, *y, cx);
            }
            TerminalViewEvent::Activated { session_id } => {
                self.inbox.mark_pane_read(*session_id);
                if self.snapshot.select_terminal(*session_id) {
                    self.sync_terminal_surface_visibility(cx);
                    self.sync_diff_root(cx);
                    self.refresh_project_files(cx);
                    self.persist(cx);
                }
                cx.notify();
            }
            TerminalViewEvent::AgentPresenceChanged {
                session_id,
                presence,
            } => {
                let previous = self.agent_presence.get(session_id);
                let occupant_changed = match (previous, presence) {
                    (Some(previous), Some(current)) => {
                        !previous.kind.eq_ignore_ascii_case(&current.kind)
                            || (previous.kind_source == TerminalAgentKindSource::Process
                                && current.kind_source != TerminalAgentKindSource::Process)
                            || matches!(
                                (previous.process_id, current.process_id),
                                (Some(left), Some(right)) if left != right
                            )
                    }
                    (Some(_), None) => true,
                    _ => false,
                };
                let definitive_exit = previous.is_some_and(|previous| {
                    previous.kind_source == TerminalAgentKindSource::Process
                }) || !self.hook_agent_presence.contains_key(session_id);
                if occupant_changed && (presence.is_some() || definitive_exit) {
                    self.hook_agent_presence.remove(session_id);
                }
                if let Some(presence) = presence {
                    self.agent_presence.insert(*session_id, presence.clone());
                } else {
                    self.agent_presence.remove(session_id);
                }
                self.publish_agent_activity(*session_id, cx);
                cx.notify();
            }
            TerminalViewEvent::FontSizeChanged { size } => {
                self.set_terminal_font_size(*size, cx);
            }
        }
    }

    pub(super) fn fail_pending_review_for_session(
        &mut self,
        session_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let tokens: Vec<_> = self
            .pending_review_pastes
            .iter()
            .filter_map(|(token, target)| (*target == session_id).then_some(*token))
            .collect();
        for token in tokens {
            self.pending_review_pastes.remove(&token);
            self.diff_view.update(cx, |view, cx| {
                view.resolve_review_delivery(token, false, cx)
            });
        }
    }

    pub(super) fn focus_selected_terminal(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_section != WorkspaceSection::Workspace {
            self.focus_handle.focus(window);
            return;
        }
        if self.review_covers_terminal(cx) {
            self.diff_view.read(cx).focus_review(window);
            return;
        }
        if let Some(terminal) = self
            .snapshot
            .selected_session()
            .and_then(|session| self.terminals.get(&session.id))
        {
            terminal.read(cx).focus_handle(cx).focus(window);
        } else {
            self.focus_handle.focus(window);
        }
    }

    /// Paste Git review comments into the agent that should act on them: the
    /// selected pane when it runs an agent, else a visible pane that does.
    /// The prompt is pasted, never submitted, so it can still be edited.
    pub(super) fn send_review_to_agent(
        &mut self,
        prompt: &str,
        token: Uuid,
        cx: &mut Context<Self>,
    ) -> TerminalInsertStatus {
        let selected = self.snapshot.selected_session().map(|session| session.id);
        let has_agent = |this: &Self, id: Uuid| this.resolved_agent_presence(id).is_some();
        let target = selected
            .filter(|id| has_agent(self, *id))
            .or_else(|| {
                self.snapshot.selected_tab().and_then(|tab| {
                    tab.sessions
                        .iter()
                        .map(|session| session.id)
                        .find(|id| has_agent(self, *id))
                })
            })
            .or(selected);
        let Some(target) = target else {
            self.persistence_error =
                Some("Abre una terminal con un agente para enviarle la revisión.".into());
            cx.notify();
            return TerminalInsertStatus::Rejected;
        };
        let Some(terminal) = self.terminals.get(&target).cloned() else {
            self.persistence_error =
                Some("No se pudo encontrar la terminal para enviarle la revisión.".into());
            cx.notify();
            return TerminalInsertStatus::Rejected;
        };
        let status = terminal.update(cx, |terminal, cx| {
            terminal.insert_external_text(prompt, token, cx)
        });
        if status == TerminalInsertStatus::Rejected {
            self.persistence_error = Some(
                concat!(
                    "No se pudo pegar la revisión: la terminal está ocupada o rechazó el texto. ",
                    "Los comentarios siguen disponibles."
                )
                .into(),
            );
            cx.notify();
            return status;
        }
        if status == TerminalInsertStatus::Pending {
            self.pending_review_pastes.insert(token, target);
        }
        if self.persistence_error.as_ref().is_some_and(|error| {
            let error = error.to_string();
            error.starts_with("No se pudo pegar la revisión:")
                || error.starts_with("Abre una terminal con un agente")
                || error.starts_with("No se pudo encontrar la terminal para enviarle la revisión")
        }) {
            self.persistence_error = None;
        }
        if selected != Some(target) && self.snapshot.select_terminal(target) {
            self.sync_terminal_surface_visibility(cx);
            self.persist(cx);
        }
        self.diff_view
            .update(cx, |diff, cx| diff.set_review_expanded(false, cx));
        self.sync_terminal_surface_visibility(cx);
        self.pending_focus_session = Some(target);
        cx.notify();
        status
    }

    pub(super) fn focus_terminal(
        &self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Deferred callbacks must not steal focus after the user navigated
        // away or opened an overlay while the callback was waiting.
        if self.workspace_section != WorkspaceSection::Workspace
            || self.review_covers_terminal(cx)
            || self.palette_mode.is_some()
            || self.settings_open
            || self.usage.open
            || self.rename_prompt.is_some()
            || self
                .snapshot
                .selected_session()
                .is_none_or(|session| session.id != session_id)
        {
            return;
        }
        if let Some(terminal) = self.terminals.get(&session_id) {
            terminal.read(cx).focus_handle(cx).focus(window);
        }
    }

    pub(super) fn select_terminal(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.select_terminal(session_id) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
        }
        self.focus_terminal(session_id, window, cx);
    }

    pub(super) fn capture_selected_working_directory(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.snapshot.selected_session().map(|session| session.id) else {
            return;
        };
        let Some(terminal) = self.terminals.get(&session_id) else {
            return;
        };
        let path = terminal.read(cx).current_working_directory();
        self.snapshot
            .update_session_working_directory(session_id, &path);
    }
}
