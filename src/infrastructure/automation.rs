use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

const MAX_AUTOMATION_REQUEST_BYTES: u64 = 1024 * 1024;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum AutomationCommand {
    List,
    Send {
        text: String,
        #[serde(default)]
        newline: bool,
    },
    Split {
        direction: AutomationDirection,
    },
    Focus {
        direction: AutomationDirection,
    },
    Close,
    Zoom,
    AgentStatus,
    SetAgentState {
        state: AgentRuntimeState,
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
        let path = directory.join(format!("{}.sock", std::process::id()));
        if path.exists() {
            fs::remove_file(&path)?;
        }
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
                    handle_connection(stream, &sender);
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
    let parsed = Read::by_ref(&mut stream)
        .take(MAX_AUTOMATION_REQUEST_BYTES)
        .read_to_string(&mut request)
        .map_err(anyhow::Error::from)
        .and_then(|_| serde_json::from_str::<AutomationEnvelope>(&request).map_err(Into::into));
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
                    .recv_timeout(Duration::from_secs(10))
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
    if !matches!(mode, "+pane" | "+agent") {
        return Ok(false);
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
    let command = parse_cli_command(mode, &arguments[1..])?;
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
    stream.read_to_end(&mut response)?;
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

fn parse_cli_command(mode: &str, arguments: &[String]) -> Result<AutomationCommand> {
    let operation = arguments.first().map(String::as_str).unwrap_or("status");
    if mode == "+agent" {
        return match operation {
            "status" => Ok(AutomationCommand::AgentStatus),
            "idle" => Ok(AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Idle,
            }),
            "working" => Ok(AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Working,
            }),
            "waiting" => Ok(AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Waiting,
            }),
            _ => bail!("uso: +agent [status|idle|working|waiting]"),
        };
    }
    match operation {
        "list" | "status" => Ok(AutomationCommand::List),
        "send" | "run" => {
            let text = arguments.get(1..).unwrap_or_default().join(" ");
            if text.is_empty() {
                bail!("uso: +pane {operation} <texto>");
            }
            Ok(AutomationCommand::Send {
                text,
                newline: operation == "run",
            })
        }
        "split" | "focus" => {
            let direction = parse_direction(arguments.get(1).map(String::as_str))?;
            if operation == "split" {
                Ok(AutomationCommand::Split { direction })
            } else {
                Ok(AutomationCommand::Focus { direction })
            }
        }
        "close" => Ok(AutomationCommand::Close),
        "zoom" => Ok(AutomationCommand::Zoom),
        _ => bail!("uso: +pane [list|send|run|split|focus|close|zoom]"),
    }
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

    #[test]
    fn cli_parser_covers_pane_and_agent_commands() {
        assert!(matches!(
            parse_cli_command("+pane", &["split".into(), "right".into()]).unwrap(),
            AutomationCommand::Split {
                direction: AutomationDirection::Right
            }
        ));
        assert!(matches!(
            parse_cli_command("+agent", &["working".into()]).unwrap(),
            AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Working
            }
        ));
        assert!(parse_cli_command("+pane", &["split".into(), "diagonal".into()]).is_err());
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
}
