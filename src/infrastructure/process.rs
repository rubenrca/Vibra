//! Bounded capture for short-lived commands, with concurrent pipe draining and
//! cleanup of descendants that inherit a pipe from a login shell.

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
    capture_command(command, input, limits, false)
}

/// Read-only queries can stop at their stdout budget. Keep the sentinel byte
/// so callers distinguish a truncated diff from a complete response.
pub(super) fn command_output_until_limit(
    command: &mut Command,
    limits: CommandLimits,
) -> Result<Output> {
    capture_command(command, None, limits, true)
}

fn capture_command(
    command: &mut Command,
    input: Option<Vec<u8>>,
    limits: CommandLimits,
    stop_at_stdout_limit: bool,
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
    make_nonblocking(&stdout)?;
    if let Some(stderr) = &process.child.stderr {
        make_nonblocking(stderr)?;
    }
    if let Some(stdin) = &process.child.stdin {
        make_nonblocking(stdin)?;
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let reader_cancelled = cancelled.clone();
    let reader = thread::spawn(move || {
        capture_pipe(
            stdout,
            limits.stdout.saturating_add(1),
            stop_at_stdout_limit,
            &reader_cancelled,
        )
    });
    let error_cancelled = cancelled.clone();
    let error_reader = process.child.stderr.take().map(|stderr| {
        thread::spawn(move || capture_pipe(stderr, limits.stderr, false, &error_cancelled))
    });
    let writer = input.map(|bytes| {
        let stdin = process.child.stdin.take().expect("Command stdin was piped");
        let cancelled = cancelled.clone();
        thread::spawn(move || write_pipe(stdin, &bytes, &cancelled))
    });
    let started = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            process.stop();
        }
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
    cancelled.store(true, Ordering::Release);
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

fn make_nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn poll_pipe(pipe: &impl AsRawFd, events: libc::c_short) {
    let mut poll = libc::pollfd {
        fd: pipe.as_raw_fd(),
        events,
        revents: 0,
    };
    unsafe { libc::poll(&mut poll, 1, 20) };
}

fn capture_pipe(
    mut pipe: impl Read + AsRawFd,
    limit: usize,
    stop_at_limit: bool,
    cancelled: &AtomicBool,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    // Drain at most 128 KiB after cancellation: twice macOS's largest 64 KiB
    // pipe buffer. Never wait for EOF from descendants outside the group, or
    // let one keep a reader alive by continuously filling its inherited pipe.
    let mut final_bytes = 128 * 1024;
    loop {
        let cancelling = cancelled.load(Ordering::Acquire);
        let read_limit = if cancelling {
            if final_bytes == 0 {
                break;
            }
            buffer.len().min(final_bytes)
        } else {
            buffer.len()
        };
        match pipe.read(&mut buffer[..read_limit]) {
            Ok(0) => break,
            Ok(count) => {
                if cancelling {
                    final_bytes -= count;
                }
                bytes.extend_from_slice(&buffer[..count.min(limit.saturating_sub(bytes.len()))]);
                if stop_at_limit && bytes.len() == limit {
                    cancelled.store(true, Ordering::Release);
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                poll_pipe(&pipe, libc::POLLIN);
            }
            Err(error) => {
                cancelled.store(true, Ordering::Release);
                return Err(error);
            }
        }
    }
    Ok(bytes)
}

fn write_pipe(
    mut pipe: impl Write + AsRawFd,
    mut bytes: &[u8],
    cancelled: &AtomicBool,
) -> io::Result<()> {
    while !bytes.is_empty() && !cancelled.load(Ordering::Acquire) {
        match pipe.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                poll_pipe(&pipe, libc::POLLOUT);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
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
    fn immediate_exit_preserves_complete_output_below_both_pipe_budgets() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "head -c 262144 /dev/zero; head -c 262144 /dev/zero >&2",
        ]);
        let output = command_output(
            &mut command,
            None,
            CommandLimits {
                stderr: 256 * 1024,
                ..limits()
            },
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, vec![0; 256 * 1024]);
        assert_eq!(output.stderr, vec![0; 256 * 1024]);
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
    fn stdout_budget_stops_an_unbounded_query_and_keeps_its_sentinel() {
        let mut command = Command::new("/bin/cat");
        command.arg("/dev/zero");
        let started = Instant::now();
        let output = command_output_until_limit(
            &mut command,
            CommandLimits {
                stdout: 1024,
                ..limits()
            },
        )
        .unwrap();
        assert_eq!(output.stdout.len(), 1025);
        assert!(!output.status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn pipe_capture_drains_discarded_output_without_retaining_it_all() {
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        make_nonblocking(&reader).unwrap();
        let writer = thread::spawn(move || writer.write_all(&vec![b'x'; 128 * 1024]));
        let output = capture_pipe(reader, 1024, false, &AtomicBool::new(false)).unwrap();
        writer.join().unwrap().unwrap();
        assert_eq!(output, vec![b'x'; 1024]);
    }

    #[test]
    fn cancellation_preserves_available_output_without_waiting_for_pipe_eof() {
        let (reader, mut open_writer) = std::os::unix::net::UnixStream::pair().unwrap();
        make_nonblocking(&reader).unwrap();
        open_writer.write_all(b"finished").unwrap();
        let started = Instant::now();
        let output = capture_pipe(reader, 1024, false, &AtomicBool::new(true)).unwrap();
        assert_eq!(output, b"finished");
        assert!(started.elapsed() < Duration::from_secs(1));
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
            Some(vec![b'x'; 128 * 1024]),
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
