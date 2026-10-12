//! Client terminals behind `terminal/*`. Each terminal is its own process group
//! whose merged stdout and stderr feed two places: live chunks to the chat view,
//! and a bounded buffer the agent reads with `terminal/output`.

use std::collections::HashMap;
use std::io::{self, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::domain::agent_chat::ChatEvent;

use super::Emitter;
use super::child::{ExitSlot, join_before, signal_group, signal_name};
use super::rpc::{INTERNAL_ERROR, INVALID_PARAMS, RpcError, lock};

/// Output kept per terminal when the agent does not pass `outputByteLimit`.
const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
/// How long the waiter lets the output pump drain after the process exits.
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct Terminals {
    default_cwd: PathBuf,
    emitter: Arc<Emitter>,
    next_id: AtomicU64,
    terminals: Mutex<HashMap<String, Arc<Terminal>>>,
}

pub(super) struct Terminal {
    id: String,
    pid: i32,
    output: Mutex<Output>,
    exit: ExitSlot,
}

struct Output {
    text: String,
    truncated: bool,
    limit: usize,
}

impl Terminals {
    pub(super) fn new(default_cwd: PathBuf, emitter: Arc<Emitter>) -> Self {
        Self {
            default_cwd,
            emitter,
            next_id: AtomicU64::new(0),
            terminals: Mutex::new(HashMap::new()),
        }
    }

    /// `terminal/create`: starts the command and returns its terminal id.
    pub(super) fn create(&self, params: &Value) -> Result<String, RpcError> {
        let command = str_param(params, "command")
            .filter(|command| !command.is_empty())
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "terminal/create needs a command."))?;
        let args: Vec<String> = params
            .get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let env: Vec<(String, String)> = params
            .get("env")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        Some((
                            str_param(entry, "name")?.to_owned(),
                            str_param(entry, "value")?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let cwd = match str_param(params, "cwd") {
            Some(cwd) => self.default_cwd.join(cwd),
            None => self.default_cwd.clone(),
        };
        let limit = params
            .get("outputByteLimit")
            .and_then(Value::as_u64)
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(DEFAULT_OUTPUT_LIMIT);

        let (reader, child) = spawn_command(command, &args, &env, &cwd).map_err(|error| {
            RpcError::new(INTERNAL_ERROR, format!("Could not run {command}: {error}"))
        })?;
        let id = format!("term-{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let terminal = Arc::new(Terminal {
            id: id.clone(),
            pid: i32::try_from(child.id()).unwrap_or(0),
            output: Mutex::new(Output {
                text: String::new(),
                truncated: false,
                limit,
            }),
            exit: ExitSlot::default(),
        });
        lock(&self.terminals).insert(id.clone(), terminal.clone());

        let pump = {
            let terminal = terminal.clone();
            let emitter = self.emitter.clone();
            thread::spawn(move || pump_output(reader, &terminal, &emitter))
        };
        let emitter = self.emitter.clone();
        thread::spawn(move || wait_for_child(child, pump, &terminal, &emitter));
        Ok(id)
    }

    /// `terminal/output`: the retained output tail and, once it ended, the exit status.
    pub(super) fn output(&self, params: &Value) -> Result<Value, RpcError> {
        Ok(self.lookup(params)?.snapshot())
    }

    /// `terminal/kill`: kills the command and keeps its output.
    pub(super) fn kill(&self, params: &Value) -> Result<(), RpcError> {
        self.lookup(params)?.kill();
        Ok(())
    }

    /// `terminal/release`: kills the command if needed and forgets the terminal.
    pub(super) fn release(&self, params: &Value) {
        let removed =
            str_param(params, "terminalId").and_then(|id| lock(&self.terminals).remove(id));
        if let Some(terminal) = removed {
            terminal.kill();
        }
    }

    pub(super) fn lookup(&self, params: &Value) -> Result<Arc<Terminal>, RpcError> {
        let id = str_param(params, "terminalId")
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "The request needs a terminalId."))?;
        lock(&self.terminals)
            .get(id)
            .cloned()
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("Unknown terminal: {id}")))
    }

    /// Ends every terminal's process group. Used when the connection closes.
    pub(super) fn kill_all(&self) {
        for terminal in lock(&self.terminals).values() {
            terminal.kill();
        }
    }
}

impl Terminal {
    /// Blocks until the command exits. Call it off the reader thread.
    pub(super) fn wait_for_exit(&self) -> Value {
        exit_status_json(self.exit.wait())
    }

    fn kill(&self) {
        signal_group(self.pid, libc::SIGKILL);
    }

    fn snapshot(&self) -> Value {
        let output = lock(&self.output);
        let mut value = json!({"output": output.text, "truncated": output.truncated});
        if let Some(status) = self.exit.status() {
            value["exitStatus"] = exit_status_json(status);
        }
        value
    }

    /// Appends output and keeps only its last `limit` bytes, cut on a char boundary.
    fn append(&self, text: &str) {
        let mut output = lock(&self.output);
        output.text.push_str(text);
        if output.text.len() > output.limit {
            let mut cut = output.text.len() - output.limit;
            while !output.text.is_char_boundary(cut) {
                cut += 1;
            }
            output.text.drain(..cut);
            output.truncated = true;
        }
    }
}

fn str_param<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

/// Both streams go into one pipe, so the chat sees output in the order it was written.
fn spawn_command(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
) -> io::Result<(io::PipeReader, Child)> {
    let (reader, writer) = io::pipe()?;
    let stderr = writer.try_clone()?;
    // The Command holds the write ends until it is dropped. Keep it in this
    // function so the reader sees EOF once the process exits.
    let child = Command::new(command)
        .args(args)
        .envs(env.iter().map(|(name, value)| (name, value)))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(stderr)
        .process_group(0)
        .spawn()?;
    Ok((reader, child))
}

fn pump_output(mut reader: io::PipeReader, terminal: &Terminal, emitter: &Emitter) {
    let mut pending = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        pending.extend_from_slice(&buffer[..read]);
        publish(terminal, emitter, take_utf8(&mut pending));
    }
    if !pending.is_empty() {
        publish(
            terminal,
            emitter,
            String::from_utf8_lossy(&pending).into_owned(),
        );
    }
}

fn publish(terminal: &Terminal, emitter: &Emitter, text: String) {
    if text.is_empty() {
        return;
    }
    terminal.append(&text);
    emitter.emit(ChatEvent::TerminalOutput {
        terminal_id: terminal.id.clone(),
        output: text,
        exit_status: None,
    });
}

/// Reaps the command, lets its output drain, then reports the exit.
fn wait_for_child(mut child: Child, pump: JoinHandle<()>, terminal: &Terminal, emitter: &Emitter) {
    let status = child.wait().ok();
    join_before(pump, Instant::now() + OUTPUT_DRAIN_TIMEOUT);
    terminal.exit.finish(status);
    emitter.emit(ChatEvent::TerminalOutput {
        terminal_id: terminal.id.clone(),
        output: String::new(),
        exit_status: Some(event_exit_code(status)),
    });
}

/// Shell convention for signal deaths (128 + signal), so the chat always gets a number.
fn event_exit_code(status: Option<ExitStatus>) -> i32 {
    status
        .and_then(|status| {
            status
                .code()
                .or_else(|| status.signal().map(|signal| 128 + signal))
        })
        .unwrap_or(-1)
}

fn exit_status_json(status: Option<ExitStatus>) -> Value {
    json!({
        "exitCode": status.and_then(|status| status.code()),
        "signal": status.and_then(|status| status.signal()).map(signal_name),
    })
}

/// Decodes the longest valid UTF-8 prefix of `pending` and leaves an incomplete
/// trailing sequence in place for the next read. Invalid bytes become U+FFFD.
fn take_utf8(pending: &mut Vec<u8>) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(valid) => {
                text.push_str(valid);
                pending.clear();
                return text;
            }
            Err(error) => {
                let valid_len = error.valid_up_to();
                text.push_str(std::str::from_utf8(&pending[..valid_len]).unwrap_or_default());
                match error.error_len() {
                    Some(invalid_len) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        pending.drain(..valid_len + invalid_len);
                    }
                    None => {
                        pending.drain(..valid_len);
                        return text;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal_with_limit(limit: usize) -> Terminal {
        Terminal {
            id: "term-test".to_owned(),
            pid: 0,
            output: Mutex::new(Output {
                text: String::new(),
                truncated: false,
                limit,
            }),
            exit: ExitSlot::default(),
        }
    }

    #[test]
    fn utf8_split_across_reads_is_kept_for_the_next_read() {
        let bytes = "é!".as_bytes();
        let mut pending = bytes[..1].to_vec();
        assert_eq!(take_utf8(&mut pending), "");
        pending.extend_from_slice(&bytes[1..]);
        assert_eq!(take_utf8(&mut pending), "é!");
        assert!(pending.is_empty());
    }

    #[test]
    fn invalid_utf8_becomes_replacement_characters() {
        let mut pending = vec![b'a', 0xff, b'b'];
        assert_eq!(take_utf8(&mut pending), "a\u{fffd}b");
        assert!(pending.is_empty());
    }

    #[test]
    fn output_keeps_the_tail_on_a_char_boundary() {
        // Keeping the last 3 of "a€b" would start inside "€" (bytes 1..4), so the
        // cut moves forward to the next boundary and only "b" survives.
        let terminal = terminal_with_limit(3);
        terminal.append("a€b");
        let output = lock(&terminal.output);
        assert!(output.truncated);
        assert_eq!(output.text, "b");
    }

    #[test]
    fn output_is_not_marked_truncated_under_the_limit() {
        let terminal = terminal_with_limit(100);
        terminal.append("hello");
        terminal.append(" world");
        let snapshot = terminal.snapshot();
        assert_eq!(snapshot["output"], "hello world");
        assert_eq!(snapshot["truncated"], false);
        assert!(snapshot.get("exitStatus").is_none());
    }

    #[test]
    fn exit_status_json_reports_code_or_signal() {
        assert_eq!(
            exit_status_json(None),
            json!({"exitCode": null, "signal": null})
        );
    }
}
