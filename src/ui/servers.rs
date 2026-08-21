use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    Context, EventEmitter, IntoElement, ParentElement, Render, SharedString, Stateful, Styled,
    Task, Timer, Window, div, prelude::*, px,
};
use uuid::Uuid;

use crate::infrastructure::process::{
    GroupedServer, ListenPort, group_listen_sockets, openable_http_url, path_is_under, process_cwd,
    scan_listen_sockets, scan_listen_sockets_under, terminate_pid,
};
use crate::ui::theme::colors;

const POLL_INTERVAL: Duration = Duration::from_millis(2_000);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRoot {
    pub pane_id: Uuid,
    pub label: String,
    pub pid: u32,
    pub cwd: PathBuf,
    pub project_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerProcess {
    pub pane_id: Uuid,
    pub pane_label: String,
    pub pid: u32,
    pub name: String,
    pub command: String,
    pub ports: Vec<ListenPort>,
}

impl ServerProcess {
    fn from_group(root: &ServerRoot, group: GroupedServer) -> Self {
        Self {
            pane_id: root.pane_id,
            pane_label: root.label.clone(),
            pid: group.pid,
            name: group.name,
            command: group.command,
            ports: group.ports,
        }
    }

    fn openable_url(&self) -> Option<String> {
        openable_http_url(&self.name, &self.ports)
    }
}

pub enum ServersViewEvent {
    FocusPane(Uuid),
}

pub struct ServersView {
    roots: Vec<ServerRoot>,
    servers: Vec<ServerProcess>,
    panel_visible: bool,
    refreshing: bool,
    error: Option<SharedString>,
    request_id: u64,
    _scan_task: Option<Task<()>>,
    _poll_task: Task<()>,
}

impl EventEmitter<ServersViewEvent> for ServersView {}

impl ServersView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let poll_task = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(POLL_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        if crate::ui::idle::should_poll_servers(this.panel_visible)
                            && !this.refreshing
                        {
                            this.refresh(false, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            roots: Vec::new(),
            servers: Vec::new(),
            panel_visible: false,
            refreshing: false,
            error: None,
            request_id: 0,
            _scan_task: None,
            _poll_task: poll_task,
        }
    }

    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    pub fn set_panel_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.panel_visible == visible {
            return;
        }
        self.panel_visible = visible;
        if visible && !self.refreshing {
            self.refresh(false, cx);
        }
    }

    pub fn set_roots(&mut self, roots: Vec<ServerRoot>, cx: &mut Context<Self>) {
        if self.roots == roots {
            return;
        }
        self.roots = roots;
        if self.panel_visible {
            self.refreshing = false;
            self.refresh(false, cx);
        }
    }

    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.refreshing = false;
        self.refresh(true, cx);
    }

    fn refresh(&mut self, notify_loading: bool, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        if notify_loading {
            self.error = None;
            cx.notify();
        }
        self.request_id = self.request_id.wrapping_add(1);
        let request_id = self.request_id;
        let roots = self.roots.clone();
        let task = cx.background_spawn(async move { scan_workspace_servers(&roots) });
        self._scan_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.request_id {
                    return;
                }
                this.refreshing = false;
                this.error = None;
                if this.servers != result {
                    this.servers = result;
                }
                cx.notify();
            });
        }));
    }

    fn header(&self) -> impl IntoElement {
        let count = self.servers.len();
        let meta = if self.refreshing && count == 0 {
            "…".to_owned()
        } else if count == 1 {
            "1 server".to_owned()
        } else {
            format!("{count} servers")
        };
        div()
            .w_full()
            .flex_none()
            .h(px(44.0))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(colors().panel)
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().foreground)
                    .child("Servers"),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(colors().muted)
                    .child(meta),
            )
    }

    fn empty_state(&self) -> impl IntoElement {
        let text = if self.refreshing {
            "Buscando procesos en escucha…"
        } else {
            "Ningún servidor en escucha. Cuando un agente arranque vite, next u otro, aparece aquí."
        };
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .p_8()
            .child(
                div()
                    .max_w(px(260.0))
                    .text_center()
                    .text_size(px(13.0))
                    .line_height(px(20.0))
                    .text_color(colors().subtle)
                    .child(text),
            )
    }

    fn cards(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let servers = self.servers.clone();
        div()
            .id("server-cards")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_y_scroll()
            .px_2()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .children(servers.into_iter().map(|server| self.card(server, cx)))
    }

    fn card(&self, server: ServerProcess, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let pane_id = server.pane_id;
        let pid = server.pid;
        let open_url = server.openable_url();
        let ports = server
            .ports
            .iter()
            .map(ListenPort::display_label)
            .collect::<Vec<_>>()
            .join("  ");
        let command = shorten_command(&server.name, &server.command);
        div()
            .id(SharedString::from(format!("server-card-{pid}")))
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_2()
            .rounded(px(8.0))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(colors().elevated)
            .cursor_pointer()
            .hover(|card| card.bg(colors().hover))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(ServersViewEvent::FocusPane(pane_id));
            }))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(7.0))
                            .flex_none()
                            .rounded_full()
                            .bg(colors().success),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().foreground)
                            .child(server.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family("JetBrains Mono")
                            .text_size(px(11.0))
                            .text_color(colors().accent)
                            .child(ports),
                    ),
            )
            .child(
                div()
                    .pl(px(15.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(10.5))
                    .text_color(colors().muted)
                    .child(format!("{} · pid {pid}", server.pane_label)),
            )
            .when(!command.is_empty(), |card| {
                card.child(
                    div()
                        .pl(px(15.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_family("JetBrains Mono")
                        .text_size(px(9.5))
                        .text_color(colors().subtle)
                        .child(command),
                )
            })
            .child(
                div()
                    .pl(px(15.0))
                    .pt(px(2.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(open_url, |row, url| {
                        row.child(action_button("Abrir", pid, false, cx, move |_this, cx| {
                            cx.open_url(&url);
                        }))
                    })
                    .child(action_button("Detener", pid, true, cx, move |this, cx| {
                        let _ = terminate_pid(pid);
                        this.refresh(true, cx);
                    })),
            )
    }
}

fn action_button(
    label: &'static str,
    pid: u32,
    danger: bool,
    cx: &mut Context<ServersView>,
    on_click: impl Fn(&mut ServersView, &mut Context<ServersView>) + 'static,
) -> impl IntoElement {
    let color = if danger {
        colors().danger
    } else {
        colors().foreground
    };
    div()
        .id(SharedString::from(format!("server-action-{label}-{pid}")))
        .h(px(22.0))
        .px_2()
        .rounded(px(6.0))
        .border_1()
        .border_color(colors().border_subtle)
        .bg(colors().panel)
        .flex()
        .items_center()
        .cursor_pointer()
        .hover(|button| button.bg(colors().selection))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            on_click(this, cx);
        }))
        .child(
            div()
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(color)
                .child(label),
        )
}

fn shorten_command(name: &str, command: &str) -> String {
    let command = command.trim();
    if command.is_empty() {
        return String::new();
    }
    let first = command.split_whitespace().next().unwrap_or(command);
    let first_base = std::path::Path::new(first)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(first);
    if first_base.eq_ignore_ascii_case(name) && command.split_whitespace().count() <= 1 {
        return String::new();
    }
    if command.chars().count() <= 72 {
        command.to_owned()
    } else {
        format!("{}…", command.chars().take(70).collect::<String>())
    }
}

fn scan_workspace_servers(roots: &[ServerRoot]) -> Vec<ServerProcess> {
    let mut assigned = HashSet::new();
    let mut servers = Vec::new();
    for root in roots {
        if root.pid == 0 {
            continue;
        }
        for group in group_listen_sockets(scan_listen_sockets(root.pid)) {
            if !assigned.insert(group.pid) {
                continue;
            }
            servers.push(ServerProcess::from_group(root, group));
        }
    }

    let mut dirs = Vec::new();
    for root in roots {
        for path in [&root.project_root, &root.cwd] {
            if path_is_under(path, path) && !dirs.iter().any(|existing| existing == path) {
                dirs.push(path.clone());
            }
        }
    }
    for group in group_listen_sockets(scan_listen_sockets_under(&dirs)) {
        if !assigned.insert(group.pid) {
            continue;
        }
        let cwd = process_cwd(group.pid);
        let Some(root) = matching_root(cwd.as_deref(), roots) else {
            continue;
        };
        servers.push(ServerProcess::from_group(root, group));
    }

    servers.sort_by(|left, right| {
        left.ports
            .first()
            .map(|port| port.port)
            .cmp(&right.ports.first().map(|port| port.port))
            .then(left.name.cmp(&right.name))
            .then(left.pid.cmp(&right.pid))
    });
    servers
}

fn matching_root<'a>(
    cwd: Option<&std::path::Path>,
    roots: &'a [ServerRoot],
) -> Option<&'a ServerRoot> {
    let cwd = cwd?;
    roots
        .iter()
        .filter(|root| path_is_under(cwd, &root.cwd) || path_is_under(cwd, &root.project_root))
        .max_by_key(|root| {
            let cwd_len = path_is_under(cwd, &root.cwd)
                .then_some(root.cwd.as_os_str().len())
                .unwrap_or(0);
            let project_len = path_is_under(cwd, &root.project_root)
                .then_some(root.project_root.as_os_str().len())
                .unwrap_or(0);
            cwd_len.max(project_len)
        })
}

impl Render for ServersView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.servers.is_empty();
        let error = self.error.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors().panel)
            .child(self.header())
            .when_some(error, |view, error| {
                view.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .bg(colors().diff_deleted_bg)
                        .border_b_1()
                        .border_color(colors().danger)
                        .text_size(px(9.0))
                        .text_color(colors().danger)
                        .child(error),
                )
            })
            .when(empty, |view| view.child(self.empty_state()))
            .when(!empty, |view| view.child(self.cards(cx)))
    }
}
