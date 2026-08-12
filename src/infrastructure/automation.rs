use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

const MAX_AUTOMATION_REQUEST_BYTES: u64 = 1024 * 1024;
const MAX_AUTOMATION_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_AGENT_HOOK_BYTES: u64 = 1024 * 1024;
const AUTOMATION_IO_TIMEOUT: Duration = Duration::from_secs(5);
static NEXT_AUTOMATION_SERVER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutomationDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRuntimeState {
    Idle,
    Working,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentKind {
    Aider,
    Amp,
    Claude,
    Codex,
    Cursor,
    Gemini,
    Grok,
    OpenCode,
    Pi,
}

impl AgentKind {
    pub const ALL: [Self; 9] = [
        Self::Aider,
        Self::Amp,
        Self::Claude,
        Self::Codex,
        Self::Cursor,
        Self::Gemini,
        Self::Grok,
        Self::OpenCode,
        Self::Pi,
    ];

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Aider => "Aider",
            Self::Amp => "Amp",
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Gemini => "Gemini",
            Self::Grok => "Grok",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
        }
    }

    pub const fn cli_name(self) -> &'static str {
        match self {
            Self::Aider => "aider",
            Self::Amp => "amp",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }

    pub const fn executable(self) -> &'static str {
        match self {
            Self::Aider => "aider",
            Self::Amp => "amp",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor-agent",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "aider" => Some(Self::Aider),
            "amp" => Some(Self::Amp),
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "cursor" | "cursor-agent" => Some(Self::Cursor),
            "gemini" => Some(Self::Gemini),
            "grok" => Some(Self::Grok),
            "opencode" => Some(Self::OpenCode),
            "pi" => Some(Self::Pi),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentPlacement {
    Current,
    #[default]
    Split,
    Tab,
}

const DEFAULT_AGENT_START_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_AGENT_WAIT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_AGENT_READ_LINES: usize = 80;
const AUTOMATION_RESPONSE_TIMEOUT: Duration = Duration::from_secs(330);

fn default_agent_start_timeout_ms() -> u64 {
    DEFAULT_AGENT_START_TIMEOUT_MS
}

fn default_agent_wait_timeout_ms() -> u64 {
    DEFAULT_AGENT_WAIT_TIMEOUT_MS
}

fn default_agent_read_lines() -> usize {
    DEFAULT_AGENT_READ_LINES
}

fn default_wait_true() -> bool {
    true
}

fn default_split_direction() -> AutomationDirection {
    AutomationDirection::Right
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentAttention {
    Permission,
    Question,
    Plan,
    Notification,
}

impl AgentAttention {
    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "permission" => Some(Self::Permission),
            "question" => Some(Self::Question),
            "plan" => Some(Self::Plan),
            "notification" => Some(Self::Notification),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Plan => "plan",
            Self::Notification => "notification",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum AutomationCommand {
    List,
    Send {
        text: String,
        #[serde(default)]
        newline: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane: Option<Uuid>,
    },
    Split {
        direction: AutomationDirection,
        #[serde(default)]
        no_focus: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
    },
    CreateTab {
        #[serde(default)]
        no_focus: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
    },
    Focus {
        direction: AutomationDirection,
    },
    Close,
    Zoom,
    AgentStatus {
        /// Pane UUID or agent name. Defaults to the calling pane.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    AgentList,
    AgentKinds,
    AgentStart {
        kind: AgentKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default = "default_agent_start_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "default_wait_true")]
        wait: bool,
        #[serde(default)]
        args: Vec<String>,
    },
    AgentOpen {
        kind: AgentKind,
        #[serde(default)]
        placement: AgentPlacement,
        #[serde(default = "default_split_direction")]
        direction: AutomationDirection,
        #[serde(default)]
        no_focus: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(default = "default_agent_start_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "default_wait_true")]
        wait: bool,
        #[serde(default)]
        args: Vec<String>,
    },
    AgentPrompt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        text: String,
        #[serde(default = "default_wait_true")]
        wait: bool,
        #[serde(default = "default_agent_wait_timeout_ms")]
        timeout_ms: u64,
        /// Empty means settled: idle or waiting.
        #[serde(default)]
        until: Vec<AgentRuntimeState>,
    },
    AgentWait {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default = "default_agent_wait_timeout_ms")]
        timeout_ms: u64,
        /// Empty means settled: idle or waiting.
        #[serde(default)]
        until: Vec<AgentRuntimeState>,
    },
    AgentRead {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default = "default_agent_read_lines")]
        lines: usize,
    },
    AgentRename {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        /// `None` clears the name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default)]
        clear: bool,
    },
    SetAgentState {
        state: AgentRuntimeState,
    },
    SetAgentPresence {
        kind: AgentKind,
        state: AgentRuntimeState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attention: Option<AgentAttention>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    ClearAgentPresence {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationEnvelope {
    pub pane_id: Uuid,
    pub token: Uuid,
    pub command: AutomationCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AutomationResponse {
    pub fn success(data: impl Into<Value>) -> Self {
        Self {
            ok: true,
            data: Some(data.into()),
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error.into()),
        }
    }
}

pub struct AutomationIncoming {
    pub envelope: AutomationEnvelope,
    pub response: mpsc::Sender<AutomationResponse>,
}

pub struct AutomationServer {
    path: PathBuf,
    receiver: async_channel::Receiver<AutomationIncoming>,
    stopped: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl AutomationServer {
    pub fn start() -> Result<Self> {
        let directory = automation_directory();
        fs::create_dir_all(&directory)
            .with_context(|| format!("no se pudo crear {}", directory.display()))?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let server_id = NEXT_AUTOMATION_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("{}-{server_id}.sock", std::process::id()));
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("no se pudo abrir {}", path.display()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let (sender, receiver) = async_channel::unbounded();
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_stopped = stopped.clone();
        let thread = thread::Builder::new()
            .name("vibra-automation".into())
            .spawn(move || {
                for connection in listener.incoming() {
                    if thread_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = connection else {
                        continue;
                    };
                    let sender = sender.clone();
                    let _ = thread::Builder::new()
                        .name("vibra-automation-client".into())
                        .spawn(move || handle_connection(stream, &sender));
                }
            })?;
        Ok(Self {
            path,
            receiver,
            stopped,
            thread: Some(thread),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn receiver(&self) -> async_channel::Receiver<AutomationIncoming> {
        self.receiver.clone()
    }
}

impl Drop for AutomationServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn handle_connection(mut stream: UnixStream, sender: &async_channel::Sender<AutomationIncoming>) {
    let mut request = String::new();
    let parsed = stream
        .set_read_timeout(Some(AUTOMATION_IO_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(AUTOMATION_IO_TIMEOUT)))
        .map_err(anyhow::Error::from)
        .and_then(|_| {
            Read::by_ref(&mut stream)
                .take(MAX_AUTOMATION_REQUEST_BYTES + 1)
                .read_to_string(&mut request)?;
            if request.len() as u64 > MAX_AUTOMATION_REQUEST_BYTES {
                bail!("la solicitud supera 1 MiB");
            }
            serde_json::from_str::<AutomationEnvelope>(&request).map_err(Into::into)
        });
    let response = match parsed {
        Ok(envelope) => {
            let (response_tx, response_rx) = mpsc::channel();
            if sender
                .send_blocking(AutomationIncoming {
                    envelope,
                    response: response_tx,
                })
                .is_err()
            {
                AutomationResponse::failure("Vibra se está cerrando")
            } else {
                response_rx
                    .recv_timeout(AUTOMATION_RESPONSE_TIMEOUT)
                    .unwrap_or_else(|_| AutomationResponse::failure("la UI no respondió a tiempo"))
            }
        }
        Err(error) => AutomationResponse::failure(format!("solicitud inválida: {error}")),
    };
    if let Ok(bytes) = serde_json::to_vec(&response) {
        let _ = stream.write_all(&bytes);
    }
}

fn automation_directory() -> PathBuf {
    let user = unsafe { libc::geteuid() };
    std::env::temp_dir().join(format!("vibra-{user}"))
}

pub fn run_cli(arguments: &[String]) -> Result<bool> {
    let Some(mode) = arguments.first().map(String::as_str) else {
        return Ok(false);
    };
    if mode == "agent" {
        run_agent_setup_cli(&arguments[1..])?;
        return Ok(true);
    }
    if mode == "+skill" {
        print!("{AUTOMATION_SKILL}");
        return Ok(true);
    }
    if !matches!(mode, "+pane" | "+agent") {
        return Ok(false);
    }
    let command = parse_cli_command(mode, &arguments[1..])?;
    if matches!(command, AutomationCommand::AgentKinds) {
        println!("{}", serde_json::to_string_pretty(&agent_kinds_payload())?);
        return Ok(true);
    }
    let socket = std::env::var_os("VIBRA_AUTOMATION_SOCKET")
        .map(PathBuf::from)
        .context("VIBRA_AUTOMATION_SOCKET no está disponible; ejecuta esto dentro de Vibra")?;
    let pane_id = std::env::var("VIBRA_PANE_ID")
        .context("falta VIBRA_PANE_ID")?
        .parse()?;
    let token = std::env::var("VIBRA_AUTOMATION_TOKEN")
        .context("falta VIBRA_AUTOMATION_TOKEN")?
        .parse()?;
    let envelope = AutomationEnvelope {
        pane_id,
        token,
        command,
    };
    let mut stream = UnixStream::connect(&socket)
        .with_context(|| format!("no se pudo conectar a {}", socket.display()))?;
    stream.write_all(&serde_json::to_vec(&envelope)?)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut response = Vec::new();
    stream
        .take(MAX_AUTOMATION_RESPONSE_BYTES + 1)
        .read_to_end(&mut response)?;
    if response.len() as u64 > MAX_AUTOMATION_RESPONSE_BYTES {
        bail!("la respuesta de automatización supera 4 MiB");
    }
    let response: AutomationResponse = serde_json::from_slice(&response)?;
    if !response.ok {
        bail!(
            response
                .error
                .unwrap_or_else(|| "automatización falló".into())
        );
    }
    if let Some(data) = response.data {
        if let Some(text) = data.as_str() {
            println!("{text}");
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
    }
    Ok(true)
}

pub fn agent_launch_command(kind: AgentKind, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(shell_quote(kind.executable()));
    for arg in args {
        parts.push(shell_quote(arg));
    }
    parts.join(" ")
}

pub fn agent_kinds_payload() -> Value {
    let kinds: Vec<_> = AgentKind::ALL
        .iter()
        .map(|kind| {
            serde_json::json!({
                "kind": kind.cli_name(),
                "executable": kind.executable(),
                "displayName": kind.display_name(),
            })
        })
        .collect();
    serde_json::json!({ "kinds": kinds })
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(ch, '-' | '_' | '.' | '/' | '=' | ':' | '@' | '+' | ',')
    }) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn parse_cli_command(mode: &str, arguments: &[String]) -> Result<AutomationCommand> {
    let operation = arguments.first().map(String::as_str).unwrap_or("status");
    if mode == "+agent" {
        return match operation {
            "status" => Ok(AutomationCommand::AgentStatus {
                target: parse_agent_target(&arguments[1..])?,
            }),
            "list" => Ok(AutomationCommand::AgentList),
            "kinds" => Ok(AutomationCommand::AgentKinds),
            "open" => parse_agent_open(&arguments[1..]),
            "start" => parse_agent_start(&arguments[1..]),
            "prompt" => parse_agent_prompt(&arguments[1..]),
            "wait" => parse_agent_wait(&arguments[1..]),
            "read" => parse_agent_read(&arguments[1..]),
            "rename" => parse_agent_rename(&arguments[1..]),
            "presence" => parse_agent_presence(&arguments[1..]),
            "attention" => parse_agent_attention(&arguments[1..]),
            "hook" => parse_agent_hook(&arguments[1..]),
            "clear" => Ok(AutomationCommand::ClearAgentPresence {
                session_id: parse_agent_flag(&arguments[1..], "--session")?,
            }),
            "idle" => Ok(AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Idle,
            }),
            "working" => Ok(AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Working,
            }),
            "waiting" => Ok(AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Waiting,
            }),
            _ => bail!(
                "uso: +agent [status|list|kinds|open|start|prompt|wait|read|rename|idle|working|waiting|presence|attention|hook|clear]"
            ),
        };
    }
    match operation {
        "list" | "status" => Ok(AutomationCommand::List),
        "send" | "run" => parse_pane_send_or_run(operation, &arguments[1..]),
        "split" => parse_pane_split(&arguments[1..]),
        "tab" => parse_pane_tab(&arguments[1..]),
        "focus" => {
            let direction = parse_direction(arguments.get(1).map(String::as_str))?;
            Ok(AutomationCommand::Focus { direction })
        }
        "close" => Ok(AutomationCommand::Close),
        "zoom" => Ok(AutomationCommand::Zoom),
        _ => bail!("uso: +pane [list|send|run|split|tab|focus|close|zoom]"),
    }
}

fn parse_pane_send_or_run(operation: &str, arguments: &[String]) -> Result<AutomationCommand> {
    let mut target_pane = None;
    let mut text_parts = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--pane" => {
                let value = arguments
                    .get(index + 1)
                    .context("falta valor para --pane")?;
                target_pane = Some(
                    value
                        .parse()
                        .with_context(|| format!("pane id inválido: {value}"))?,
                );
                index += 2;
            }
            "--" => {
                text_parts.extend(arguments[index + 1..].iter().cloned());
                break;
            }
            other => {
                text_parts.push(other.to_owned());
                index += 1;
            }
        }
    }
    let text = text_parts.join(" ");
    if text.is_empty() {
        bail!("uso: +pane {operation} [--pane <id>] <texto>");
    }
    Ok(AutomationCommand::Send {
        text,
        newline: operation == "run",
        target_pane,
    })
}

fn parse_pane_split(arguments: &[String]) -> Result<AutomationCommand> {
    let direction = parse_direction(arguments.first().map(String::as_str))?;
    let rest = if arguments.is_empty() {
        &[][..]
    } else {
        &arguments[1..]
    };
    let no_focus = rest.iter().any(|argument| argument == "--no-focus");
    let cwd = parse_agent_flag(rest, "--cwd")?.map(PathBuf::from);
    Ok(AutomationCommand::Split {
        direction,
        no_focus,
        cwd,
    })
}

fn parse_pane_tab(arguments: &[String]) -> Result<AutomationCommand> {
    let no_focus = arguments.iter().any(|argument| argument == "--no-focus");
    let cwd = parse_agent_flag(arguments, "--cwd")?.map(PathBuf::from);
    Ok(AutomationCommand::CreateTab { no_focus, cwd })
}

fn parse_agent_target(arguments: &[String]) -> Result<Option<String>> {
    if let Some(value) = parse_agent_flag(arguments, "--pane")? {
        return Ok(Some(value));
    }
    if let Some(value) = parse_agent_flag(arguments, "--name")? {
        return Ok(Some(value));
    }
    if let Some(value) = parse_agent_flag(arguments, "--target")? {
        return Ok(Some(value));
    }
    // Positional target when it is not a flag.
    Ok(arguments
        .first()
        .filter(|value| !value.starts_with('-'))
        .cloned())
}

fn parse_until_states(arguments: &[String]) -> Result<Vec<AgentRuntimeState>> {
    let mut states = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--until" {
            let value = arguments
                .get(index + 1)
                .map(String::as_str)
                .context("falta valor para --until")?;
            states.push(parse_agent_runtime_state(Some(value))?);
            index += 2;
            continue;
        }
        index += 1;
    }
    Ok(states)
}

fn parse_agent_prompt(arguments: &[String]) -> Result<AutomationCommand> {
    let (flags, trailing) = split_agent_args(arguments);
    let mut target = None;
    if let Some(value) = parse_agent_flag(&flags, "--pane")? {
        target = Some(value);
    } else if let Some(value) = parse_agent_flag(&flags, "--name")? {
        target = Some(value);
    } else if let Some(value) = parse_agent_flag(&flags, "--target")? {
        target = Some(value);
    }
    let mut wait = true;
    let mut timeout_ms = DEFAULT_AGENT_WAIT_TIMEOUT_MS;
    let until = parse_until_states(&flags)?;
    if let Some(timeout) = parse_agent_flag(&flags, "--timeout")? {
        timeout_ms = parse_timeout_ms(Some(timeout.as_str()))?;
    }
    let mut free = Vec::new();
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--pane" | "--name" | "--target" | "--until" | "--timeout" => index += 2,
            "--wait" => {
                wait = true;
                index += 1;
            }
            "--no-wait" => {
                wait = false;
                index += 1;
            }
            other if other.starts_with('-') => {
                bail!("flag desconocida en +agent prompt: {other}");
            }
            other => {
                free.push(other.to_owned());
                index += 1;
            }
        }
    }
    free.extend(trailing);
    // `+agent prompt reviewer Review the diff` → target + text when target not set by flags.
    if target.is_none() && free.len() >= 2 {
        target = Some(free.remove(0));
    }
    let text = free.join(" ");
    if text.is_empty() {
        bail!(
            "uso: +agent prompt [<target>] [--wait|--no-wait] [--timeout ms] [--until state] <texto>"
        );
    }
    Ok(AutomationCommand::AgentPrompt {
        target,
        text,
        wait,
        timeout_ms,
        until,
    })
}

fn parse_agent_wait(arguments: &[String]) -> Result<AutomationCommand> {
    let target = parse_agent_target(arguments)?;
    let mut timeout_ms = DEFAULT_AGENT_WAIT_TIMEOUT_MS;
    if let Some(timeout) = parse_agent_flag(arguments, "--timeout")? {
        timeout_ms = parse_timeout_ms(Some(timeout.as_str()))?;
    }
    let until = parse_until_states(arguments)?;
    Ok(AutomationCommand::AgentWait {
        target,
        timeout_ms,
        until,
    })
}

fn parse_agent_read(arguments: &[String]) -> Result<AutomationCommand> {
    let target = parse_agent_target(arguments)?;
    let mut lines = DEFAULT_AGENT_READ_LINES;
    if let Some(value) = parse_agent_flag(arguments, "--lines")? {
        lines = value
            .parse()
            .with_context(|| format!("--lines inválido: {value}"))?;
        if lines == 0 || lines > 2_000 {
            bail!("--lines debe estar entre 1 y 2000");
        }
    }
    Ok(AutomationCommand::AgentRead { target, lines })
}

fn parse_agent_rename(arguments: &[String]) -> Result<AutomationCommand> {
    let clear = arguments.iter().any(|argument| argument == "--clear");
    let target = parse_agent_target(arguments)?;
    let name = if clear {
        None
    } else {
        parse_agent_flag(arguments, "--to")?
            .or_else(|| {
                // `+agent rename <target> <name>` or `+agent rename <name>` for current
                let positional: Vec<_> = arguments
                    .iter()
                    .filter(|argument| !argument.starts_with('-'))
                    .cloned()
                    .collect();
                match positional.as_slice() {
                    [only] if target.as_ref() == Some(only) => None,
                    [only] => Some(only.clone()),
                    [first, second] if target.as_ref() == Some(first) => Some(second.clone()),
                    [_, second] => Some(second.clone()),
                    _ => None,
                }
            })
            .or_else(|| parse_agent_flag(arguments, "--name").ok().flatten())
    };
    if !clear && name.is_none() {
        bail!(
            "uso: +agent rename <target> <name> | +agent rename --name <alias> | +agent rename <target> --clear"
        );
    }
    if let Some(name) = name.as_ref() {
        validate_agent_name(name)?;
    }
    Ok(AutomationCommand::AgentRename {
        target,
        name,
        clear,
    })
}

pub fn validate_agent_name(name: &str) -> Result<()> {
    let valid = name.len() <= 32
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'));
    if !valid {
        bail!("nombre de agente inválido (use [a-z][a-z0-9_-]{{0,31}}): {name}");
    }
    Ok(())
}

/// Default settled states for waits (idle or waiting/blocked).
pub fn default_settled_states() -> Vec<AgentRuntimeState> {
    vec![AgentRuntimeState::Idle, AgentRuntimeState::Waiting]
}

fn parse_agent_open(arguments: &[String]) -> Result<AutomationCommand> {
    let kind = arguments
        .first()
        .and_then(|kind| AgentKind::parse(kind))
        .with_context(|| {
            format!(
                "kind esperado: {}",
                AgentKind::ALL
                    .iter()
                    .map(|kind| kind.cli_name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let (flags, args) = split_agent_args(&arguments[1..]);
    let mut placement = AgentPlacement::Split;
    let mut direction = AutomationDirection::Right;
    let mut no_focus = true;
    let mut name = None;
    let mut cwd = None;
    let mut timeout_ms = DEFAULT_AGENT_START_TIMEOUT_MS;
    let mut wait = true;
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--tab" => {
                placement = AgentPlacement::Tab;
                index += 1;
            }
            "--current" => {
                placement = AgentPlacement::Current;
                index += 1;
            }
            "--split" => {
                placement = AgentPlacement::Split;
                let value = flags
                    .get(index + 1)
                    .map(String::as_str)
                    .filter(|value| !value.starts_with("--"));
                if let Some(value) = value {
                    direction = parse_direction(Some(value))?;
                    index += 2;
                } else {
                    direction = AutomationDirection::Right;
                    index += 1;
                }
            }
            "--no-focus" => {
                no_focus = true;
                index += 1;
            }
            "--focus" => {
                no_focus = false;
                index += 1;
            }
            "--name" => {
                name = Some(
                    flags
                        .get(index + 1)
                        .cloned()
                        .context("falta valor para --name")?,
                );
                index += 2;
            }
            "--cwd" => {
                cwd = Some(PathBuf::from(
                    flags
                        .get(index + 1)
                        .cloned()
                        .context("falta valor para --cwd")?,
                ));
                index += 2;
            }
            "--timeout" => {
                timeout_ms = parse_timeout_ms(flags.get(index + 1).map(String::as_str))?;
                index += 2;
            }
            "--wait" => {
                wait = true;
                index += 1;
            }
            "--no-wait" => {
                wait = false;
                index += 1;
            }
            other => bail!("flag desconocida en +agent open: {other}"),
        }
    }
    if let Some(name) = name.as_ref() {
        validate_agent_name(name)?;
    }
    Ok(AutomationCommand::AgentOpen {
        kind,
        placement,
        direction,
        no_focus,
        name,
        cwd,
        timeout_ms,
        wait,
        args,
    })
}

fn parse_agent_start(arguments: &[String]) -> Result<AutomationCommand> {
    let (flags, args) = split_agent_args(arguments);
    let mut kind = None;
    let mut pane = None;
    let mut name = None;
    let mut timeout_ms = DEFAULT_AGENT_START_TIMEOUT_MS;
    let mut wait = true;
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--kind" => {
                let value = flags.get(index + 1).context("falta valor para --kind")?;
                kind = Some(
                    AgentKind::parse(value).with_context(|| format!("kind inválido: {value}"))?,
                );
                index += 2;
            }
            "--pane" => {
                let value = flags.get(index + 1).context("falta valor para --pane")?;
                pane = Some(
                    value
                        .parse()
                        .with_context(|| format!("pane id inválido: {value}"))?,
                );
                index += 2;
            }
            "--name" => {
                name = Some(
                    flags
                        .get(index + 1)
                        .cloned()
                        .context("falta valor para --name")?,
                );
                index += 2;
            }
            "--timeout" => {
                timeout_ms = parse_timeout_ms(flags.get(index + 1).map(String::as_str))?;
                index += 2;
            }
            "--wait" => {
                wait = true;
                index += 1;
            }
            "--no-wait" => {
                wait = false;
                index += 1;
            }
            other if kind.is_none() && AgentKind::parse(other).is_some() => {
                kind = AgentKind::parse(other);
                index += 1;
            }
            other => bail!("flag desconocida en +agent start: {other}"),
        }
    }
    let kind = kind.context("uso: +agent start --kind <kind> [--pane <id>] [--name <alias>]")?;
    if let Some(name) = name.as_ref() {
        validate_agent_name(name)?;
    }
    Ok(AutomationCommand::AgentStart {
        kind,
        pane,
        name,
        timeout_ms,
        wait,
        args,
    })
}

fn split_agent_args(arguments: &[String]) -> (Vec<String>, Vec<String>) {
    if let Some(index) = arguments.iter().position(|argument| argument == "--") {
        (arguments[..index].to_vec(), arguments[index + 1..].to_vec())
    } else {
        (arguments.to_vec(), Vec::new())
    }
}

fn parse_timeout_ms(value: Option<&str>) -> Result<u64> {
    let value = value.context("falta valor para --timeout")?;
    let timeout: u64 = value
        .parse()
        .with_context(|| format!("timeout inválido: {value}"))?;
    if !(3_000..=300_000).contains(&timeout) {
        bail!("--timeout debe estar entre 3000 y 300000 ms");
    }
    Ok(timeout)
}

const AUTOMATION_SKILL: &str = r#"# Vibra automation skill

Use these commands only when the user asks to open another agent, split panes, prompt another agent, or control Vibra from inside a pane.

## Preconditions

```bash
test -n "$VIBRA_CLI" && test -n "$VIBRA_AUTOMATION_SOCKET" && test -n "$VIBRA_PANE_ID"
```

If any is missing, say you are not running inside Vibra and stop.

## Open an agent

Default layout is a sibling split to the right without stealing focus:

```bash
"$VIBRA_CLI" +agent open codex --split right --no-focus --name reviewer
"$VIBRA_CLI" +agent open claude --tab --no-focus --cwd "$PWD"
"$VIBRA_CLI" +agent open codex --split down --name builder -- -m o3
```

Supported kinds: aider, amp, claude, codex, cursor, gemini, grok, opencode, pi.
Names must match `[a-z][a-z0-9_-]{0,31}` and be unique in the project.

```bash
"$VIBRA_CLI" +agent kinds
"$VIBRA_CLI" +agent list
```

Parse JSON from stdout. Do not invent pane IDs or names.

## Prompt, wait, and read

Targets are a pane UUID or a live agent name:

```bash
"$VIBRA_CLI" +agent prompt reviewer "Review the current diff" --wait --timeout 120000
"$VIBRA_CLI" +agent wait reviewer --until waiting --timeout 120000
"$VIBRA_CLI" +agent read reviewer --lines 120
"$VIBRA_CLI" +agent status reviewer
"$VIBRA_CLI" +agent rename <pane-id> --to reviewer
```

`--wait` (default on prompt) waits until the agent is settled (`idle` or `waiting`). Use `--until idle` / `--until waiting` / `--until working` to narrow it. `--no-wait` returns after submitting.

## Layout primitives

```bash
"$VIBRA_CLI" +pane split right --no-focus --cwd "$PWD"
"$VIBRA_CLI" +pane tab --no-focus
"$VIBRA_CLI" +pane run --pane <id> "npm test"
"$VIBRA_CLI" +agent start --kind codex --pane <id> --name reviewer
"$VIBRA_CLI" +pane list
```

## Rules

- Prefer `+agent open <kind> --name <alias>` over manual split+run unless the user wants a bare shell.
- Keep the user's focus with `--no-focus` unless they asked to switch.
- Do not close panes/tabs you did not create.
- Honor the agent kind the user named.
- Extra agent CLI args go after `--`.
- After a wait returns `waiting`, read the pane before deciding what to send.
"#;

fn parse_agent_presence(arguments: &[String]) -> Result<AutomationCommand> {
    let kind = arguments
        .first()
        .and_then(|kind| AgentKind::parse(kind))
        .context(
            "agent esperado: aider, amp, claude, codex, cursor, gemini, grok, opencode o pi",
        )?;
    let state = parse_agent_runtime_state(arguments.get(1).map(String::as_str))?;
    let attention = arguments
        .get(2)
        .filter(|value| !value.starts_with("--"))
        .and_then(|value| AgentAttention::parse(value))
        .or_else(|| (state == AgentRuntimeState::Waiting).then_some(AgentAttention::Notification));
    if arguments
        .get(2)
        .is_some_and(|value| !value.starts_with("--") && attention.is_none())
    {
        bail!("atención esperada: permission, question, plan o notification");
    }
    Ok(AutomationCommand::SetAgentPresence {
        kind,
        state,
        attention,
        session_id: parse_agent_flag(arguments, "--session")?,
    })
}

fn parse_agent_attention(arguments: &[String]) -> Result<AutomationCommand> {
    let kind = arguments
        .first()
        .and_then(|kind| AgentKind::parse(kind))
        .context("agent esperado después de attention")?;
    let attention = arguments
        .get(1)
        .and_then(|attention| AgentAttention::parse(attention))
        .unwrap_or(AgentAttention::Notification);
    Ok(AutomationCommand::SetAgentPresence {
        kind,
        state: AgentRuntimeState::Waiting,
        attention: Some(attention),
        session_id: parse_agent_flag(arguments, "--session")?,
    })
}

fn parse_agent_hook(arguments: &[String]) -> Result<AutomationCommand> {
    let kind = arguments
        .first()
        .and_then(|kind| AgentKind::parse(kind))
        .context("uso: +agent hook <claude|codex> <evento>")?;
    let event = arguments
        .get(1)
        .map(String::as_str)
        .context("falta evento de hook")?;
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_AGENT_HOOK_BYTES + 1)
        .read_to_string(&mut input)
        .context("no se pudo leer el JSON del hook")?;
    if input.len() as u64 > MAX_AGENT_HOOK_BYTES {
        bail!("el payload del hook supera 1 MiB");
    }
    let payload = if input.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&input).context("JSON de hook inválido")?
    };
    agent_hook_command(kind, event, &payload)
}

fn agent_hook_command(kind: AgentKind, event: &str, payload: &Value) -> Result<AutomationCommand> {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let presence = |state, attention| AutomationCommand::SetAgentPresence {
        kind,
        state,
        attention,
        session_id: session_id.clone(),
    };
    match (kind, event) {
        (AgentKind::Claude | AgentKind::Codex, "session-end") => {
            Ok(AutomationCommand::ClearAgentPresence { session_id })
        }
        (AgentKind::Claude | AgentKind::Codex, "prompt") => {
            Ok(presence(AgentRuntimeState::Working, None))
        }
        (AgentKind::Claude | AgentKind::Codex, "stop" | "session-start") => {
            Ok(presence(AgentRuntimeState::Idle, None))
        }
        (AgentKind::Claude | AgentKind::Codex, "permission") => Ok(presence(
            AgentRuntimeState::Waiting,
            Some(AgentAttention::Permission),
        )),
        (AgentKind::Claude, "notification") => {
            match payload.get("notification_type").and_then(Value::as_str) {
                Some("permission_prompt") => Ok(presence(
                    AgentRuntimeState::Waiting,
                    Some(AgentAttention::Permission),
                )),
                Some("idle_prompt") => Ok(presence(AgentRuntimeState::Idle, None)),
                _ => bail!("notificación de Claude no soportada"),
            }
        }
        (AgentKind::Claude | AgentKind::Codex, _) => bail!("evento de hook no soportado: {event}"),
        _ => bail!("Vibra todavía no incluye hooks para {}", kind.cli_name()),
    }
}

fn parse_agent_runtime_state(value: Option<&str>) -> Result<AgentRuntimeState> {
    match value {
        Some("idle") => Ok(AgentRuntimeState::Idle),
        Some("working") => Ok(AgentRuntimeState::Working),
        Some("waiting") => Ok(AgentRuntimeState::Waiting),
        _ => bail!("estado esperado: idle, working o waiting"),
    }
}

fn parse_agent_flag(arguments: &[String], flag: &str) -> Result<Option<String>> {
    let Some(index) = arguments.iter().position(|argument| argument == flag) else {
        return Ok(None);
    };
    arguments
        .get(index + 1)
        .cloned()
        .map(Some)
        .with_context(|| format!("falta valor para {flag}"))
}

const CLAUDE_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Managed by Vibra. This is deliberately a no-op outside a Vibra pane.
[ -n "$VIBRA_CLI" ] && [ -n "$VIBRA_AUTOMATION_SOCKET" ] && [ -n "$VIBRA_PANE_ID" ] || exit 0
"$VIBRA_CLI" +agent hook claude "$1" >/dev/null 2>&1 || true
exit 0
"#;

const CODEX_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Managed by Vibra. Never fail a Codex hook: PermissionRequest treats errors as policy input.
[ -n "$VIBRA_CLI" ] && [ -n "$VIBRA_AUTOMATION_SOCKET" ] && [ -n "$VIBRA_PANE_ID" ] || exit 0
"$VIBRA_CLI" +agent hook codex "$1" >/dev/null 2>&1 || true
exit 0
"#;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentHookStatus {
    pub claude_installed: bool,
    pub codex_installed: bool,
}

impl AgentHookStatus {
    pub const fn any_installed(self) -> bool {
        self.claude_installed || self.codex_installed
    }

    pub const fn all_installed(self) -> bool {
        self.claude_installed && self.codex_installed
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AgentHookOperation {
    Install,
    Status,
    Uninstall,
}

pub fn agent_hook_status() -> Result<AgentHookStatus> {
    manage_current_user_agent_hooks(AgentHookOperation::Status)
}

pub fn install_agent_hooks() -> Result<AgentHookStatus> {
    manage_current_user_agent_hooks(AgentHookOperation::Install)
}

pub fn uninstall_agent_hooks() -> Result<AgentHookStatus> {
    manage_current_user_agent_hooks(AgentHookOperation::Uninstall)
}

fn manage_current_user_agent_hooks(operation: AgentHookOperation) -> Result<AgentHookStatus> {
    let home = BaseDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .context("no se pudo resolver el directorio de usuario")?;
    let selected = [AgentKind::Claude, AgentKind::Codex].into_iter().collect();
    let report = manage_agent_hooks(&home, &selected, operation, false)?;
    let report = if operation == AgentHookOperation::Status {
        report
    } else {
        manage_agent_hooks(&home, &selected, AgentHookOperation::Status, false)?
    };
    Ok(agent_hook_status_from_report(&report))
}

fn agent_hook_status_from_report(report: &Value) -> AgentHookStatus {
    let installed = |agent| {
        report["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["agent"] == agent)
            .and_then(|entry| entry["installed"].as_bool())
            .unwrap_or(false)
    };
    AgentHookStatus {
        claude_installed: installed("Claude"),
        codex_installed: installed("Codex"),
    }
}

fn run_agent_setup_cli(arguments: &[String]) -> Result<()> {
    let operation = match arguments.first().map(String::as_str) {
        Some("setup") | None => AgentHookOperation::Install,
        Some("status") => AgentHookOperation::Status,
        Some("uninstall") => AgentHookOperation::Uninstall,
        _ => bail!("uso: agent [setup|status|uninstall] [claude|codex|all] [--dry-run]"),
    };
    let dry_run = arguments.iter().any(|argument| argument == "--dry-run");
    if dry_run && operation != AgentHookOperation::Install {
        bail!("--dry-run solo se puede usar con agent setup");
    }
    let selected: HashSet<_> = arguments
        .iter()
        .filter_map(|argument| AgentKind::parse(argument))
        .filter(|kind| matches!(kind, AgentKind::Claude | AgentKind::Codex))
        .collect();
    let selected = if selected.is_empty() {
        [AgentKind::Claude, AgentKind::Codex].into_iter().collect()
    } else {
        selected
    };
    let home = BaseDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .context("no se pudo resolver el directorio de usuario")?;
    let report = manage_agent_hooks(&home, &selected, operation, dry_run)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn manage_agent_hooks(
    home: &Path,
    selected: &HashSet<AgentKind>,
    operation: AgentHookOperation,
    dry_run: bool,
) -> Result<Value> {
    let managed_directory = home.join(".vibra").join("agent-hooks");
    let mut reports = Vec::new();
    for kind in [AgentKind::Claude, AgentKind::Codex] {
        if !selected.contains(&kind) {
            continue;
        }
        let (config_path, script_name, script, entries) = match kind {
            AgentKind::Claude => (
                home.join(".claude").join("settings.json"),
                "vibra-claude.sh",
                CLAUDE_HOOK_SCRIPT,
                vec![
                    ("SessionStart", Some(""), "session-start"),
                    ("UserPromptSubmit", None, "prompt"),
                    ("Stop", None, "stop"),
                    ("PermissionRequest", None, "permission"),
                    ("SessionEnd", Some(""), "session-end"),
                    (
                        "Notification",
                        Some("idle_prompt|permission_prompt"),
                        "notification",
                    ),
                ],
            ),
            AgentKind::Codex => (
                home.join(".codex").join("hooks.json"),
                "vibra-codex.sh",
                CODEX_HOOK_SCRIPT,
                vec![
                    (
                        "SessionStart",
                        Some("startup|resume|clear|compact"),
                        "session-start",
                    ),
                    ("UserPromptSubmit", None, "prompt"),
                    ("Stop", None, "stop"),
                    ("PermissionRequest", None, "permission"),
                    ("SessionEnd", None, "session-end"),
                ],
            ),
            _ => unreachable!(),
        };
        let script_path = managed_directory.join(script_name);
        let script_command = shell_quote(&script_path.to_string_lossy());
        let commands: Vec<_> = entries
            .iter()
            .map(|(_, _, event)| format!("{script_command} {event}"))
            .collect();
        let installed = hooks_installed(&config_path, &commands)?;
        if operation == AgentHookOperation::Status {
            reports.push(serde_json::json!({
                "agent": kind.display_name(),
                "installed": installed,
                "config": config_path,
                "script": script_path,
            }));
            continue;
        }

        let changed = match operation {
            AgentHookOperation::Install => {
                let mut config = read_hook_config(&config_path)?;
                let mut config_changed = false;
                for ((slot, matcher, _), command) in entries.iter().zip(&commands) {
                    config_changed |= ensure_hook_entry(&mut config, slot, *matcher, command)?;
                }
                let script_needs_write = script_changed(&script_path, script)?;
                if !dry_run {
                    if config_changed {
                        backup_if_exists(&config_path)?;
                        write_json_atomically(&config_path, &config)?;
                    }
                    if script_needs_write {
                        write_script_atomically(&script_path, script)?;
                    }
                }
                config_changed || script_needs_write
            }
            AgentHookOperation::Uninstall => {
                let mut config = read_hook_config(&config_path)?;
                let config_changed = remove_hook_entries(&mut config, &script_path)?;
                if !dry_run && config_changed {
                    backup_if_exists(&config_path)?;
                    write_json_atomically(&config_path, &config)?;
                }
                let script_removed = if !dry_run && script_path.exists() {
                    fs::remove_file(&script_path)?;
                    true
                } else {
                    script_path.exists()
                };
                config_changed || script_removed
            }
            AgentHookOperation::Status => false,
        };
        reports.push(serde_json::json!({
            "agent": kind.display_name(),
            "operation": match operation {
                AgentHookOperation::Install => if dry_run { "dry-run" } else { "setup" },
                AgentHookOperation::Uninstall => "uninstall",
                AgentHookOperation::Status => "status",
            },
            "changed": changed,
            "config": config_path,
            "script": script_path,
        }));
    }
    Ok(serde_json::json!({ "agents": reports }))
}

fn read_hook_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("no se pudo leer {}", path.display()))?;
    let value: Value = serde_json::from_str(&content)
        .with_context(|| format!("{} no contiene JSON válido", path.display()))?;
    value
        .is_object()
        .then_some(value)
        .ok_or_else(|| anyhow!("{} debe contener un objeto JSON", path.display()))
}

fn ensure_hook_entry(
    config: &mut Value,
    slot: &str,
    matcher: Option<&str>,
    command: &str,
) -> Result<bool> {
    let root = config
        .as_object_mut()
        .context("configuración JSON inválida")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("hooks debe contener un objeto")?;
    let groups = hooks
        .entry(slot)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .with_context(|| format!("hooks.{slot} debe contener una lista"))?;
    if groups.iter().any(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|handlers| {
                handlers
                    .iter()
                    .any(|handler| handler.get("command").and_then(Value::as_str) == Some(command))
            })
    }) {
        return Ok(false);
    }
    let handler = serde_json::json!({
        "type": "command",
        "command": command,
        "timeout": 1,
        "async": true,
    });
    let mut group = serde_json::json!({ "hooks": [handler] });
    if let Some(matcher) = matcher {
        group["matcher"] = Value::String(matcher.to_owned());
    }
    groups.push(group);
    Ok(true)
}

fn remove_hook_entries(config: &mut Value, script_path: &Path) -> Result<bool> {
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let raw_script_path = script_path.to_string_lossy();
    let quoted_script_path = shell_quote(&raw_script_path);
    let mut changed = false;
    hooks.retain(|_, groups| {
        let Some(groups) = groups.as_array_mut() else {
            return true;
        };
        groups.retain_mut(|group| {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = handlers.len();
            handlers.retain(|handler| {
                !handler
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| {
                        managed_hook_command_matches(
                            command,
                            raw_script_path.as_ref(),
                            &quoted_script_path,
                        )
                    })
            });
            changed |= handlers.len() != before;
            !handlers.is_empty()
        });
        !groups.is_empty()
    });
    Ok(changed)
}

fn managed_hook_command_matches(command: &str, raw_path: &str, quoted_path: &str) -> bool {
    [raw_path, quoted_path].iter().any(|prefix| {
        command
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.chars().next().is_some_and(char::is_whitespace))
    })
}

fn hooks_installed(path: &Path, commands: &[String]) -> Result<bool> {
    let config = read_hook_config(path)?;
    Ok(commands.iter().all(|command| {
        config
            .get("hooks")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|hooks| hooks.values())
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(|group| group.get("hooks").and_then(Value::as_array))
            .flatten()
            .any(|handler| handler.get("command").and_then(Value::as_str) == Some(command))
    }))
}

fn script_changed(path: &Path, script: &str) -> Result<bool> {
    Ok(!path.exists() || fs::read_to_string(path).ok().as_deref() != Some(script))
}

fn backup_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::copy(
            path,
            PathBuf::from(format!("{}.vibra-backup", path.display())),
        )?;
    }
    Ok(())
}

fn write_json_atomically(path: &Path, value: &Value) -> Result<()> {
    write_text_atomically(
        path,
        &format!("{}\n", serde_json::to_string_pretty(value)?),
        0o600,
    )
}

fn write_script_atomically(path: &Path, script: &str) -> Result<()> {
    write_text_atomically(path, script, 0o700)
}

fn write_text_atomically(path: &Path, text: &str, mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .context("ruta de configuración sin directorio padre")?;
    let parent_exists = parent.exists();
    fs::create_dir_all(parent)?;
    if !parent_exists {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("ruta de configuración sin nombre de archivo")?;
    let temporary = parent.join(format!(".{name}.vibra-tmp-{}", std::process::id()));
    fs::write(&temporary, text)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn parse_direction(value: Option<&str>) -> Result<AutomationDirection> {
    match value {
        Some("left") => Ok(AutomationDirection::Left),
        Some("right") => Ok(AutomationDirection::Right),
        Some("up") => Ok(AutomationDirection::Up),
        Some("down") => Ok(AutomationDirection::Down),
        _ => bail!("dirección esperada: left, right, up o down"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHome(PathBuf);

    impl TestHome {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("vibra automation test-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn cli_parser_covers_pane_and_agent_commands() {
        assert!(matches!(
            parse_cli_command("+pane", &["split".into(), "right".into()]).unwrap(),
            AutomationCommand::Split {
                direction: AutomationDirection::Right,
                no_focus: false,
                cwd: None,
            }
        ));
        assert!(matches!(
            parse_cli_command(
                "+pane",
                &["split".into(), "right".into(), "--no-focus".into()]
            )
            .unwrap(),
            AutomationCommand::Split {
                direction: AutomationDirection::Right,
                no_focus: true,
                cwd: None,
            }
        ));
        assert!(matches!(
            parse_cli_command("+pane", &["tab".into(), "--no-focus".into()]).unwrap(),
            AutomationCommand::CreateTab {
                no_focus: true,
                cwd: None
            }
        ));
        assert!(matches!(
            parse_cli_command(
                "+pane",
                &[
                    "run".into(),
                    "--pane".into(),
                    Uuid::nil().to_string(),
                    "codex".into()
                ]
            )
            .unwrap(),
            AutomationCommand::Send {
                newline: true,
                target_pane: Some(_),
                ..
            }
        ));
        assert!(matches!(
            parse_cli_command("+agent", &["working".into()]).unwrap(),
            AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Working
            }
        ));
        assert!(matches!(
            parse_cli_command(
                "+agent",
                &[
                    "presence".into(),
                    "codex".into(),
                    "waiting".into(),
                    "permission".into(),
                    "--session".into(),
                    "session-1".into(),
                ],
            )
            .unwrap(),
            AutomationCommand::SetAgentPresence {
                kind: AgentKind::Codex,
                state: AgentRuntimeState::Waiting,
                attention: Some(AgentAttention::Permission),
                session_id: Some(session_id),
            } if session_id == "session-1"
        ));
        assert!(matches!(
            parse_cli_command("+agent", &["open".into(), "codex".into()]).unwrap(),
            AutomationCommand::AgentOpen {
                kind: AgentKind::Codex,
                placement: AgentPlacement::Split,
                direction: AutomationDirection::Right,
                no_focus: true,
                wait: true,
                ..
            }
        ));
        assert!(matches!(
            parse_cli_command(
                "+agent",
                &[
                    "open".into(),
                    "claude".into(),
                    "--tab".into(),
                    "--name".into(),
                    "reviewer".into(),
                    "--".into(),
                    "--resume".into(),
                ]
            )
            .unwrap(),
            AutomationCommand::AgentOpen {
                kind: AgentKind::Claude,
                placement: AgentPlacement::Tab,
                name: Some(_),
                args,
                ..
            } if args == ["--resume"]
        ));
        assert!(matches!(
            parse_cli_command(
                "+agent",
                &[
                    "start".into(),
                    "--kind".into(),
                    "codex".into(),
                    "--no-wait".into()
                ]
            )
            .unwrap(),
            AutomationCommand::AgentStart {
                kind: AgentKind::Codex,
                wait: false,
                ..
            }
        ));
        assert!(matches!(
            parse_cli_command("+agent", &["kinds".into()]).unwrap(),
            AutomationCommand::AgentKinds
        ));
        assert!(matches!(
            parse_cli_command(
                "+agent",
                &[
                    "prompt".into(),
                    "reviewer".into(),
                    "Review".into(),
                    "the".into(),
                    "diff".into(),
                    "--until".into(),
                    "idle".into(),
                ]
            )
            .unwrap(),
            AutomationCommand::AgentPrompt {
                target: Some(ref t),
                text,
                wait: true,
                until,
                ..
            } if t == "reviewer" && text == "Review the diff" && until == [AgentRuntimeState::Idle]
        ));
        assert!(matches!(
            parse_cli_command(
                "+agent",
                &["wait".into(), "reviewer".into(), "--until".into(), "waiting".into()]
            )
            .unwrap(),
            AutomationCommand::AgentWait {
                target: Some(ref t),
                until,
                ..
            } if t == "reviewer" && until == [AgentRuntimeState::Waiting]
        ));
        assert!(matches!(
            parse_cli_command(
                "+agent",
                &["read".into(), "--name".into(), "reviewer".into(), "--lines".into(), "40".into()]
            )
            .unwrap(),
            AutomationCommand::AgentRead {
                target: Some(ref t),
                lines: 40,
            } if t == "reviewer"
        ));
        assert!(matches!(
            parse_cli_command("+agent", &["list".into()]).unwrap(),
            AutomationCommand::AgentList
        ));
        assert!(parse_cli_command("+pane", &["split".into(), "diagonal".into()]).is_err());
        assert!(parse_cli_command("+agent", &["open".into(), "nope".into()]).is_err());
        assert!(
            parse_cli_command(
                "+agent",
                &[
                    "open".into(),
                    "codex".into(),
                    "--name".into(),
                    "BadName".into()
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn agent_setup_merges_hooks_and_uninstall_leaves_user_hooks() {
        let home = TestHome::new();
        let claude_settings = home.0.join(".claude/settings.json");
        let codex_hooks = home.0.join(".codex/hooks.json");
        let existing =
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo keep"}]}]}}"#;
        write_text_atomically(&claude_settings, existing, 0o600).unwrap();
        write_text_atomically(&codex_hooks, existing, 0o600).unwrap();
        let agents = [AgentKind::Claude, AgentKind::Codex].into_iter().collect();

        let report =
            manage_agent_hooks(&home.0, &agents, AgentHookOperation::Install, false).unwrap();
        assert!(
            report["agents"]
                .as_array()
                .unwrap()
                .iter()
                .all(|agent| agent["changed"] == true)
        );
        assert!(home.0.join(".vibra/agent-hooks/vibra-claude.sh").exists());
        assert!(home.0.join(".vibra/agent-hooks/vibra-codex.sh").exists());
        assert!(!home.0.join(".codex/config.toml").exists());

        for path in [&claude_settings, &codex_hooks] {
            let config: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            let handlers: Vec<_> = config["hooks"]
                .as_object()
                .unwrap()
                .values()
                .flat_map(|groups| groups.as_array().unwrap())
                .flat_map(|group| group["hooks"].as_array().unwrap())
                .collect();
            assert!(
                handlers
                    .iter()
                    .any(|handler| handler["command"] == "echo keep")
            );
            assert!(handlers.iter().any(|handler| {
                handler["command"]
                    .as_str()
                    .is_some_and(|command| command.contains(".vibra/agent-hooks"))
                    && handler["timeout"] == 1
                    && handler["async"] == true
            }));
        }

        let repeat =
            manage_agent_hooks(&home.0, &agents, AgentHookOperation::Install, false).unwrap();
        assert!(
            repeat["agents"]
                .as_array()
                .unwrap()
                .iter()
                .all(|agent| agent["changed"] == false)
        );

        manage_agent_hooks(&home.0, &agents, AgentHookOperation::Uninstall, false).unwrap();
        assert!(!home.0.join(".vibra/agent-hooks/vibra-claude.sh").exists());
        assert!(!home.0.join(".vibra/agent-hooks/vibra-codex.sh").exists());
        for path in [&claude_settings, &codex_hooks] {
            let config: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(
                config["hooks"]["Stop"][0]["hooks"][0]["command"],
                "echo keep"
            );
        }
    }

    #[test]
    fn agent_hook_status_reads_each_agent_from_a_report() {
        let status = agent_hook_status_from_report(&serde_json::json!({
            "agents": [
                { "agent": "Claude", "installed": true },
                { "agent": "Codex", "installed": false },
            ]
        }));

        assert!(status.claude_installed);
        assert!(!status.codex_installed);
        assert!(status.any_installed());
        assert!(!status.all_installed());
    }

    #[test]
    fn agent_launch_command_quotes_args() {
        assert_eq!(
            agent_launch_command(AgentKind::Codex, &["-m".into(), "o3".into()]),
            "codex -m o3"
        );
        assert_eq!(
            agent_launch_command(AgentKind::Claude, &["hello world".into()]),
            "claude 'hello world'"
        );
        assert_eq!(AgentKind::Cursor.executable(), "cursor-agent");
    }

    #[test]
    fn hook_events_normalize_to_presence_commands() {
        let session = serde_json::json!({ "session_id": "session-1" });
        assert!(matches!(
            agent_hook_command(AgentKind::Codex, "prompt", &session).unwrap(),
            AutomationCommand::SetAgentPresence {
                kind: AgentKind::Codex,
                state: AgentRuntimeState::Working,
                attention: None,
                session_id: Some(session_id),
            } if session_id == "session-1"
        ));
        assert!(matches!(
            agent_hook_command(
                AgentKind::Claude,
                "notification",
                &serde_json::json!({ "notification_type": "permission_prompt" }),
            )
            .unwrap(),
            AutomationCommand::SetAgentPresence {
                kind: AgentKind::Claude,
                state: AgentRuntimeState::Waiting,
                attention: Some(AgentAttention::Permission),
                ..
            }
        ));
        assert!(matches!(
            agent_hook_command(AgentKind::Codex, "session-end", &session).unwrap(),
            AutomationCommand::ClearAgentPresence {
                session_id: Some(session_id),
            } if session_id == "session-1"
        ));
    }

    #[test]
    fn unix_socket_round_trips_a_capability_request() {
        let server = AutomationServer::start().unwrap();
        let path = server.path().to_path_buf();
        let pane_id = Uuid::new_v4();
        let token = Uuid::new_v4();
        let client = thread::spawn(move || {
            let mut stream = UnixStream::connect(path).unwrap();
            serde_json::to_writer(
                &mut stream,
                &AutomationEnvelope {
                    pane_id,
                    token,
                    command: AutomationCommand::List,
                },
            )
            .unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            serde_json::from_reader::<_, AutomationResponse>(stream).unwrap()
        });

        let incoming = server.receiver().recv_blocking().unwrap();
        assert_eq!(incoming.envelope.pane_id, pane_id);
        assert_eq!(incoming.envelope.token, token);
        incoming
            .response
            .send(AutomationResponse::success(
                serde_json::json!({ "count": 1 }),
            ))
            .unwrap();

        let response = client.join().unwrap();
        assert!(response.ok);
        assert_eq!(response.data.unwrap()["count"], 1);
    }

    #[test]
    fn automation_server_accepts_a_second_request_while_the_first_is_pending() {
        let server = AutomationServer::start().unwrap();
        let path = server.path().to_path_buf();
        let start_client = |path: PathBuf| {
            thread::spawn(move || {
                let mut stream = UnixStream::connect(path).unwrap();
                serde_json::to_writer(
                    &mut stream,
                    &AutomationEnvelope {
                        pane_id: Uuid::new_v4(),
                        token: Uuid::new_v4(),
                        command: AutomationCommand::List,
                    },
                )
                .unwrap();
                stream.shutdown(std::net::Shutdown::Write).unwrap();
                serde_json::from_reader::<_, AutomationResponse>(stream).unwrap()
            })
        };

        let first_client = start_client(path.clone());
        let first = server.receiver().recv_blocking().unwrap();
        let second_client = start_client(path);
        let receiver = server.receiver();
        let (incoming_tx, incoming_rx) = mpsc::channel();
        let relay = thread::spawn(move || incoming_tx.send(receiver.recv_blocking()).unwrap());

        let (second, concurrent) = match incoming_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(second) => (second.unwrap(), true),
            Err(_) => {
                first
                    .response
                    .send(AutomationResponse::success(Value::Null))
                    .unwrap();
                (
                    incoming_rx
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap()
                        .unwrap(),
                    false,
                )
            }
        };
        second
            .response
            .send(AutomationResponse::success(Value::Null))
            .unwrap();
        if concurrent {
            first
                .response
                .send(AutomationResponse::success(Value::Null))
                .unwrap();
        }

        relay.join().unwrap();
        assert!(first_client.join().unwrap().ok);
        assert!(second_client.join().unwrap().ok);
        assert!(concurrent, "the first response blocked the second request");
    }
}
