use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::domain::workspace::HostedAgentKind;
use crate::infrastructure::harness::{HostedConversation, official_spawn_argv};
use crate::ports::harness::HarnessError;

pub struct HostedProcess {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
}

impl HostedProcess {
    pub fn spawn(
        agent: HostedAgentKind,
        working_directory: &Path,
        vendor_session_id: Option<&str>,
        conversation: &mut HostedConversation,
        environment: HashMap<String, String>,
    ) -> Result<(Self, async_channel::Receiver<String>), HarnessError> {
        let argv = official_spawn_argv(agent, vendor_session_id)?;
        let program = resolve_program(&argv[0]).ok_or_else(|| {
            HarnessError::Protocol(format!("{} no está instalado en PATH", argv[0]))
        })?;
        let mut command = Command::new(&program);
        command
            .args(&argv[1..])
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in environment {
            command.env(key, value);
        }
        compose_path(&mut command, &program);
        let mut child = command.spawn().map_err(|error| {
            HarnessError::Protocol(format!("no se pudo arrancar {}: {error}", argv[0]))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::Protocol("stdin del agente no disponible".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Protocol("stdout del agente no disponible".into()))?;
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for _line in reader.lines() {}
            });
        }
        let (tx, rx) = async_channel::unbounded();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if tx.send_blocking(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let stdin = Arc::new(Mutex::new(stdin));
        let process = Self { child, stdin };
        let handshake = conversation.encode_handshake()?;
        if !handshake.is_empty() {
            process.write_all(&handshake)?;
        }
        Ok((process, rx))
    }

    pub fn write_all(&self, bytes: &[u8]) -> Result<(), HarnessError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| HarnessError::Protocol("stdin bloqueado".into()))?;
        stdin
            .write_all(bytes)
            .and_then(|_| stdin.flush())
            .map_err(|error| HarnessError::Protocol(format!("no se pudo escribir al agente: {error}")))
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for HostedProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

fn resolve_program(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return path.exists().then_some(path);
    }
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".claude").join("local"));
        dirs.push(home.join(".codex").join("bin"));
        dirs.push(home.join(".grok").join("bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.into_iter().map(|dir| dir.join(name)).find(|path| path.is_file())
}

fn compose_path(command: &mut Command, program: &Path) {
    let mut paths = Vec::new();
    if let Some(dir) = program.parent() {
        paths.push(dir.to_path_buf());
    }
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        command.env("PATH", joined);
    }
}
