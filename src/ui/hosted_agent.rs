use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render, SharedString,
    Window, div, prelude::*, px,
};

use crate::domain::workspace::HostedAgentKind;
use crate::infrastructure::automation::AgentRuntimeState;
use crate::infrastructure::harness::{HostedConversation, HostedProcess, tui_resume_argv};
use crate::ports::harness::{HarnessEvent, HarnessPermissionRequest};
use crate::ui::theme::colors;

pub enum HostedAgentViewEvent {
    VendorSession {
        session_id: uuid::Uuid,
        vendor_session_id: String,
    },
    RuntimeState {
        session_id: uuid::Uuid,
        state: AgentRuntimeState,
        attention: Option<crate::infrastructure::automation::AgentAttention>,
    },
    OpenTui {
        #[allow(dead_code)]
        session_id: uuid::Uuid,
        agent: HostedAgentKind,
        vendor_session_id: String,
        working_directory: PathBuf,
    },
}

#[derive(Clone)]
enum TranscriptItem {
    User(String),
    Agent(String),
    Reasoning(String),
    Tools(Vec<TranscriptTool>),
    Error(String),
}

#[derive(Clone)]
struct TranscriptTool {
    id: String,
    summary: String,
    output: Option<String>,
}

pub struct HostedAgentView {
    session_id: uuid::Uuid,
    agent: HostedAgentKind,
    title: SharedString,
    working_directory: PathBuf,
    conversation: HostedConversation,
    process: Option<HostedProcess>,
    transcript: Vec<TranscriptItem>,
    draft: String,
    slash_selected: usize,
    model_selected: Option<String>,
    effort_selected: Option<String>,
    state: AgentRuntimeState,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl EventEmitter<HostedAgentViewEvent> for HostedAgentView {}

impl HostedAgentView {
    pub fn new(
        session_id: uuid::Uuid,
        agent: HostedAgentKind,
        title: impl Into<SharedString>,
        working_directory: PathBuf,
        vendor_session_id: Option<String>,
        environment: HashMap<String, String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut conversation = HostedConversation::new(agent, vendor_session_id.clone());
        let mut error = None;
        let process = match HostedProcess::spawn(
            agent,
            &working_directory,
            vendor_session_id.as_deref(),
            &mut conversation,
            environment,
        ) {
            Ok((process, rx)) => {
                cx.spawn(async move |this, cx| {
                    while let Ok(line) = rx.recv().await {
                        if this
                            .update(cx, |this, cx| this.ingest_line(&line, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
                Some(process)
            }
            Err(spawn_error) => {
                error = Some(spawn_error.to_string().into());
                None
            }
        };
        Self {
            session_id,
            agent,
            title: title.into(),
            working_directory,
            conversation,
            process,
            transcript: Vec::new(),
            draft: String::new(),
            slash_selected: 0,
            model_selected: None,
            effort_selected: None,
            state: AgentRuntimeState::Idle,
            error,
            focus_handle: cx.focus_handle(),
        }
    }

    #[allow(dead_code)]
    pub fn agent(&self) -> HostedAgentKind {
        self.agent
    }

    pub fn transcript_text(&self, lines: usize) -> String {
        let items: Vec<String> = self
            .transcript
            .iter()
            .map(|item| match item {
                TranscriptItem::User(text) => format!("user: {text}"),
                TranscriptItem::Agent(text) => format!("agent: {text}"),
                TranscriptItem::Reasoning(text) => format!("thinking: {text}"),
                TranscriptItem::Tools(tools) => tools
                    .iter()
                    .map(|tool| match &tool.output {
                        Some(output) => format!("tool {}: {output}", tool.summary),
                        None => format!("tool {}", tool.summary),
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                TranscriptItem::Error(text) => format!("error: {text}"),
            })
            .collect();
        items
            .into_iter()
            .rev()
            .take(lines)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn prompt(&mut self, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        self.submit_text(text, cx)
    }

    pub fn apply_launch_args(&mut self, args: &[String]) {
        self.conversation.apply_launch_args(args);
        if let Some(model) = launch_arg_value(args, &["-m", "--model"]) {
            self.model_selected = Some(model);
        }
        self.effort_selected = launch_effort(args).or_else(|| self.effort_selected.clone());
    }

    #[allow(dead_code)]
    pub fn runtime_state(&self) -> AgentRuntimeState {
        self.state
    }

    fn submit_text(&mut self, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("el mensaje está vacío".into());
        }
        if text.starts_with('/') {
            let name = text.trim_start_matches('/').split_whitespace().next();
            if let Some(name) = name
                && !self
                    .conversation
                    .advertised_commands()
                    .iter()
                    .any(|command| command == name)
                && !self.conversation.advertised_commands().is_empty()
            {
                return Err("ese comando no lo anunció el agente".into());
            }
        }
        let bytes = self
            .conversation
            .encode_user_turn(text)
            .map_err(|error| error.to_string())?;
        let Some(process) = &self.process else {
            return Err("el agente no está en ejecución".into());
        };
        process
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
        self.transcript.push(TranscriptItem::User(text.to_owned()));
        self.draft.clear();
        self.set_state(AgentRuntimeState::Working, None, cx);
        cx.notify();
        Ok(())
    }

    fn respond_permission(&mut self, option_id: &str, cx: &mut Context<Self>) {
        match self.conversation.encode_permission_reply(option_id) {
            Ok(bytes) => {
                if let Some(process) = &self.process
                    && let Err(error) = process.write_all(&bytes)
                {
                    self.transcript
                        .push(TranscriptItem::Error(error.to_string()));
                }
                self.set_state(AgentRuntimeState::Working, None, cx);
            }
            Err(error) => self
                .transcript
                .push(TranscriptItem::Error(error.to_string())),
        }
        cx.notify();
    }

    fn interrupt(&mut self, cx: &mut Context<Self>) {
        if let Ok(bytes) = self.conversation.encode_interrupt()
            && let Some(process) = &self.process
        {
            let _ = process.write_all(&bytes);
        }
        self.set_state(AgentRuntimeState::Idle, None, cx);
        cx.notify();
    }

    fn ingest_line(&mut self, line: &str, cx: &mut Context<Self>) {
        for event in self.conversation.ingest_line(line) {
            match event {
                HarnessEvent::Text(text) => self.push_agent_text(text),
                HarnessEvent::Reasoning(text) => {
                    self.transcript.push(TranscriptItem::Reasoning(text))
                }
                HarnessEvent::ToolCall { id, summary } => {
                    let tool = TranscriptTool {
                        id,
                        summary,
                        output: None,
                    };
                    if let Some(TranscriptItem::Tools(tools)) = self.transcript.last_mut() {
                        tools.push(tool);
                    } else {
                        self.transcript.push(TranscriptItem::Tools(vec![tool]));
                    }
                }
                HarnessEvent::ToolResult { id, output } => {
                    for item in self.transcript.iter_mut().rev() {
                        let TranscriptItem::Tools(tools) = item else {
                            continue;
                        };
                        if let Some(tool) = tools.iter_mut().rev().find(|tool| tool.id == id) {
                            tool.output = output;
                            break;
                        }
                    }
                }
                HarnessEvent::PermissionRequest(request) => {
                    let _ = request;
                    self.set_state(
                        AgentRuntimeState::Waiting,
                        Some(crate::infrastructure::automation::AgentAttention::Permission),
                        cx,
                    );
                }
                HarnessEvent::Question { prompt, .. } => {
                    self.transcript.push(TranscriptItem::Agent(prompt));
                    self.set_state(
                        AgentRuntimeState::Waiting,
                        Some(crate::infrastructure::automation::AgentAttention::Question),
                        cx,
                    );
                }
                HarnessEvent::Commands(_) => {}
                HarnessEvent::Done { vendor_session_id } => {
                    if let Some(id) = vendor_session_id {
                        cx.emit(HostedAgentViewEvent::VendorSession {
                            session_id: self.session_id,
                            vendor_session_id: id,
                        });
                    }
                    self.set_state(AgentRuntimeState::Idle, None, cx);
                }
            }
        }
        if let Some(id) = self.conversation.vendor_session_id() {
            cx.emit(HostedAgentViewEvent::VendorSession {
                session_id: self.session_id,
                vendor_session_id: id.to_owned(),
            });
        }
        cx.notify();
    }

    fn push_agent_text(&mut self, text: String) {
        if let Some(TranscriptItem::Agent(existing)) = self.transcript.last_mut() {
            existing.push_str(&text);
        } else {
            self.transcript.push(TranscriptItem::Agent(text));
        }
    }

    fn set_state(
        &mut self,
        state: AgentRuntimeState,
        attention: Option<crate::infrastructure::automation::AgentAttention>,
        cx: &mut Context<Self>,
    ) {
        self.state = state;
        cx.emit(HostedAgentViewEvent::RuntimeState {
            session_id: self.session_id,
            state,
            attention,
        });
    }

    fn open_tui(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.conversation.vendor_session_id() else {
            return;
        };
        if tui_resume_argv(self.agent, id).is_none() {
            return;
        }
        cx.emit(HostedAgentViewEvent::OpenTui {
            session_id: self.session_id,
            agent: self.agent,
            vendor_session_id: id.to_owned(),
            working_directory: self.working_directory.clone(),
        });
    }

    fn matching_commands(&self) -> Vec<String> {
        let query = self
            .draft
            .strip_prefix('/')
            .unwrap_or("")
            .split_whitespace()
            .next()
            .unwrap_or("");
        self.conversation
            .advertised_commands()
            .iter()
            .filter(|command| query.is_empty() || command.starts_with(query))
            .cloned()
            .collect()
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.to_ascii_lowercase();
        if self.conversation.pending_permission().is_some() {
            cx.stop_propagation();
            return;
        }
        let commands = self.matching_commands();
        if self.draft.starts_with('/') && !commands.is_empty() {
            match key.as_str() {
                "up" | "arrowup" => {
                    self.slash_selected = self.slash_selected.saturating_sub(1);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                "down" | "arrowdown" => {
                    if self.slash_selected + 1 < commands.len() {
                        self.slash_selected += 1;
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                "tab" => {
                    if let Some(command) = commands.get(self.slash_selected) {
                        self.draft = format!("/{command} ");
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }
        match key.as_str() {
            "enter" | "return" if event.keystroke.modifiers.shift => {
                self.draft.push('\n');
            }
            "enter" | "return" => {
                if self.state == AgentRuntimeState::Working {
                    self.interrupt(cx);
                } else {
                    let draft = self.draft.clone();
                    let _ = self.submit_text(&draft, cx);
                }
            }
            "backspace" => {
                self.draft.pop();
            }
            _ if !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    self.draft.push_str(text);
                }
            }
            _ => {}
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Focusable for HostedAgentView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HostedAgentView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_open_tui = self
            .conversation
            .vendor_session_id()
            .and_then(|id| tui_resume_argv(self.agent, id))
            .is_some();
        let permission = self.conversation.pending_permission().cloned();
        let models = self.conversation.advertised_models().to_vec();
        let commands = if self.draft.starts_with('/') {
            self.matching_commands()
        } else {
            Vec::new()
        };
        let selected = self.slash_selected;
        let working = self.state == AgentRuntimeState::Working;

        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .bg(colors().background)
            .child(self.render_header(can_open_tui, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(760.0))
                            .h_full()
                            .flex()
                            .flex_col()
                            .px(px(32.0))
                            .child(self.render_transcript())
                            .when_some(permission, |column, request| {
                                column.child(self.render_permission(&request, cx))
                            })
                            .when(!commands.is_empty(), |column| {
                                column.child(self.render_slash_list(&commands, selected, cx))
                            })
                            .child(self.render_composer(
                                working,
                                permission_blocks(&self.conversation),
                                &models,
                                cx,
                            ))
                            .when_some(self.error.clone(), |column, error| {
                                column.child(
                                    div()
                                        .pt_1()
                                        .pb_3()
                                        .text_size(px(12.0))
                                        .text_color(colors().danger)
                                        .child(error),
                                )
                            }),
                    ),
            )
    }
}

fn permission_blocks(conversation: &HostedConversation) -> bool {
    conversation.pending_permission().is_some()
}

impl HostedAgentView {
    fn render_header(&self, can_open_tui: bool, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(48.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .px(px(24.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(920.0))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().muted)
                            .child(self.title.clone()),
                    )
                    .when(can_open_tui, |header| {
                        header.child(
                            div()
                                .id("open-tui")
                                .px_3()
                                .py(px(5.0))
                                .rounded(px(999.0))
                                .border_1()
                                .border_color(colors().border_subtle)
                                .text_size(px(11.5))
                                .text_color(colors().muted)
                                .cursor_pointer()
                                .hover(|style| {
                                    style.bg(colors().hover).text_color(colors().foreground)
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.open_tui(cx)))
                                .child("Abrir TUI"),
                        )
                    }),
            )
    }

    fn render_transcript(&self) -> impl IntoElement {
        let items = self.transcript.clone();
        div()
            .id("hosted-transcript")
            .flex_1()
            .min_h(px(0.0))
            .pt(px(36.0))
            .pb(px(24.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .justify_end()
            .gap(px(24.0))
            .when(items.is_empty(), |empty| {
                empty.justify_center().child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(18.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors().foreground)
                                .child("¿En qué trabajamos?"),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(colors().subtle)
                                .child("Escribe un mensaje para comenzar"),
                        ),
                )
            })
            .children(items.into_iter().map(|item| {
                match item {
                    TranscriptItem::User(text) => user_bubble(text),
                    TranscriptItem::Agent(text) => assistant_copy(text),
                    TranscriptItem::Reasoning(_) => activity_chip("Thinking"),
                    TranscriptItem::Tools(tools) => {
                        activity_chip(&command_activity_label(tools.len()))
                    }
                    TranscriptItem::Error(text) => div()
                        .text_size(px(13.0))
                        .text_color(colors().danger)
                        .child(text)
                        .into_any_element(),
                }
            }))
    }

    fn render_permission(
        &self,
        request: &HarnessPermissionRequest,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .mb_3()
            .p_3()
            .rounded(px(16.0))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(colors().elevated)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(colors().foreground)
                    .child(request.summary.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .children(request.options.iter().map(|option| {
                        let id = option.id.clone();
                        div()
                            .id(SharedString::from(format!("perm-{}", option.id)))
                            .px_3()
                            .py_1()
                            .rounded(px(999.0))
                            .bg(colors().selection)
                            .cursor_pointer()
                            .hover(|style| style.bg(colors().hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond_permission(&id, cx);
                            }))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(colors().foreground)
                                    .child(option.label.clone()),
                            )
                    })),
            )
    }

    fn render_slash_list(
        &self,
        commands: &[String],
        selected: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .mb_2()
            .rounded(px(16.0))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(colors().elevated)
            .py_1()
            .children(commands.iter().enumerate().map(|(index, command)| {
                let command = command.clone();
                let label = command.clone();
                div()
                    .id(SharedString::from(format!("slash-{command}")))
                    .px_3()
                    .py_1()
                    .when(index == selected, |row| row.bg(colors().hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.draft = format!("/{command} ");
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(colors().foreground)
                            .child(format!("/{label}")),
                    )
            }))
    }

    fn render_composer(
        &self,
        working: bool,
        blocked: bool,
        models: &[String],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let draft = if self.draft.is_empty() {
            SharedString::from("Escribe algo…")
        } else {
            SharedString::from(self.draft.clone())
        };
        let model_label = self
            .model_selected
            .clone()
            .or_else(|| models.first().cloned())
            .unwrap_or_else(|| "Modelo predeterminado".to_owned());
        let effort_label = self
            .effort_selected
            .clone()
            .unwrap_or_else(|| "estándar".to_owned());
        let folder = self
            .working_directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("local");
        let disabled =
            blocked || self.process.is_none() || (!working && self.draft.trim().is_empty());

        div().flex_none().pb(px(24.0)).child(
            div()
                .id("hosted-composer")
                .track_focus(&self.focus_handle)
                .w_full()
                .min_h(px(88.0))
                .pl(px(18.0))
                .pr(px(10.0))
                .pt(px(14.0))
                .pb(px(9.0))
                .rounded(px(24.0))
                .border_1()
                .border_color(colors().border_subtle)
                .bg(colors().elevated)
                .flex()
                .flex_col()
                .gap_3()
                .when(!blocked, |composer| {
                    composer.capture_key_down(cx.listener(Self::on_key_down))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .text_color(if self.draft.is_empty() {
                            colors().subtle
                        } else {
                            colors().foreground
                        })
                        .child(draft),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(colors().subtle)
                                .child(format!("Local · {folder}")),
                        )
                        .child(self.render_model_control(model_label, effort_label, models, cx))
                        .child(self.render_send(working, disabled, cx)),
                ),
        )
    }

    fn render_send(
        &self,
        working: bool,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("hosted-send")
            .size(px(34.0))
            .rounded(px(999.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(if disabled {
                colors().selection
            } else {
                colors().foreground
            })
            .when(!disabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.88))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.state == AgentRuntimeState::Working {
                            this.interrupt(cx);
                        } else {
                            let draft = this.draft.clone();
                            let _ = this.submit_text(&draft, cx);
                        }
                    }))
            })
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(if disabled {
                        colors().subtle
                    } else {
                        colors().background
                    })
                    .child(if working { "■" } else { "↑" }),
            )
    }

    fn render_model_control(
        &self,
        model_label: String,
        effort_label: String,
        models: &[String],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let advertised = models.to_vec();
        div()
            .id("hosted-model")
            .max_w(px(220.0))
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py(px(4.0))
            .rounded(px(999.0))
            .text_size(px(10.5))
            .text_color(colors().subtle)
            .when(advertised.len() > 1, |control| {
                control
                    .cursor_pointer()
                    .hover(|style| style.bg(colors().hover).text_color(colors().muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if advertised.is_empty() {
                            return;
                        }
                        let current = this
                            .model_selected
                            .as_ref()
                            .and_then(|selected| {
                                advertised.iter().position(|model| model == selected)
                            })
                            .unwrap_or(0);
                        let model = advertised[(current + 1) % advertised.len()].clone();
                        this.model_selected = Some(model.clone());
                        this.conversation
                            .apply_launch_args(&["--model".into(), model]);
                        cx.notify();
                    }))
            })
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .child(format!("{model_label} · Esfuerzo {effort_label}")),
            )
            .when(models.len() > 1, |control| control.child("⌄"))
    }
}

fn user_bubble(text: String) -> gpui::AnyElement {
    div()
        .w_full()
        .flex()
        .justify_end()
        .child(
            div()
                .max_w(px(420.0))
                .px(px(16.0))
                .py(px(10.0))
                .rounded(px(18.0))
                .bg(colors().panel)
                .text_size(px(14.5))
                .line_height(px(21.0))
                .text_color(colors().foreground)
                .child(text),
        )
        .into_any_element()
}

fn assistant_copy(text: String) -> gpui::AnyElement {
    div()
        .w_full()
        .max_w(px(680.0))
        .text_size(px(14.5))
        .line_height(px(22.0))
        .text_color(colors().foreground)
        .child(text)
        .into_any_element()
}

fn activity_chip(label: &str) -> gpui::AnyElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(999.0))
                .border_1()
                .border_color(colors().border_subtle)
                .bg(colors().panel)
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .child(label.to_owned()),
        )
        .into_any_element()
}

fn command_activity_label(count: usize) -> String {
    if count == 1 {
        "Ran 1 command".to_owned()
    } else {
        format!("Ran {count} commands")
    }
}

fn launch_arg_value(args: &[String], flags: &[&str]) -> Option<String> {
    args.windows(2)
        .find_map(|pair| flags.contains(&pair[0].as_str()).then(|| pair[1].clone()))
}

fn launch_effort(args: &[String]) -> Option<String> {
    if let Some(value) = launch_arg_value(args, &["-c", "--effort", "--reasoning"]) {
        return Some(
            value
                .strip_prefix("model_reasoning_effort=")
                .unwrap_or(&value)
                .replace("x-high", "xhigh"),
        );
    }
    args.iter()
        .find(|arg| {
            matches!(
                arg.as_str(),
                "minimal" | "low" | "medium" | "high" | "xhigh" | "x-high"
            )
        })
        .map(|value| value.replace("x-high", "xhigh"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_activity_copy_is_compact() {
        assert_eq!(command_activity_label(1), "Ran 1 command");
        assert_eq!(command_activity_label(4), "Ran 4 commands");
    }

    #[test]
    fn composer_metadata_reads_launch_options() {
        let args = vec![
            "--model".to_owned(),
            "local-model".to_owned(),
            "--effort".to_owned(),
            "high".to_owned(),
        ];
        assert_eq!(
            launch_arg_value(&args, &["-m", "--model"]).as_deref(),
            Some("local-model")
        );
        assert_eq!(launch_effort(&args).as_deref(), Some("high"));
    }
}
