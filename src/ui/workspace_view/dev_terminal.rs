//! Lifecycle of the utility terminals, owned by a workspace rather than a pane.

use std::collections::{HashMap, HashSet};

use gpui::{AppContext, Context, Entity, Focusable, Window};
use uuid::Uuid;

use crate::ToggleDevTerminal;
use crate::ui::terminal::{TerminalView, TerminalViewEvent};

use super::WorkspaceView;

pub(super) struct DevTerminalDrawer {
    pub(super) terminals: Vec<Entity<TerminalView>>,
    pub(super) selected_id: Uuid,
    pub(super) visible: bool,
}

impl WorkspaceView {
    pub(super) fn is_dev_terminal_visible(&self) -> bool {
        self.current_workspace_id()
            .and_then(|workspace_id| self.dev_terminals.get(&workspace_id))
            .is_some_and(|drawer| drawer.visible)
    }

    pub(super) fn is_dev_terminal(&self, session_id: Uuid, cx: &Context<Self>) -> bool {
        self.dev_workspace_for_session(session_id, cx).is_some()
    }

    pub(super) fn dev_workspace_for_session(
        &self,
        session_id: Uuid,
        cx: &Context<Self>,
    ) -> Option<Uuid> {
        self.dev_terminals
            .iter()
            .find_map(|(workspace_id, drawer)| {
                drawer
                    .terminals
                    .iter()
                    .any(|terminal| terminal.read(cx).session_id() == session_id)
                    .then_some(*workspace_id)
            })
    }

    pub(super) fn selected_dev_terminal(
        &self,
        workspace_id: Uuid,
        cx: &Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        let drawer = self.dev_terminals.get(&workspace_id)?;
        drawer
            .terminals
            .iter()
            .find(|terminal| terminal.read(cx).session_id() == drawer.selected_id)
            .or_else(|| drawer.terminals.first())
            .cloned()
    }

    fn spawn_dev_terminal(
        &mut self,
        workspace_id: Uuid,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let working_directory = self.selected_live_cwd(cx);
        let terminal_port = self.terminal_port.clone();
        let font_size = self.settings.terminal_font_size;
        let visible = self
            .dev_terminals
            .get(&workspace_id)
            .is_some_and(|drawer| drawer.visible);
        let mut environment = HashMap::new();
        if let Ok(executable) = std::env::current_exe() {
            environment.insert(
                "VIBRA_CLI".into(),
                executable.to_string_lossy().into_owned(),
            );
        }
        let session_id = Uuid::new_v4();
        let terminal = cx.new(|cx| {
            TerminalView::new_with_environment(
                session_id,
                "Dev terminal".to_owned(),
                &working_directory,
                terminal_port,
                environment,
                cx,
            )
        });
        terminal.update(cx, |terminal, cx| {
            terminal.apply_font_size(font_size, cx);
            terminal.set_surface_visible(visible);
        });
        let subscription = cx.subscribe(
            &terminal,
            |this, _terminal, event: &TerminalViewEvent, cx| {
                this.handle_terminal_view_event(event, cx);
            },
        );
        self.dev_terminal_subscriptions
            .insert(session_id, subscription);
        let drawer = self
            .dev_terminals
            .entry(workspace_id)
            .or_insert_with(|| DevTerminalDrawer {
                terminals: Vec::new(),
                selected_id: session_id,
                visible: false,
            });
        drawer.terminals.push(terminal.clone());
        drawer.selected_id = session_id;
        terminal
    }

    fn ensure_dev_terminal(&mut self, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let workspace_id = self.current_workspace_id()?;
        if let Some(terminal) = self.selected_dev_terminal(workspace_id, cx) {
            return Some(terminal);
        }
        Some(self.spawn_dev_terminal(workspace_id, cx))
    }

    pub(super) fn prune_dev_terminals(&mut self, cx: &mut Context<Self>) {
        let live: HashSet<Uuid> = self
            .snapshot
            .workspace_entries()
            .into_iter()
            .map(|entry| entry.workspace_id)
            .collect();
        let stale: Vec<Uuid> = self
            .dev_terminals
            .keys()
            .copied()
            .filter(|workspace_id| !live.contains(workspace_id))
            .collect();
        for workspace_id in stale {
            self.shutdown_dev_drawer(workspace_id, cx);
        }
    }

    fn shutdown_dev_drawer(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let Some(drawer) = self.dev_terminals.remove(&workspace_id) else {
            return;
        };
        for terminal in drawer.terminals {
            let session_id = terminal.read(cx).session_id();
            terminal.read(cx).shutdown();
            self.dev_terminal_subscriptions.remove(&session_id);
        }
    }

    pub(super) fn set_dev_terminal_visible(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.current_workspace_id() else {
            return;
        };
        if visible {
            let Some(terminal) = self.ensure_dev_terminal(cx) else {
                return;
            };
            if let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) {
                drawer.visible = true;
            }
            self.sync_terminal_surface_visibility(cx);
            cx.notify();
            cx.defer_in(window, move |_, window, cx| {
                terminal.read(cx).focus_handle(cx).focus(window);
            });
            return;
        }
        let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) else {
            return;
        };
        if !drawer.visible {
            return;
        }
        drawer.visible = false;
        self.pending_focus_dev_terminal = false;
        self.sync_terminal_surface_visibility(cx);
        self.focus_selected_terminal(window, cx);
        cx.notify();
    }

    pub(super) fn toggle_dev_terminal(
        &mut self,
        _: &ToggleDevTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_dev_terminal_visible(!self.is_dev_terminal_visible(), window, cx);
    }

    pub(super) fn add_dev_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.current_workspace_id() else {
            return;
        };
        let terminal = self.spawn_dev_terminal(workspace_id, cx);
        self.select_dev_terminal_tab(terminal.read(cx).session_id(), window, cx);
    }

    pub(super) fn select_dev_terminal_tab(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.dev_workspace_for_session(session_id, cx) else {
            return;
        };
        if let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) {
            drawer.selected_id = session_id;
            drawer.visible = true;
        }
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
        if let Some(terminal) = self.selected_dev_terminal(workspace_id, cx) {
            cx.defer_in(window, move |_, window, cx| {
                terminal.read(cx).focus_handle(cx).focus(window);
            });
        }
    }

    pub(super) fn close_dev_terminal(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.dev_workspace_for_session(session_id, cx) else {
            return;
        };
        let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) else {
            return;
        };
        let Some(index) = drawer
            .terminals
            .iter()
            .position(|terminal| terminal.read(cx).session_id() == session_id)
        else {
            return;
        };
        drawer.terminals.remove(index).read(cx).shutdown();
        self.dev_terminal_subscriptions.remove(&session_id);
        if drawer.terminals.is_empty() {
            self.dev_terminals.remove(&workspace_id);
            self.pending_focus_dev_terminal = false;
            self.sync_terminal_surface_visibility(cx);
            self.focus_selected_terminal(window, cx);
            cx.notify();
            return;
        }
        if drawer.selected_id == session_id {
            let next = index.min(drawer.terminals.len() - 1);
            drawer.selected_id = drawer.terminals[next].read(cx).session_id();
        }
        let selected = self.selected_dev_terminal(workspace_id, cx);
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
        if let Some(terminal) = selected {
            cx.defer_in(window, move |_, window, cx| {
                terminal.read(cx).focus_handle(cx).focus(window);
            });
        }
    }
}
