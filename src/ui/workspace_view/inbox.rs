//! Inbox: agents that finished or wait for the user, across every project,
//! plus the automations that started. A row opens its terminal.

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px, svg};
use uuid::Uuid;

use crate::domain::agents::{AgentAttention, AgentRuntimeState};
use crate::domain::inbox::{InboxItem, InboxKind, relative_time};
use crate::infrastructure::library::unix_now;
use crate::infrastructure::notifications::{AgentNotificationKind, agent_notification_copy};
use crate::ui::agent_marks::{agent_compact_badge, agent_status_color};
use crate::ui::theme::{colors, surface_tint};

use super::WorkspaceView;
use super::navigation::{section_button, section_empty_state, section_frame, section_heading};

pub(super) fn agent_state_label(
    state: AgentRuntimeState,
    attention: Option<AgentAttention>,
) -> &'static str {
    match (state, attention) {
        (AgentRuntimeState::Waiting, Some(AgentAttention::Permission)) => "Pide permiso",
        (AgentRuntimeState::Waiting, _) => "Espera tu respuesta",
        (AgentRuntimeState::Working, _) => "Trabajando",
        (AgentRuntimeState::Idle, _) => "En espera",
    }
}

fn inbox_kind_color(kind: InboxKind) -> gpui::Rgba {
    match kind {
        InboxKind::NeedsPermission | InboxKind::AutomationFailed => colors().danger,
        InboxKind::NeedsAttention => colors().warning,
        InboxKind::Finished => colors().success,
        InboxKind::AutomationStarted => colors().accent,
    }
}

impl WorkspaceView {
    pub(super) fn record_agent_event(
        &mut self,
        pane_id: Uuid,
        event: AgentNotificationKind,
        agent: &str,
        seen: bool,
    ) {
        let kind = match event {
            AgentNotificationKind::Finished => InboxKind::Finished,
            AgentNotificationKind::NeedsPermission => InboxKind::NeedsPermission,
            AgentNotificationKind::NeedsAttention => InboxKind::NeedsAttention,
        };
        let (title, _) = agent_notification_copy(event, agent);
        let detail = self.session_location_label(pane_id);
        self.inbox
            .push(kind, Some(pane_id), title, detail, unix_now(), seen);
    }

    /// "Project › Session" for a terminal, or an empty string once it is gone.
    pub(super) fn session_location_label(&self, pane_id: Uuid) -> String {
        self.snapshot
            .projects
            .iter()
            .find_map(|project| {
                project
                    .workspaces
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|workspace| {
                        workspace
                            .tabs
                            .iter()
                            .flat_map(|tab| &tab.sessions)
                            .any(|session| session.id == pane_id)
                    })
                    .map(|workspace| {
                        if workspace.name == project.name {
                            project.name.clone()
                        } else {
                            format!("{} › {}", project.name, workspace.name)
                        }
                    })
            })
            .unwrap_or_default()
    }

    /// Shows a terminal from anywhere in the app and acknowledges its events.
    pub(super) fn open_pane(&mut self, pane_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if !self.snapshot.select_terminal_global(pane_id) {
            return;
        }
        self.inbox.mark_pane_read(pane_id);
        self.apply_workspace_selection_change(window, cx);
        self.pending_focus_session = Some(pane_id);
        cx.notify();
    }

    fn open_inbox_item(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.inbox.mark_read(id);
        if let Some(pane_id) = self.inbox.item(id).and_then(|item| item.pane_id) {
            self.open_pane(pane_id, window, cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn inbox_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let now = unix_now();
        let agents: Vec<_> = self
            .snapshot
            .terminal_sessions()
            .into_iter()
            .filter_map(|session| {
                self.resolved_agent_presence(session.id)
                    .map(|presence| (session.id, presence))
            })
            .collect();
        let unread = self.inbox.unread_count();
        let actions = vec![
            section_button("inbox-mark-read", "Marcar como leído", false)
                .when(unread == 0, |button| button.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.inbox.mark_all_read() {
                        cx.notify();
                    }
                }))
                .into_any_element(),
            section_button("inbox-clear", "Limpiar", false)
                .when(self.inbox.is_empty(), |button| button.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.inbox.clear() {
                        cx.notify();
                    }
                }))
                .into_any_element(),
        ];

        let mut body = div().flex().flex_col().gap_1().max_w(px(760.0)).w_full();
        body = body.child(section_heading(format!(
            "Agentes activos · {}",
            agents.len()
        )));
        if agents.is_empty() {
            body = body.child(
                div()
                    .px_3()
                    .pb_3()
                    .text_size(px(12.0))
                    .text_color(colors().subtle)
                    .child("Ningún agente corriendo. Abre Claude, Codex o cualquier CLI en una terminal y aparecerá aquí."),
            );
        }
        for (pane_id, presence) in agents {
            let location = self.session_location_label(pane_id);
            let label = agent_state_label(presence.state, presence.attention);
            let color = agent_status_color(Some(presence.state), presence.attention)
                .unwrap_or(colors().subtle);
            body = body.child(
                inbox_row(SharedString::from(format!("inbox-agent-{pane_id}")))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open_pane(pane_id, window, cx)),
                    )
                    .child(agent_compact_badge(
                        Some(presence.kind.as_str()),
                        None,
                        None,
                        false,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(colors().foreground)
                                    .truncate()
                                    .child(presence.kind.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(colors().subtle)
                                    .truncate()
                                    .child(location),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(px(12.0))
                            .text_color(colors().muted)
                            .child(div().size(px(6.0)).rounded_full().bg(color))
                            .child(label),
                    ),
            );
        }

        body = body.child(div().h(px(12.0)));
        body = body.child(section_heading(if unread > 0 {
            format!("Actividad · {unread} sin leer")
        } else {
            "Actividad".to_owned()
        }));
        if self.inbox.is_empty() {
            body = body.child(section_empty_state(
                "chrome-icons/inbox.svg",
                "Todo al día",
                "Cuando un agente termine, pida permiso o espere tu respuesta en una terminal que no estás mirando, lo verás aquí.",
            ));
        }
        for item in self.inbox.items() {
            body = body.child(self.inbox_item_row(item, now, cx));
        }

        section_frame("Inbox", actions, body.into_any_element())
    }

    fn inbox_item_row(&self, item: &InboxItem, now: u64, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id;
        let openable = item.pane_id.is_some();
        let icon = match item.kind {
            InboxKind::AutomationStarted | InboxKind::AutomationFailed => {
                "chrome-icons/automations.svg"
            }
            _ => "chrome-icons/inbox.svg",
        };
        inbox_row(SharedString::from(format!("inbox-item-{id}")))
            .when(!openable, |row| row.cursor_default())
            .on_click(cx.listener(move |this, _, window, cx| this.open_inbox_item(id, window, cx)))
            .child(
                div()
                    .size(px(8.0))
                    .flex_none()
                    .rounded_full()
                    .when(!item.read, |dot| dot.bg(colors().accent)),
            )
            .child(
                div()
                    .size(px(28.0))
                    .flex_none()
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(surface_tint(colors().elevated, colors().background))
                    .child(
                        svg()
                            .path(icon)
                            .size(px(14.0))
                            .text_color(inbox_kind_color(item.kind)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .when(!item.read, |title| {
                                title.font_weight(gpui::FontWeight::MEDIUM)
                            })
                            .text_color(if item.read {
                                colors().muted
                            } else {
                                colors().foreground
                            })
                            .truncate()
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(colors().subtle)
                            .truncate()
                            .child(if item.detail.is_empty() {
                                "Terminal cerrada".to_owned()
                            } else if openable {
                                item.detail.clone()
                            } else {
                                format!("{} · terminal cerrada", item.detail)
                            }),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(colors().subtle)
                    .child(relative_time(now, item.at)),
            )
            .into_any_element()
    }

    /// Unread count for the global navigation badge.
    pub(super) fn inbox_unread_badge(&self) -> Option<AnyElement> {
        let unread = self.inbox.unread_count();
        (unread > 0).then(|| {
            div()
                .min_w(px(20.0))
                .h(px(18.0))
                .px(px(6.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::Rgba {
                    a: 0.18,
                    ..colors().accent
                })
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(colors().accent)
                .child(if unread > 99 {
                    "99+".to_owned()
                } else {
                    unread.to_string()
                })
                .into_any_element()
        })
    }
}

fn inbox_row(id: SharedString) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w_full()
        .min_h(px(48.0))
        .px_3()
        .py(px(7.0))
        .rounded(px(8.0))
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
}
