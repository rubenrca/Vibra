//! Process helpers shared by the agent process and client terminals.

use std::process::ExitStatus;
use std::sync::{Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::rpc::lock;

#[derive(Default)]
enum ExitState {
    #[default]
    Running,
    /// `None` when the process was reaped but its status could not be read.
    Done(Option<ExitStatus>),
}

/// Written by the thread that reaps a child; read by whoever needs its status.
#[derive(Default)]
pub(super) struct ExitSlot {
    state: Mutex<ExitState>,
    changed: Condvar,
}

impl ExitSlot {
    pub(super) fn finish(&self, status: Option<ExitStatus>) {
        *lock(&self.state) = ExitState::Done(status);
        self.changed.notify_all();
    }

    /// `None` while running. `Some(None)` means exited with an unknown status.
    pub(super) fn status(&self) -> Option<Option<ExitStatus>> {
        match *lock(&self.state) {
            ExitState::Running => None,
            ExitState::Done(status) => Some(status),
        }
    }

    /// Waits up to `timeout`. Same return shape as [`ExitSlot::status`].
    pub(super) fn wait_timeout(&self, timeout: Duration) -> Option<Option<ExitStatus>> {
        self.wait_until(Some(Instant::now() + timeout))
    }

    /// Blocks until the process exits. Only call this off the reader thread.
    pub(super) fn wait(&self) -> Option<ExitStatus> {
        self.wait_until(None).flatten()
    }

    fn wait_until(&self, deadline: Option<Instant>) -> Option<Option<ExitStatus>> {
        let mut state = lock(&self.state);
        loop {
            if let ExitState::Done(status) = *state {
                return Some(status);
            }
            state = match deadline {
                None => self
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return None;
                    }
                    self.changed
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                }
            };
        }
    }
}

/// Sends `signal` to every process in the group led by `pid`. Failures are
/// ignored because the group may already be gone.
pub(super) fn signal_group(pid: i32, signal: i32) {
    if pid > 0 {
        // SAFETY: kill(2) has no memory-safety preconditions; a negative pid
        // addresses a process group.
        unsafe {
            libc::kill(-pid, signal);
        }
    }
}

/// Joins `thread` if it finishes before `deadline`, otherwise detaches it. A
/// stray grandchild can keep a pipe open, so teardown must never wait forever.
pub(super) fn join_before(thread: JoinHandle<()>, deadline: Instant) {
    while !thread.is_finished() {
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let _ = thread.join();
}

pub(super) fn signal_name(signal: i32) -> String {
    let name = match signal {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGABRT => "SIGABRT",
        libc::SIGKILL => "SIGKILL",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGALRM => "SIGALRM",
        libc::SIGTERM => "SIGTERM",
        other => return format!("signal {other}"),
    };
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_timeout_returns_none_while_running() {
        let slot = ExitSlot::default();
        assert_eq!(slot.status(), None);
        assert_eq!(slot.wait_timeout(Duration::from_millis(1)), None);
    }

    #[test]
    fn signal_names_fall_back_to_numbers() {
        assert_eq!(signal_name(libc::SIGKILL), "SIGKILL");
        assert_eq!(signal_name(99), "signal 99");
    }
}
