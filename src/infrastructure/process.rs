//! Bounded capture for short-lived commands, with concurrent pipe draining and
//! cleanup of descendants that inherit a pipe from a login shell.

use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

pub(super) struct CommandLimits {
    pub timeout: Duration,
    /// Retain one extra byte so the caller can detect oversized output.
    pub stdout: usize,
    /// Diagnostics are truncated while the rest of stderr is drained.
    pub stderr: usize,
}

pub(super) fn command_output(
    command: &mut Command,
    input: Option<Vec<u8>>,
    limits: CommandLimits,
) -> Result<Output> {
    let child = command
        .process_group(0)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(if limits.stderr > 0 {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .spawn()
        .context("Could not start the command.")?;
    let mut process = ProcessGroup {
        child,
        stopped: false,
    };
    let stdout = process
        .child
        .stdout
        .take()
        .context("Command stdout unavailable.")?;
    let reader = thread::spawn(move || drain_capped(stdout, limits.stdout.saturating_add(1)));
    let error_reader = process
        .child
        .stderr
        .take()
        .map(|stderr| thread::spawn(move || drain_capped(stderr, limits.stderr)));
    let writer = input.map(|bytes| {
        let mut stdin = process.child.stdin.take().expect("Command stdin was piped");
        thread::spawn(move || stdin.write_all(&bytes))
    });
    let started = Instant::now();
    let status = loop {
        match process.child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < limits.timeout => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                break Err(anyhow::anyhow!(
                    "The command timed out after {} seconds.",
                    limits.timeout.as_secs()
                ));
            }
            Err(error) => break Err(error).context("Could not wait for the command."),
        }
    };
    // The main process may have exited while a descendant still owns a pipe.
    // Close that private group before joining readers, including on errors.
    process.stop();
    let written = writer.map(|writer| writer.join());
    let stdout = reader.join();
    let stderr = error_reader.map(|reader| reader.join());
    let status = status?;
    if let Some(written) = written {
        let result = written.map_err(|_| anyhow::anyhow!("Command stdin writer failed."))?;
        if let Err(error) = result
            && error.kind() != io::ErrorKind::BrokenPipe
        {
            return Err(error).context("Could not write the command input.");
        }
    }
    Ok(Output {
        status,
        stdout: stdout.map_err(|_| anyhow::anyhow!("Command stdout reader failed."))??,
        stderr: match stderr {
            Some(result) => {
                result.map_err(|_| anyhow::anyhow!("Command stderr reader failed."))??
            }
            None => Vec::new(),
        },
    })
}

pub(super) fn drain_capped(mut reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.by_ref().take(limit as u64).read_to_end(&mut bytes)?;
    io::copy(&mut reader, &mut io::sink())?;
    Ok(bytes)
}

struct ProcessGroup {
    child: Child,
    stopped: bool,
}

impl ProcessGroup {
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
        self.stopped = true;
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> CommandLimits {
        CommandLimits {
            timeout: Duration::from_secs(2),
            stdout: 256 * 1024,
            stderr: 1024,
        }
    }

    #[test]
    fn drains_input_and_both_output_pipes_without_deadlocking() {
        let input = vec![b'x'; 128 * 1024];
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "cat; head -c 131072 /dev/zero >&2"]);
        let output = command_output(&mut command, Some(input.clone()), limits()).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, input);
        assert_eq!(output.stderr.len(), 1024);
    }

    #[test]
    fn oversized_stdout_is_bounded_and_detectable_without_breaking_the_command() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "head -c 131072 /dev/zero"]);
        let output = command_output(
            &mut command,
            None,
            CommandLimits {
                stdout: 1024,
                ..limits()
            },
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 1025);
    }

    #[test]
    fn exited_shell_does_not_leave_a_descendant_holding_output_open() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & printf done"]);
        let started = Instant::now();
        let output = command_output(&mut command, None, limits()).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"done");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn timeout_stops_descendants_and_joins_all_pipe_workers() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & wait"]);
        let started = Instant::now();
        let error = command_output(
            &mut command,
            None,
            CommandLimits {
                timeout: Duration::from_millis(100),
                ..limits()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn captures_failure_diagnostics_and_handles_an_early_closed_stdin() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf failed >&2; exit 7"]);
        let output = command_output(&mut command, Some(vec![b'x'; 128 * 1024]), limits()).unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stderr, b"failed");
    }
}
