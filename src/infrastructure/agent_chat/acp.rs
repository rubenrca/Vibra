//! ACP client over the agent's stdio (JSON-RPC 2.0, one message per line).
//!
//! A connection runs a few threads that share `Shared`: a stdout reader that
//! answers requests and emits events, a stdin writer fed by an outbox, a stderr
//! tail, a process waiter, and a loop that handles the view's commands. The
//! reader never waits on stdin or on a child process, so a stalled agent cannot
//! freeze incoming messages.

mod child;
mod mapping;
mod rpc;
mod terminals;

use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{ChildStderr, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::{Value, json};

use crate::domain::agent_chat::{
    ChatAgent, ChatCapabilities, ChatCommand, ChatEvent, PermissionOption, PermissionResolution,
    PromptFile,
};

use super::{AgentChatConnection, AgentLaunch, SessionTarget};
use child::{ExitSlot, join_before, signal_group, signal_name};
use mapping::{
    Controls, MODEL_OPTION_ID, Negotiated, SessionSupport, is_missing_session,
    parse_config_options, parse_initialize, permission_item, permission_outcome, prompt_blocks,
    requires_sign_in, session_update_events,
};
use rpc::{INVALID_PARAMS, Incoming, Outbox, RpcError, StreamEnd, lock};
use terminals::Terminals;

const PROTOCOL_VERSION: i64 = 1;
const STDERR_TAIL_BYTES: usize = 8 * 1024;
/// How long an agent gets to exit after SIGTERM before SIGKILL.
const EXIT_GRACE: Duration = Duration::from_millis(500);
/// Bound on the best-effort `session/close` and on waiting for stdin EOF to end the agent.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
/// Bound on joining I/O threads during teardown.
const JOIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Stops the agent process group and joins its I/O threads when dropped.
pub(crate) struct ConnectionGuard {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.shared.stop();
        let deadline = Instant::now() + JOIN_TIMEOUT;
        for thread in self.threads.drain(..) {
            join_before(thread, deadline);
        }
    }
}

pub(super) fn spawn(
    agent: ChatAgent,
    launch: AgentLaunch,
    cwd: PathBuf,
    target: SessionTarget,
) -> anyhow::Result<AgentChatConnection> {
    let mut child = Command::new(&launch.program)
        .args(&launch.args)
        // Vibra's own agent hooks key on these; a chat session belongs to no
        // terminal pane, even when Vibra itself was started from one.
        .env_remove("VIBRA_PANE_ID")
        .env_remove("VIBRA_AUTOMATION_SOCKET")
        .env_remove("VIBRA_CLI")
        .envs(launch.env.iter().map(|(name, value)| (name, value)))
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .with_context(|| format!("Could not start {}.", launch.program.display()))?;
    let pid = i32::try_from(child.id()).context("The agent process id is out of range.")?;
    let stdin = child
        .stdin
        .take()
        .context("The agent stdin is unavailable.")?;
    let stdout = child
        .stdout
        .take()
        .context("The agent stdout is unavailable.")?;
    let stderr = child
        .stderr
        .take()
        .context("The agent stderr is unavailable.")?;

    let (events_tx, events_rx) = async_channel::unbounded();
    let (commands_tx, commands_rx) = async_channel::unbounded();
    let (outbox, outbox_rx) = Outbox::new();
    let emitter = Arc::new(Emitter::new(events_tx));
    let exit = Arc::new(ExitSlot::default());
    let stderr_tail = Arc::new(StderrTail::default());
    let cwd = cwd.to_string_lossy().into_owned();
    let shared = Arc::new(Shared {
        agent,
        cwd: cwd.clone(),
        target,
        pid,
        outbox,
        emitter: emitter.clone(),
        exit: exit.clone(),
        stderr: stderr_tail.clone(),
        terminals: Terminals::new(PathBuf::from(&cwd), emitter),
        pending: Mutex::new(HashMap::new()),
        next_rpc_id: AtomicI64::new(1),
        session: Mutex::new(Session::default()),
    });

    let mut threads = vec![thread::spawn(move || rpc::write_messages(stdin, outbox_rx))];
    threads.push(thread::spawn(move || stderr_tail.capture(stderr)));
    threads.push(thread::spawn(move || exit.finish(child.wait().ok())));
    threads.push({
        let shared = shared.clone();
        thread::spawn(move || {
            let end = rpc::read_messages(BufReader::new(stdout), |message| {
                shared.handle_incoming(message);
            });
            shared.on_stream_end(end);
        })
    });
    {
        let shared = shared.clone();
        thread::spawn(move || {
            while let Ok(command) = commands_rx.recv_blocking() {
                if !shared.handle_command(command) {
                    break;
                }
            }
        });
    }

    shared.send_request(
        Pending::Initialize,
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": true,
            },
            "clientInfo": {"name": "vibra", "version": env!("CARGO_PKG_VERSION")},
        }),
    );

    Ok(AgentChatConnection {
        commands: commands_tx,
        events: events_rx,
        _guard: ConnectionGuard { shared, threads },
    })
}

/// Sends events to the view. `finish` sends the single final `Exited` and drops
/// anything emitted after it, so `Exited` is always last.
pub(super) struct Emitter {
    state: Mutex<EmitterState>,
}

struct EmitterState {
    events: async_channel::Sender<ChatEvent>,
    finished: bool,
}

impl Emitter {
    fn new(events: async_channel::Sender<ChatEvent>) -> Self {
        Self {
            state: Mutex::new(EmitterState {
                events,
                finished: false,
            }),
        }
    }

    pub(super) fn emit(&self, event: ChatEvent) {
        let state = lock(&self.state);
        if !state.finished {
            let _ = state.events.try_send(event);
        }
    }

    fn finish(&self, message: Option<String>) {
        let mut state = lock(&self.state);
        if !state.finished {
            state.finished = true;
            let _ = state.events.try_send(ChatEvent::Exited { message });
        }
    }
}

/// Requests we sent, keyed by JSON-RPC id, with what their response means.
enum Pending {
    Initialize,
    NewSession,
    LoadSession {
        session_id: String,
    },
    ResumeSession {
        session_id: String,
    },
    Prompt,
    SetMode {
        mode_id: String,
    },
    /// `option_id` names the option in an error message.
    SetConfigOption {
        option_id: String,
        value: String,
    },
    /// `session/set_model`, used for the synthetic model option.
    SetModel {
        model_id: String,
    },
    /// `done` wakes the shutdown path once the agent has answered (or failed).
    CloseSession {
        done: mpsc::Sender<()>,
    },
}

/// A permission request waiting for the view's answer.
struct PendingPermission {
    rpc_id: Value,
    options: Vec<PermissionOption>,
}

#[derive(Default)]
struct Session {
    capabilities: ChatCapabilities,
    support: SessionSupport,
    session_id: Option<String>,
    turn_active: bool,
    /// Set by `Shutdown`: the agent is being stopped on purpose.
    closing: bool,
    next_permission_id: u64,
    permissions: HashMap<u64, PendingPermission>,
    controls: Controls,
}

/// State shared by every thread of one connection.
struct Shared {
    agent: ChatAgent,
    cwd: String,
    target: SessionTarget,
    /// The agent's pid, which is also its process group id.
    pid: i32,
    outbox: Arc<Outbox>,
    emitter: Arc<Emitter>,
    exit: Arc<ExitSlot>,
    stderr: Arc<StderrTail>,
    terminals: Terminals,
    pending: Mutex<HashMap<i64, Pending>>,
    next_rpc_id: AtomicI64,
    session: Mutex<Session>,
}

impl Shared {
    fn send_request(&self, pending: Pending, method: &str, params: Value) {
        let id = self.next_rpc_id.fetch_add(1, Ordering::Relaxed);
        lock(&self.pending).insert(id, pending);
        self.outbox.send(rpc::request(id, method, params));
    }

    fn notify(&self, method: &str, params: Value) {
        self.outbox.send(rpc::notification(method, params));
    }

    fn respond(&self, id: &Value, result: Value) {
        self.outbox.send(rpc::result_response(id, result));
    }

    fn respond_error(&self, id: &Value, error: &RpcError) {
        self.outbox.send(rpc::error_response(id, error));
    }

    fn handle_incoming(&self, message: Incoming) {
        match message {
            Incoming::Response { id, result } => self.handle_response(&id, result),
            Incoming::Notification { method, params } => self.handle_notification(&method, &params),
            Incoming::Request { id, method, params } => self.handle_request(&id, &method, &params),
        }
    }

    fn handle_notification(&self, method: &str, params: &Value) {
        if method == "session/update"
            && let Some(update) = params.get("update")
        {
            let events = session_update_events(update, &mut lock(&self.session).controls);
            for event in events {
                self.emitter.emit(event);
            }
        }
    }

    fn handle_response(&self, id: &Value, result: Result<Value, RpcError>) {
        let Some(rpc_id) = id.as_i64() else {
            return;
        };
        let pending = lock(&self.pending).remove(&rpc_id);
        match pending {
            Some(Pending::Initialize) => self.on_initialized(result),
            Some(Pending::NewSession) => self.on_new_session(result),
            Some(Pending::LoadSession { session_id }) => self.on_load_session(session_id, result),
            Some(Pending::ResumeSession { session_id }) => {
                self.on_resume_session(session_id, result)
            }
            Some(Pending::Prompt) => self.on_prompt_finished(result),
            Some(Pending::SetMode { mode_id }) => self.on_set_mode(mode_id, result),
            Some(Pending::SetConfigOption { option_id, value }) => {
                self.on_set_config_option(option_id, value, result)
            }
            Some(Pending::SetModel { model_id }) => self.on_set_model(model_id, result),
            Some(Pending::CloseSession { done }) => {
                let _ = done.send(());
            }
            None => {}
        }
    }

    fn on_initialized(&self, result: Result<Value, RpcError>) {
        let value = match result {
            Ok(value) => value,
            Err(error) => return self.fail_setup(error),
        };
        let Negotiated {
            agent_name,
            capabilities,
            support,
            protocol_version,
        } = parse_initialize(&value);
        if protocol_version.is_some_and(|version| version != PROTOCOL_VERSION) {
            return self.end(Some(format!(
                "The {} agent uses ACP protocol version {}. Vibra supports version {PROTOCOL_VERSION}.",
                self.agent.label(),
                protocol_version.unwrap_or_default(),
            )));
        }
        {
            let mut session = lock(&self.session);
            session.capabilities = capabilities.clone();
            session.support = support;
        }
        self.emitter.emit(ChatEvent::Initialized {
            agent_name,
            capabilities: capabilities.clone(),
        });

        match self.target.clone() {
            SessionTarget::New => self.send_new(),
            SessionTarget::Load(session_id) => {
                // Prefer resume when the agent cannot replay history but can reattach.
                if !capabilities.load_session && support.resume {
                    self.send_resume(session_id);
                } else {
                    self.send_load(session_id);
                }
            }
        }
    }

    fn on_new_session(&self, result: Result<Value, RpcError>) {
        match result {
            Ok(value) => match value.get("sessionId").and_then(Value::as_str) {
                Some(session_id) => self.session_started(session_id.to_owned(), false, &value),
                None => self.end(Some(format!(
                    "The {} agent did not return a session id.",
                    self.agent.label()
                ))),
            },
            Err(error) => self.fail_setup(error),
        }
    }

    fn on_load_session(&self, session_id: String, result: Result<Value, RpcError>) {
        match result {
            Ok(value) => self.session_started(session_id, true, &value),
            // Method not found: this agent cannot load, so try resume if it has it.
            Err(error)
                if error.code == rpc::METHOD_NOT_FOUND && lock(&self.session).support.resume =>
            {
                self.send_resume(session_id)
            }
            Err(error) if is_missing_session(&error) => self.replace_missing_session(),
            Err(error) => self.fail_setup(error),
        }
    }

    fn on_resume_session(&self, session_id: String, result: Result<Value, RpcError>) {
        match result {
            Ok(value) => self.session_started(session_id, true, &value),
            Err(error) if is_missing_session(&error) => self.replace_missing_session(),
            Err(error) => self.fail_setup(error),
        }
    }

    fn send_new(&self) {
        self.send_request(
            Pending::NewSession,
            "session/new",
            json!({"cwd": self.cwd, "mcpServers": []}),
        );
    }

    /// The saved session is gone (for example a chat that never got a message,
    /// so the agent never wrote it). Start a new one and say so, rather than
    /// ending the chat.
    fn replace_missing_session(&self) {
        self.emitter.emit(ChatEvent::Notice {
            text: format!(
                "Previous {} session not found. Started a new one.",
                self.agent.label()
            ),
        });
        self.send_new();
    }

    fn send_load(&self, session_id: String) {
        let params = json!({"sessionId": session_id, "cwd": self.cwd, "mcpServers": []});
        self.send_request(Pending::LoadSession { session_id }, "session/load", params);
    }

    fn send_resume(&self, session_id: String) {
        let params = json!({"sessionId": session_id, "cwd": self.cwd, "mcpServers": []});
        self.send_request(
            Pending::ResumeSession { session_id },
            "session/resume",
            params,
        );
    }

    /// `setup` is the `session/new`, `load` or `resume` result. Its modes and
    /// config options go out right after `SessionStarted`.
    fn session_started(&self, session_id: String, loaded: bool, setup: &Value) {
        let controls = {
            let mut session = lock(&self.session);
            session.session_id = Some(session_id.clone());
            session.controls.setup(setup)
        };
        self.emitter
            .emit(ChatEvent::SessionStarted { session_id, loaded });
        for event in controls {
            self.emitter.emit(event);
        }
    }

    fn on_prompt_finished(&self, result: Result<Value, RpcError>) {
        lock(&self.session).turn_active = false;
        match result {
            Ok(value) => {
                let stop_reason = value
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                self.emitter.emit(ChatEvent::TurnFinished { stop_reason });
            }
            Err(error) => self.emitter.emit(ChatEvent::Error {
                message: format!("The turn failed: {}", error.message),
            }),
        }
    }

    /// A failed `initialize` or session call. A missing sign-in gets a hint to
    /// run the agent's CLI; any other failure ends the connection.
    fn fail_setup(&self, error: RpcError) {
        let label = self.agent.label();
        if requires_sign_in(&error) {
            let message =
                format!("Sign in to {label} in a terminal with its CLI, then reopen the chat.");
            self.emitter.emit(ChatEvent::Error {
                message: message.clone(),
            });
            self.end(Some(message));
        } else {
            let message = format!("The {label} chat could not start: {}", error.message);
            self.end(Some(self.with_stderr(message)));
        }
    }

    fn request_permission(&self, id: &Value, params: &Value) {
        let request_id = {
            let mut session = lock(&self.session);
            session.next_permission_id += 1;
            session.next_permission_id
        };
        let Some(item) = permission_item(params, request_id) else {
            return self.respond_error(
                id,
                &RpcError::new(
                    INVALID_PARAMS,
                    "session/request_permission needs a toolCall and options.",
                ),
            );
        };
        lock(&self.session).permissions.insert(
            request_id,
            PendingPermission {
                rpc_id: id.clone(),
                options: item.options.clone(),
            },
        );
        self.emitter.emit(ChatEvent::PermissionRequested(item));
    }

    fn handle_request(&self, id: &Value, method: &str, params: &Value) {
        let result = match method {
            "session/request_permission" => return self.request_permission(id, params),
            // Blocks until exit, so it runs on its own thread.
            "terminal/wait_for_exit" => return self.wait_for_terminal(id, params),
            "terminal/create" => self
                .terminals
                .create(params)
                .map(|terminal_id| json!({"terminalId": terminal_id})),
            "terminal/output" => self.terminals.output(params),
            "terminal/kill" => self.terminals.kill(params).map(|()| json!({})),
            "terminal/release" => {
                self.terminals.release(params);
                Ok(json!({}))
            }
            _ => Err(RpcError::method_not_found(method)),
        };
        match result {
            Ok(result) => self.respond(id, result),
            Err(error) => self.respond_error(id, &error),
        }
    }

    fn wait_for_terminal(&self, id: &Value, params: &Value) {
        let terminal = match self.terminals.lookup(params) {
            Ok(terminal) => terminal,
            Err(error) => return self.respond_error(id, &error),
        };
        let outbox = self.outbox.clone();
        let id = id.clone();
        thread::spawn(move || {
            outbox.send(rpc::result_response(&id, terminal.wait_for_exit()));
        });
    }

    fn handle_command(&self, command: ChatCommand) -> bool {
        match command {
            ChatCommand::Prompt { text, files } => self.prompt(text, files),
            ChatCommand::Cancel => self.cancel(),
            ChatCommand::SetMode { mode_id } => self.set_mode(mode_id),
            ChatCommand::SetConfigOption { option_id, value } => {
                self.set_config_option(option_id, value)
            }
            ChatCommand::ResolvePermission {
                request_id,
                resolution,
            } => self.resolve_permission(request_id, &resolution),
            ChatCommand::Shutdown => {
                self.shutdown();
                return false;
            }
        }
        true
    }

    fn prompt(&self, text: String, files: Vec<PromptFile>) {
        let session_id = {
            let mut session = lock(&self.session);
            if session.closing {
                Err("The chat is closing.")
            } else if session.turn_active {
                Err("A turn is already running. Wait for it to finish or cancel it.")
            } else if let Some(session_id) = session.session_id.clone() {
                session.turn_active = true;
                Ok(session_id)
            } else {
                Err("The chat session is not ready yet.")
            }
        };
        match session_id {
            Ok(session_id) => self.send_request(
                Pending::Prompt,
                "session/prompt",
                json!({"sessionId": session_id, "prompt": prompt_blocks(&text, &files)}),
            ),
            Err(message) => self.emitter.emit(ChatEvent::Error {
                message: message.to_owned(),
            }),
        }
    }

    /// The session id for a call that needs an open session.
    fn ready_session_id(&self) -> Result<String, &'static str> {
        lock(&self.session)
            .session_id
            .clone()
            .ok_or("The chat session is not ready yet.")
    }

    /// `session/set_mode`. The view's mode changes when the agent answers.
    fn set_mode(&self, mode_id: String) {
        match self.ready_session_id() {
            Ok(session_id) => self.send_request(
                Pending::SetMode {
                    mode_id: mode_id.clone(),
                },
                "session/set_mode",
                json!({"sessionId": session_id, "modeId": mode_id}),
            ),
            Err(message) => self.emitter.emit(ChatEvent::Error {
                message: message.to_owned(),
            }),
        }
    }

    /// `session/set_config_option`, or `session/set_model` for the synthetic
    /// model option.
    fn set_config_option(&self, option_id: String, value: String) {
        let session_id = match self.ready_session_id() {
            Ok(session_id) => session_id,
            Err(message) => {
                return self.emitter.emit(ChatEvent::Error {
                    message: message.to_owned(),
                });
            }
        };
        if option_id == MODEL_OPTION_ID {
            self.send_request(
                Pending::SetModel {
                    model_id: value.clone(),
                },
                "session/set_model",
                json!({"sessionId": session_id, "modelId": value}),
            );
        } else {
            self.send_request(
                Pending::SetConfigOption {
                    option_id: option_id.clone(),
                    value: value.clone(),
                },
                "session/set_config_option",
                json!({"sessionId": session_id, "configId": option_id, "value": value}),
            );
        }
    }

    fn on_set_mode(&self, mode_id: String, result: Result<Value, RpcError>) {
        match result {
            Ok(_) => {
                let event = lock(&self.session).controls.set_current_mode(&mode_id);
                self.emitter.emit(event);
            }
            Err(error) => self.emitter.emit(ChatEvent::Error {
                message: format!("The agent could not change the mode: {}", error.message),
            }),
        }
    }

    /// Uses the options the agent sends back. Without them, the new value is
    /// applied locally.
    fn on_set_config_option(
        &self,
        option_id: String,
        value: String,
        result: Result<Value, RpcError>,
    ) {
        match result {
            Ok(response) => {
                let event = {
                    let mut session = lock(&self.session);
                    match parse_config_options(&response) {
                        Some(options) => session.controls.replace_options(options),
                        None => session.controls.set_option_value(&option_id, &value),
                    }
                };
                self.emitter.emit(event);
            }
            Err(error) => {
                let label = lock(&self.session).controls.option_label(&option_id);
                self.emitter.emit(ChatEvent::Error {
                    message: format!("The agent could not change {label}: {}", error.message),
                });
            }
        }
    }

    fn on_set_model(&self, model_id: String, result: Result<Value, RpcError>) {
        match result {
            Ok(_) => {
                let event = lock(&self.session).controls.set_model(&model_id);
                self.emitter.emit(event);
            }
            Err(error) => self.emitter.emit(ChatEvent::Error {
                message: format!("The agent could not change the model: {}", error.message),
            }),
        }
    }

    /// Asks the agent to stop the turn, and answers every open permission as
    /// cancelled, as ACP requires.
    fn cancel(&self) {
        let (session_id, permissions) = {
            let mut session = lock(&self.session);
            let permissions: Vec<PendingPermission> = session
                .permissions
                .drain()
                .map(|(_, pending)| pending)
                .collect();
            (session.session_id.clone(), permissions)
        };
        if let Some(session_id) = session_id {
            self.notify("session/cancel", json!({"sessionId": session_id}));
        }
        for permission in permissions {
            self.respond(
                &permission.rpc_id,
                json!({"outcome": {"outcome": "cancelled"}}),
            );
        }
    }

    fn resolve_permission(&self, request_id: u64, resolution: &PermissionResolution) {
        let answer = {
            let mut session = lock(&self.session);
            let answer = match session.permissions.get(&request_id) {
                None => Err("That permission request is no longer waiting for an answer."),
                Some(pending) => permission_outcome(&pending.options, resolution)
                    .map(|outcome| (pending.rpc_id.clone(), outcome))
                    .ok_or("That option is not one the agent offered."),
            };
            if answer.is_ok() {
                session.permissions.remove(&request_id);
            }
            answer
        };
        match answer {
            Ok((rpc_id, outcome)) => self.respond(&rpc_id, json!({"outcome": outcome})),
            Err(message) => self.emitter.emit(ChatEvent::Error {
                message: message.to_owned(),
            }),
        }
    }

    /// Closes the session if the agent supports it, then ends the process.
    fn shutdown(&self) {
        let close_session = {
            let mut session = lock(&self.session);
            session.closing = true;
            if session.support.close {
                session.session_id.clone()
            } else {
                None
            }
        };
        if let Some(session_id) = close_session {
            let (done, closed) = mpsc::channel();
            self.send_request(
                Pending::CloseSession { done },
                "session/close",
                json!({"sessionId": session_id}),
            );
            let _ = closed.recv_timeout(CLOSE_TIMEOUT);
        }
        // Closing stdin lets a well-behaved agent exit on its own. stop() enforces it.
        self.outbox.close();
        self.exit.wait_timeout(CLOSE_TIMEOUT);
        self.stop();
    }

    fn on_stream_end(&self, end: StreamEnd) {
        let label = self.agent.label();
        let message = match end {
            StreamEnd::Malformed(error) => Some(self.with_stderr(format!(
                "The {label} agent sent a message Vibra could not read: {error}"
            ))),
            StreamEnd::Closed if lock(&self.session).closing => None,
            StreamEnd::Closed => {
                let status = self.exit.wait_timeout(EXIT_GRACE).flatten();
                match status {
                    Some(status) if status.success() => {
                        Some(format!("The {label} agent process exited."))
                    }
                    _ => Some(self.with_stderr(describe_exit(label, status))),
                }
            }
        };
        self.end(message);
    }

    /// Emits the final `Exited` (first call only), then stops the process.
    fn end(&self, message: Option<String>) {
        self.emitter.finish(message);
        self.stop();
    }

    /// Closes stdin, sends SIGTERM to the agent's group, waits briefly, then
    /// sends SIGKILL. Terminal processes in the same connection go down too.
    /// Safe to call more than once.
    fn stop(&self) {
        self.outbox.close();
        signal_group(self.pid, libc::SIGTERM);
        self.exit.wait_timeout(EXIT_GRACE);
        signal_group(self.pid, libc::SIGKILL);
        self.terminals.kill_all();
    }

    fn with_stderr(&self, message: String) -> String {
        let tail = self.stderr.text();
        if tail.is_empty() {
            message
        } else {
            format!("{message}\n\nAgent stderr:\n{tail}")
        }
    }
}

fn describe_exit(label: &str, status: Option<ExitStatus>) -> String {
    use std::os::unix::process::ExitStatusExt;

    match status {
        Some(status) => {
            if let Some(code) = status.code() {
                format!("The {label} agent process exited with code {code}.")
            } else if let Some(signal) = status.signal() {
                format!(
                    "The {label} agent process was stopped by {}.",
                    signal_name(signal)
                )
            } else {
                format!("The {label} agent process closed its output.")
            }
        }
        None => format!("The {label} agent process closed its output."),
    }
}

/// The last `STDERR_TAIL_BYTES` the agent wrote to stderr.
#[derive(Default)]
struct StderrTail {
    bytes: Mutex<Vec<u8>>,
}

impl StderrTail {
    fn capture(&self, mut stderr: ChildStderr) {
        let mut chunk = [0u8; 4096];
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let mut bytes = lock(&self.bytes);
                    bytes.extend_from_slice(&chunk[..read]);
                    if bytes.len() > STDERR_TAIL_BYTES {
                        let excess = bytes.len() - STDERR_TAIL_BYTES;
                        bytes.drain(..excess);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    }

    fn text(&self) -> String {
        let bytes = lock(&self.bytes);
        // The cut may land inside a UTF-8 character; skip its continuation bytes.
        let start = bytes
            .iter()
            .position(|byte| byte & 0xC0 != 0x80)
            .unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::domain::agent_chat::{
        ConfigCategory, ConfigOption, ConfigValue, ContextUsage, PermissionItem,
        PermissionOptionKind, SessionMode, SessionModes, SlashCommand, ToolCallItem,
        ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
    };

    use super::*;

    /// A tiny ACP agent. It answers the handshake and runs one prompt turn that
    /// sends a tool call, a permission request, a terminal, and a final update.
    /// Mode `stubborn` ignores SIGTERM and stdin EOF, so only the guard's SIGKILL ends it.
    const FAKE_AGENT: &str = r#"
import json, os, signal, sys, time

mode = sys.argv[1]
with open(sys.argv[2], "w") as pid_file:
    pid_file.write(str(os.getpid()))
if mode == "stubborn":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def read():
    line = sys.stdin.readline()
    if not line:
        return None
    return json.loads(line)


def update(session_id, payload):
    send({"jsonrpc": "2.0", "method": "session/update",
          "params": {"sessionId": session_id, "update": payload}})


# Mode `controls` keeps the session's mode and effort, as an agent would.
state = {"mode": "default", "effort": "high"}


def controls_options():
    return [
        {"id": "mode", "name": "Mode", "category": "mode", "type": "select",
         "currentValue": state["mode"],
         "options": [{"value": "default", "name": "Manual"}, {"value": "plan", "name": "Plan"}]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
         "currentValue": state["effort"],
         "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]},
    ]


while True:
    message = read()
    if message is None:
        if mode == "stubborn":
            while True:
                time.sleep(0.1)
        break
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params", {})
    if method == "initialize" and mode == "gone":
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "protocolVersion": 1,
            "agentInfo": {"name": "fake-agent", "version": "0.0.1"},
            "agentCapabilities": {"loadSession": True}}})
    elif method == "session/load" and mode == "gone":
        send({"jsonrpc": "2.0", "id": request_id, "error": {
            "code": -32603, "message": "Internal error",
            "data": {"details": "no rollout found for thread id old-1"}}})
    elif method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "protocolVersion": 1,
            "agentInfo": {"name": "fake-agent", "version": "0.0.1"},
            "agentCapabilities": {"loadSession": False,
                                  "promptCapabilities": {"image": False},
                                  "sessionCapabilities": {"close": {}}}}})
    elif method == "session/new" and mode == "controls":
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "sessionId": "sess-1",
            "modes": {"currentModeId": "default", "availableModes": [
                {"id": "default", "name": "Manual", "description": "Ask before changes"},
                {"id": "plan", "name": "Plan", "description": "Plan first"}]},
            "configOptions": controls_options(),
            "models": {"currentModelId": "m1", "availableModels": [
                {"modelId": "m1", "name": "Model One"},
                {"modelId": "m2", "name": "Model Two", "description": "Larger"}]}}})
        update("sess-1", {"sessionUpdate": "available_commands_update", "availableCommands": [
            {"name": "review", "description": "Review changes", "input": {"hint": "optional instructions"}},
            {"name": "compact", "description": "Summarize", "input": None}]})
        update("sess-1", {"sessionUpdate": "session_info_update", "title": "Fix the build",
                          "updatedAt": "2026-10-12T00:00:00Z"})
        update("sess-1", {"sessionUpdate": "usage_update", "used": 1200, "size": 200000})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "sess-1"}})
    elif method == "session/prompt" and mode == "controls":
        # Echo the prompt blocks so the test can check what went on the wire.
        update(params["sessionId"], {"sessionUpdate": "agent_message_chunk",
                                     "content": {"type": "text", "text": json.dumps(params["prompt"])}})
        send({"jsonrpc": "2.0", "id": request_id, "result": {"stopReason": "end_turn"}})
    elif method == "session/set_mode" and mode == "controls":
        if params["modeId"] == "bogus":
            send({"jsonrpc": "2.0", "id": request_id,
                  "error": {"code": -32602, "message": "Unknown mode bogus"}})
        else:
            state["mode"] = params["modeId"]
            update(params["sessionId"], {"sessionUpdate": "current_mode_update",
                                         "currentModeId": params["modeId"]})
            update(params["sessionId"], {"sessionUpdate": "config_option_update",
                                         "configOptions": controls_options()})
            send({"jsonrpc": "2.0", "id": request_id, "result": {}})
    elif method == "session/set_config_option" and mode == "controls":
        if params["configId"] == "nope":
            send({"jsonrpc": "2.0", "id": request_id,
                  "error": {"code": -32602, "message": "Unknown config option nope"}})
        elif params["configId"] == "effort":
            # No options in the answer: the client has to apply the value itself.
            state["effort"] = params["value"]
            send({"jsonrpc": "2.0", "id": request_id, "result": {}})
        else:
            state["mode"] = params["value"]
            send({"jsonrpc": "2.0", "id": request_id,
                  "result": {"configOptions": controls_options()}})
    elif method == "session/set_model" and mode == "controls":
        update(params["sessionId"], {"sessionUpdate": "config_option_update",
                                     "configOptions": controls_options()})
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
    elif method == "session/prompt":
        session_id = params["sessionId"]
        update(session_id, {"sessionUpdate": "agent_message_chunk",
                            "content": {"type": "text", "text": "Hello"}})
        update(session_id, {"sessionUpdate": "tool_call", "toolCallId": "tc-1",
                            "title": "Run echo", "kind": "execute", "status": "pending",
                            "content": [{"type": "content",
                                         "content": {"type": "text", "text": "Running echo"}}]})
        send({"jsonrpc": "2.0", "id": 1000, "method": "session/request_permission",
              "params": {"sessionId": session_id,
                         "toolCall": {"toolCallId": "tc-1", "title": "Run echo"},
                         "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                                     {"optionId": "deny", "name": "Deny", "kind": "reject_once"}]}})
        outcome = read()["result"]["outcome"]
        send({"jsonrpc": "2.0", "id": 1001, "method": "terminal/create",
              "params": {"sessionId": session_id, "command": "sh",
                         "args": ["-c", "echo hi"]}})
        terminal_id = read()["result"]["terminalId"]
        send({"jsonrpc": "2.0", "id": 1002, "method": "terminal/wait_for_exit",
              "params": {"sessionId": session_id, "terminalId": terminal_id}})
        read()
        update(session_id, {"sessionUpdate": "tool_call_update", "toolCallId": "tc-1",
                            "title": outcome.get("optionId", outcome["outcome"]),
                            "status": "completed",
                            "content": [{"type": "terminal", "terminalId": terminal_id}]})
        send({"jsonrpc": "2.0", "id": request_id, "result": {"stopReason": "end_turn"}})
    elif method == "session/close":
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
"#;

    fn next_event(events: &async_channel::Receiver<ChatEvent>) -> ChatEvent {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match events.try_recv() {
                Ok(event) => return event,
                Err(async_channel::TryRecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for a chat event"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(async_channel::TryRecvError::Closed) => panic!("event channel closed early"),
            }
        }
    }

    fn python3() -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join("python3"))
            .find(|candidate| candidate.is_file())
    }

    fn process_alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// Writes the fake agent into a scratch directory that is removed on drop.
    struct FakeAgent {
        dir: PathBuf,
        launch: AgentLaunch,
    }

    impl FakeAgent {
        fn new(name: &str, mode: &str, python: PathBuf) -> Self {
            let dir = std::env::temp_dir().join(format!("vibra-acp-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create the test directory");
            let script = dir.join("fake_agent.py");
            fs::write(&script, FAKE_AGENT).expect("write the fake agent");
            let launch = AgentLaunch {
                program: python,
                args: vec![
                    "-I".to_owned(),
                    script.display().to_string(),
                    mode.to_owned(),
                    dir.join("pid").display().to_string(),
                ],
                env: Vec::new(),
            };
            Self { dir, launch }
        }

        fn pid(&self) -> i32 {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(text) = fs::read_to_string(self.dir.join("pid"))
                    && let Ok(pid) = text.trim().parse()
                {
                    return pid;
                }
                assert!(
                    Instant::now() < deadline,
                    "the fake agent never wrote its pid"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for FakeAgent {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn wait_for_session(connection: &AgentChatConnection) {
        assert!(matches!(
            next_event(&connection.events),
            ChatEvent::Initialized { .. }
        ));
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::SessionStarted {
                session_id: "sess-1".to_owned(),
                loaded: false,
            }
        );
    }

    #[test]
    fn a_saved_session_that_is_gone_starts_a_new_one() {
        let Some(python) = python3() else {
            eprintln!("skipping: python3 is not installed");
            return;
        };
        let agent = FakeAgent::new("gone", "gone", python);
        let connection = spawn(
            ChatAgent::Codex,
            agent.launch.clone(),
            agent.dir.clone(),
            SessionTarget::Load("old-1".to_owned()),
        )
        .expect("spawn the fake agent");
        assert!(matches!(
            next_event(&connection.events),
            ChatEvent::Initialized { .. }
        ));
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::Notice {
                text: "Previous Codex session not found. Started a new one.".to_owned(),
            }
        );
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::SessionStarted {
                session_id: "sess-1".to_owned(),
                loaded: false,
            }
        );
    }

    #[test]
    fn fake_agent_runs_a_full_turn_and_shuts_down() {
        let Some(python) = python3() else {
            eprintln!("skipping: python3 is not installed");
            return;
        };
        let agent = FakeAgent::new("turn", "full", python);
        let connection = spawn(
            ChatAgent::Grok,
            agent.launch.clone(),
            agent.dir.clone(),
            SessionTarget::New,
        )
        .expect("spawn the fake agent");

        assert_eq!(
            next_event(&connection.events),
            ChatEvent::Initialized {
                agent_name: Some("fake-agent".to_owned()),
                capabilities: ChatCapabilities::default(),
            }
        );
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::SessionStarted {
                session_id: "sess-1".to_owned(),
                loaded: false,
            }
        );

        connection
            .commands
            .try_send(ChatCommand::Prompt {
                files: Vec::new(),
                text: "run echo".to_owned(),
            })
            .expect("send the prompt");
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::AgentMessageChunk {
                text: "Hello".to_owned(),
            }
        );
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::ToolCall(ToolCallItem {
                id: "tc-1".to_owned(),
                title: "Run echo".to_owned(),
                kind: ToolKind::Execute,
                status: ToolStatus::Pending,
                content: vec![ToolContent::Text("Running echo".to_owned())],
                locations: Vec::new(),
            })
        );
        let ChatEvent::PermissionRequested(permission) = next_event(&connection.events) else {
            panic!("expected a permission request");
        };
        assert_eq!(
            permission,
            PermissionItem {
                request_id: 1,
                tool_call_id: Some("tc-1".to_owned()),
                title: "Run echo".to_owned(),
                options: vec![
                    PermissionOption {
                        id: "allow".to_owned(),
                        name: "Allow".to_owned(),
                        kind: PermissionOptionKind::AllowOnce,
                    },
                    PermissionOption {
                        id: "deny".to_owned(),
                        name: "Deny".to_owned(),
                        kind: PermissionOptionKind::RejectOnce,
                    },
                ],
                resolution: None,
            }
        );

        // A second prompt while the turn is open must be refused locally.
        connection
            .commands
            .try_send(ChatCommand::Prompt {
                files: Vec::new(),
                text: "again".to_owned(),
            })
            .expect("send the second prompt");
        let ChatEvent::Error { message } = next_event(&connection.events) else {
            panic!("expected an error for the second prompt");
        };
        assert!(message.contains("already running"), "{message}");

        connection
            .commands
            .try_send(ChatCommand::ResolvePermission {
                request_id: permission.request_id,
                resolution: PermissionResolution::Selected("allow".to_owned()),
            })
            .expect("resolve the permission");

        let ChatEvent::TerminalOutput {
            terminal_id,
            output,
            exit_status,
        } = next_event(&connection.events)
        else {
            panic!("expected terminal output");
        };
        assert_eq!((output.as_str(), exit_status), ("hi\n", None));
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::TerminalOutput {
                terminal_id: terminal_id.clone(),
                output: String::new(),
                exit_status: Some(0),
            }
        );
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::ToolCallUpdate(ToolCallUpdate {
                id: "tc-1".to_owned(),
                title: Some("allow".to_owned()),
                status: Some(ToolStatus::Completed),
                content: Some(vec![ToolContent::Terminal {
                    terminal_id: terminal_id.clone(),
                }]),
                ..ToolCallUpdate::default()
            })
        );
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::TurnFinished {
                stop_reason: "end_turn".to_owned(),
            }
        );

        connection
            .commands
            .try_send(ChatCommand::Shutdown)
            .expect("send shutdown");
        assert_eq!(
            next_event(&connection.events),
            ChatEvent::Exited { message: None }
        );
    }

    fn config_value(value: &str, name: &str) -> ConfigValue {
        ConfigValue {
            value: value.to_owned(),
            name: name.to_owned(),
            description: None,
        }
    }

    /// The `mode` and `effort` options the `controls` fake agent reports.
    fn controls_options(mode: &str, effort: &str) -> Vec<ConfigOption> {
        vec![
            ConfigOption {
                id: "mode".to_owned(),
                name: "Mode".to_owned(),
                category: ConfigCategory::Mode,
                current: mode.to_owned(),
                values: vec![
                    config_value("default", "Manual"),
                    config_value("plan", "Plan"),
                ],
            },
            ConfigOption {
                id: "effort".to_owned(),
                name: "Effort".to_owned(),
                category: ConfigCategory::ThoughtLevel,
                current: effort.to_owned(),
                values: vec![config_value("low", "Low"), config_value("high", "High")],
            },
        ]
    }

    /// The `ConfigOptions` event for the agent's options plus the synthetic model
    /// option, which the fake agent reports through `models`.
    fn with_model(mut options: Vec<ConfigOption>, model: &str) -> ChatEvent {
        options.push(ConfigOption {
            id: "__model".to_owned(),
            name: "Model".to_owned(),
            category: ConfigCategory::Model,
            current: model.to_owned(),
            values: vec![
                config_value("m1", "Model One"),
                ConfigValue {
                    value: "m2".to_owned(),
                    name: "Model Two".to_owned(),
                    description: Some("Larger".to_owned()),
                },
            ],
        });
        ChatEvent::ConfigOptions(options)
    }

    fn next_error(events: &async_channel::Receiver<ChatEvent>) -> String {
        let ChatEvent::Error { message } = next_event(events) else {
            panic!("expected an error event");
        };
        message
    }

    #[test]
    fn fake_agent_session_controls_round_trip() {
        let Some(python) = python3() else {
            eprintln!("skipping: python3 is not installed");
            return;
        };
        let agent = FakeAgent::new("controls", "controls", python);
        let connection = spawn(
            ChatAgent::Codex,
            agent.launch.clone(),
            agent.dir.clone(),
            SessionTarget::New,
        )
        .expect("spawn the fake agent");
        let send = |command: ChatCommand| {
            connection
                .commands
                .try_send(command)
                .expect("send the command")
        };
        let events = &connection.events;

        assert!(matches!(next_event(events), ChatEvent::Initialized { .. }));
        assert_eq!(
            next_event(events),
            ChatEvent::SessionStarted {
                session_id: "sess-1".to_owned(),
                loaded: false,
            }
        );
        let modes = SessionModes {
            current: "default".to_owned(),
            available: vec![
                SessionMode {
                    id: "default".to_owned(),
                    name: "Manual".to_owned(),
                    description: Some("Ask before changes".to_owned()),
                },
                SessionMode {
                    id: "plan".to_owned(),
                    name: "Plan".to_owned(),
                    description: Some("Plan first".to_owned()),
                },
            ],
        };
        assert_eq!(next_event(events), ChatEvent::Modes(modes.clone()));
        assert_eq!(
            next_event(events),
            with_model(controls_options("default", "high"), "m1")
        );
        // Notifications the agent sends right after the session is set up.
        assert_eq!(
            next_event(events),
            ChatEvent::Commands(vec![
                SlashCommand {
                    name: "review".to_owned(),
                    description: "Review changes".to_owned(),
                    input_hint: Some("optional instructions".to_owned()),
                },
                SlashCommand {
                    name: "compact".to_owned(),
                    description: "Summarize".to_owned(),
                    input_hint: None,
                },
            ])
        );
        assert_eq!(
            next_event(events),
            ChatEvent::Title("Fix the build".to_owned())
        );
        assert_eq!(
            next_event(events),
            ChatEvent::Usage(ContextUsage {
                used: 1200,
                size: 200_000,
            })
        );

        // set_mode: the agent's notifications come first, then its answer.
        send(ChatCommand::SetMode {
            mode_id: "plan".to_owned(),
        });
        let planned = SessionModes {
            current: "plan".to_owned(),
            ..modes.clone()
        };
        assert_eq!(next_event(events), ChatEvent::Modes(planned.clone()));
        assert_eq!(
            next_event(events),
            with_model(controls_options("plan", "high"), "m1")
        );
        assert_eq!(next_event(events), ChatEvent::Modes(planned));

        send(ChatCommand::SetMode {
            mode_id: "bogus".to_owned(),
        });
        assert_eq!(
            next_error(events),
            "The agent could not change the mode: Unknown mode bogus"
        );

        // An answer without options: the new value is applied locally.
        send(ChatCommand::SetConfigOption {
            option_id: "effort".to_owned(),
            value: "low".to_owned(),
        });
        assert_eq!(
            next_event(events),
            with_model(controls_options("plan", "low"), "m1")
        );

        // An answer with the full option list replaces the options.
        send(ChatCommand::SetConfigOption {
            option_id: "mode".to_owned(),
            value: "default".to_owned(),
        });
        assert_eq!(
            next_event(events),
            with_model(controls_options("default", "low"), "m1")
        );

        send(ChatCommand::SetConfigOption {
            option_id: "nope".to_owned(),
            value: "x".to_owned(),
        });
        assert_eq!(
            next_error(events),
            "The agent could not change nope: Unknown config option nope"
        );

        // The model goes through session/set_model. The agent's config_option_update
        // leaves the model out, and the answer then sets it.
        send(ChatCommand::SetConfigOption {
            option_id: "__model".to_owned(),
            value: "m2".to_owned(),
        });
        assert_eq!(
            next_event(events),
            with_model(controls_options("default", "low"), "m1")
        );
        assert_eq!(
            next_event(events),
            with_model(controls_options("default", "low"), "m2")
        );

        // The prompt goes out as text plus a percent-encoded resource link.
        send(ChatCommand::Prompt {
            text: "look".to_owned(),
            files: vec![PromptFile {
                path: "/Users/me/My Project/main.rs".to_owned(),
                label: "main.rs".to_owned(),
            }],
        });
        let ChatEvent::AgentMessageChunk { text } = next_event(events) else {
            panic!("expected the echoed prompt");
        };
        assert_eq!(
            serde_json::from_str::<Value>(&text).expect("the prompt is JSON"),
            json!([
                {"type": "text", "text": "look"},
                {
                    "type": "resource_link",
                    "uri": "file:///Users/me/My%20Project/main.rs",
                    "name": "main.rs",
                },
            ])
        );
        assert_eq!(
            next_event(events),
            ChatEvent::TurnFinished {
                stop_reason: "end_turn".to_owned(),
            }
        );

        send(ChatCommand::Shutdown);
        assert_eq!(next_event(events), ChatEvent::Exited { message: None });
    }

    #[test]
    fn guard_kills_agent_that_ignores_stdin_close_and_sigterm() {
        let Some(python) = python3() else {
            eprintln!("skipping: python3 is not installed");
            return;
        };
        let agent = FakeAgent::new("guard", "stubborn", python);
        let connection = spawn(
            ChatAgent::Claude,
            agent.launch.clone(),
            agent.dir.clone(),
            SessionTarget::New,
        )
        .expect("spawn the fake agent");
        wait_for_session(&connection);
        let pid = agent.pid();
        assert!(
            process_alive(pid),
            "the agent should be running before the drop"
        );

        drop(connection);
        assert!(!process_alive(pid), "the guard left the agent running");
    }

    #[test]
    fn spawn_fails_when_the_program_is_missing() {
        let launch = AgentLaunch {
            program: PathBuf::from("/nonexistent/vibra-agent"),
            args: Vec::new(),
            env: Vec::new(),
        };
        let result = spawn(
            ChatAgent::Codex,
            launch,
            std::env::temp_dir(),
            SessionTarget::New,
        );
        assert!(result.is_err());
    }

    #[test]
    fn stderr_tail_text_skips_a_split_character() {
        let tail = StderrTail::default();
        {
            let mut bytes = lock(&tail.bytes);
            bytes.extend_from_slice(b"\xa9tail");
        }
        assert_eq!(tail.text(), "tail");
    }
}
