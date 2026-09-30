//! libghostty-vt backend. The terminal state is protected by one
//! mutex; callbacks only enqueue data and never reenter it. The PTY worker owns
//! the child and reaps it, independently of the lifetime of the UI handle.
use super::terminal_support::{
    COLOR_SUPPRESSING_ENV, process_invoked_name, process_working_directory,
    terminal_child_environment,
};
use crate::ports::terminal::*;
use crate::ports::terminal_keyboard::TerminalKeyInput;
use anyhow::{Context, Result, bail};
use async_channel::{Receiver, Sender};
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const MAX_TERMINAL_EVENTS: usize = 32;
const MAX_QUEUED_PTY_INPUTS: usize = 32;
// A 1 MiB clipboard response grows to ~1.34 MiB after OSC 52 base64.
const MAX_PTY_INPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CLIPBOARD_STORE_BYTES: usize = 1024 * 1024;
const MAX_PENDING_PTY_WRITE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TERMINAL_TITLE_BYTES: usize = 4096;

mod engine;
use engine::*;

#[derive(Default)]
pub struct GhosttyTerminalPort;
impl TerminalPort for GhosttyTerminalPort {
    fn backend_name(&self) -> &'static str {
        "Ghostty"
    }
    fn spawn(
        &self,
        session_id: Uuid,
        directory: &Path,
        environment: &HashMap<String, String>,
    ) -> Result<Arc<dyn TerminalHandle>> {
        let handle = GhosttyTerminal::spawn(session_id, directory, environment, None)?
            as Arc<dyn TerminalHandle>;
        Ok(handle)
    }
}
enum PtyInput {
    Bytes(Vec<u8>),
    Key(TerminalKeyInput),
}
struct PtyWorkerControl {
    inputs: mpsc::Receiver<PtyInput>,
    signal: UnixStream,
    shutdown_requested: Arc<AtomicBool>,
    pending_resize: Arc<Mutex<Option<TerminalSize>>>,
}

/// Poisoned native state is terminal: do not recover its mutex and call FFI
/// again. The independent snapshot and failure state contain only Rust data.
struct TerminalEngine {
    inner: Mutex<Engine>,
    failed: AtomicBool,
    failure: Mutex<Option<String>>,
    last_snapshot: Mutex<Arc<TerminalSnapshot>>,
    events: Sender<TerminalEvent>,
    shutdown_requested: Arc<AtomicBool>,
}

impl TerminalEngine {
    fn fail(&self, message: impl Into<String>) {
        let mut failure = self
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if failure.is_none() {
            let message = message.into();
            // Publish the failure before the worker can observe shutdown.
            let _ = self
                .events
                .force_send(TerminalEvent::Failed(message.clone()));
            *failure = Some(message);
            self.failed.store(true, Ordering::Release);
            self.shutdown_requested.store(true, Ordering::Release);
        }
    }

    fn failure(&self) -> anyhow::Error {
        let failure = self
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        anyhow::anyhow!(
            "{}",
            failure.as_deref().unwrap_or("The terminal engine failed.")
        )
    }

    fn lock(&self) -> Result<MutexGuard<'_, Engine>> {
        if self.failed.load(Ordering::Acquire) {
            return Err(self.failure());
        }
        match self.inner.lock() {
            Ok(engine) if !self.failed.load(Ordering::Acquire) => Ok(engine),
            Ok(_) => Err(self.failure()),
            Err(_) => {
                self.fail("The terminal engine failed. Close this pane and open a new terminal.");
                Err(self.failure())
            }
        }
    }

    fn update<T>(&self, operation: impl FnOnce(&mut Engine) -> Result<T>) -> Result<T> {
        let mut engine = self.lock()?;
        operation(&mut engine).inspect_err(|error| {
            self.fail(format!(
                "The terminal engine failed: {error}. Open a new terminal."
            ));
        })
    }
}

fn enqueue_replies(
    writes: &mut VecDeque<(PtyInput, usize)>,
    queued_bytes: &mut usize,
    engine: &mut Engine,
) {
    if !engine.callbacks.replies.is_empty() {
        let bytes: Vec<_> = engine.callbacks.replies.drain(..).collect();
        let len = bytes.len();
        *queued_bytes += len;
        writes.push_back((PtyInput::Bytes(bytes), len));
    }
}
struct GhosttyTerminal {
    engine: Arc<TerminalEngine>,
    inputs: mpsc::SyncSender<PtyInput>,
    events: Receiver<TerminalEvent>,
    wakeup: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    shutdown_requested: Arc<AtomicBool>,
    pending_resize: Arc<Mutex<Option<TerminalSize>>>,
    pid: u32,
    probe: File,
    signal: UnixStream,
}
fn window_size(s: TerminalSize) -> libc::winsize {
    libc::winsize {
        ws_col: s.columns.max(1),
        ws_row: s.rows.max(1),
        ws_xpixel: (f32::from(s.columns) * s.cell_width) as u16,
        ws_ypixel: (f32::from(s.rows) * s.cell_height) as u16,
    }
}
impl GhosttyTerminal {
    fn spawn(
        id: Uuid,
        directory: &Path,
        environment: &HashMap<String, String>,
        shell: Option<(&str, &[&str])>,
    ) -> Result<Arc<Self>> {
        let size = TerminalSize::default();
        let (tx, events) = async_channel::bounded(MAX_TERMINAL_EVENTS);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let engine = Arc::new(TerminalEngine {
            inner: Mutex::new(Engine::new(size, tx.clone())?),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            last_snapshot: Mutex::new(Arc::new(TerminalSnapshot::default())),
            events: tx.clone(),
            shutdown_requested: shutdown_requested.clone(),
        });
        let mut master = -1;
        let mut slave = -1;
        let mut winsize = window_size(size);
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut winsize,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        // Own both descriptors immediately so every error path closes them.
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        for file in [&master, &slave] {
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        let default_shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let (program, args) = shell.unwrap_or((&default_shell, &["-l"]));
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(directory)
            .envs(terminal_child_environment(id, environment))
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        for key in COLOR_SUPPRESSING_ENV {
            command.env_remove(key);
        }
        // Only async-signal-safe syscalls run after fork. std::process handles
        // dup2 and closes its inherited pipe ends before this closure.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for signal in [
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGHUP,
                    libc::SIGPIPE,
                    libc::SIGCHLD,
                    libc::SIGTSTP,
                    libc::SIGTTIN,
                    libc::SIGTTOU,
                ] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                let mut mask = std::mem::zeroed();
                libc::sigemptyset(&mut mask);
                libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
                Ok(())
            });
        }
        // Configure before spawning: failure must not leave a child behind.
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let probe = master.try_clone()?;
        let (signal, worker_signal) = UnixStream::pair()?;
        signal.set_nonblocking(true)?;
        worker_signal.set_nonblocking(true)?;
        let child = command
            .spawn()
            .with_context(|| format!("could not start {program}"))?;
        let pid = child.id();
        let (inputs, input_rx) = mpsc::sync_channel(MAX_QUEUED_PTY_INPUTS);
        let wakeup = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let pending_resize = Arc::new(Mutex::new(None));
        let worker_engine = engine.clone();
        let worker_wakeup = wakeup.clone();
        let worker_alive = alive.clone();
        let worker_shutdown_requested = shutdown_requested.clone();
        let worker_pending_resize = pending_resize.clone();
        // A failed thread spawn drops Child without reaping it; retain it in a
        // shared slot until the worker actually starts to cover that error path.
        let child_slot = Arc::new(Mutex::new(Some(child)));
        let worker_child = child_slot.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("ghostty-pty-{pid}"))
            .spawn(move || {
                let Some(child) = worker_child
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take()
                else {
                    return;
                };
                pty_worker(
                    master,
                    child,
                    PtyWorkerControl {
                        inputs: input_rx,
                        signal: worker_signal,
                        shutdown_requested: worker_shutdown_requested,
                        pending_resize: worker_pending_resize,
                    },
                    worker_engine,
                    tx,
                    worker_wakeup,
                    worker_alive,
                );
            })
        {
            if let Some(mut child) = child_slot
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(error.into());
        }
        Ok(Arc::new(Self {
            engine,
            inputs,
            events,
            wakeup,
            alive,
            shutdown_requested,
            pending_resize,
            pid,
            probe,
            signal,
        }))
    }
    fn enqueue_input(&self, input: PtyInput) -> Result<()> {
        if self.engine.failed.load(Ordering::Acquire) {
            return Err(self.engine.failure());
        }
        self.inputs.try_send(input).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => {
                anyhow::anyhow!("the terminal input queue is full")
            }
            mpsc::TrySendError::Disconnected(_) => anyhow::anyhow!("PTY closed"),
        })?;
        // A full socket already contains a wakeup. The input queue is the
        // source of truth, so the signal carries no payload and can coalesce.
        let _ = (&self.signal).write(&[1]);
        Ok(())
    }
    fn foreground(&self) -> Option<u32> {
        if !self.alive.load(Ordering::Acquire) {
            return None;
        }
        let pid = unsafe { libc::tcgetpgrp(self.probe.as_raw_fd()) };
        (pid > 0).then_some(pid as u32)
    }
}
fn wake(events: &Sender<TerminalEvent>, pending: &AtomicBool) -> bool {
    if pending.swap(true, Ordering::AcqRel) {
        return true;
    }
    if events.try_send(TerminalEvent::Wakeup).is_ok() {
        true
    } else {
        pending.store(false, Ordering::Release);
        false
    }
}

fn flush_pending_writes(
    master: &mut File,
    engine: &Arc<TerminalEngine>,
    writes: &mut VecDeque<(PtyInput, usize)>,
    queued_bytes: &mut usize,
    write_offset: &mut usize,
) -> Result<()> {
    for _ in 0..16 {
        let Some((input, cost)) = writes.front_mut() else {
            break;
        };
        let encoded = if let PtyInput::Key(key) = input {
            // Encode after reading output, which may have changed keyboard mode.
            Some(key.bytes(engine.update(|engine| engine.try_mode())?))
        } else {
            None
        };
        let data = match (encoded.as_ref(), &*input) {
            (Some(data), _) | (_, PtyInput::Bytes(data)) => data,
            _ => unreachable!(),
        };
        let len = data.len();
        if *write_offset == len {
            *queued_bytes -= *cost;
            writes.pop_front();
            *write_offset = 0;
            continue;
        }
        match master.write(&data[*write_offset..]) {
            Ok(0) => break,
            Ok(n) => {
                *write_offset += n;
                if *write_offset == len {
                    *queued_bytes -= *cost;
                    writes.pop_front();
                    *write_offset = 0;
                } else if let Some(data) = encoded {
                    // Preserve a partially written key sequence verbatim.
                    *input = PtyInput::Bytes(data);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                writes.clear();
                *queued_bytes = 0;
                *write_offset = 0;
                break;
            }
        }
    }
    Ok(())
}

fn pty_worker(
    mut master: File,
    mut child: Child,
    control: PtyWorkerControl,
    engine: Arc<TerminalEngine>,
    events: Sender<TerminalEvent>,
    pending: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
) {
    let PtyWorkerControl {
        inputs,
        mut signal,
        shutdown_requested,
        pending_resize,
    } = control;
    let mut output = [0u8; 65536];
    let mut writes = VecDeque::new();
    let mut queued_bytes = 0usize;
    let mut write_offset = 0;
    let mut shutdown = None;
    let mut exit = None;
    let mut read_closed = false;
    let mut refresh_needed = false;
    loop {
        if shutdown_requested.load(Ordering::Acquire) {
            shutdown.get_or_insert_with(Instant::now);
        }
        if shutdown.is_none()
            && let Some(size) = pending_resize
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
        {
            let resize_result = engine.update(|engine| {
                if engine.size == size {
                    Ok(())
                } else {
                    let ws = window_size(size);
                    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws) } == 0 {
                        engine.resize(size)?;
                        refresh_needed = true;
                    }
                    Ok(())
                }
            });
            if resize_result.is_err() {
                shutdown.get_or_insert_with(Instant::now);
            }
        }
        for _ in 0..MAX_QUEUED_PTY_INPUTS {
            if queued_bytes >= MAX_PENDING_PTY_WRITE_BYTES || shutdown.is_some() {
                break;
            }
            match inputs.try_recv() {
                Ok(input) => {
                    let size = match &input {
                        PtyInput::Bytes(bytes) => bytes.len(),
                        PtyInput::Key(_) => 64,
                    };
                    queued_bytes += size;
                    writes.push_back((input, size));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    shutdown.get_or_insert_with(Instant::now);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if let Some(start) = shutdown
            && exit.is_none()
        {
            let signal = if start.elapsed() > Duration::from_millis(500) {
                libc::SIGKILL
            } else {
                libc::SIGHUP
            };
            // Child has not been reaped, so its PID cannot have been reused.
            unsafe {
                let fg = libc::tcgetpgrp(master.as_raw_fd());
                if fg > 0 {
                    libc::kill(-fg, signal);
                }
                libc::kill(-(child.id() as i32), signal);
            }
        }
        // Resize can emit in-band reports even when the child is waiting and
        // produces no further output. Flush every callback batch, not just feed.
        if !engine.failed.load(Ordering::Acquire) {
            let _ = engine.update(|engine| {
                enqueue_replies(&mut writes, &mut queued_bytes, engine);
                Ok(())
            });
        }
        let mut changed = false;
        // Bound each batch so continuous output cannot starve shutdown/input.
        for _ in 0..16 {
            if queued_bytes >= MAX_PENDING_PTY_WRITE_BYTES {
                break;
            }
            match master.read(&mut output) {
                Ok(0) => {
                    read_closed = true;
                    break;
                }
                Ok(n) => {
                    if !engine.failed.load(Ordering::Acquire) {
                        changed |= engine
                            .update(|engine| {
                                engine.feed(&output[..n]);
                                enqueue_replies(&mut writes, &mut queued_bytes, engine);
                                Ok(())
                            })
                            .is_ok();
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    read_closed = true;
                    break;
                }
            }
        }
        if changed {
            refresh_needed = true;
        }
        if refresh_needed && wake(&events, &pending) {
            refresh_needed = false;
        }
        if engine.failed.load(Ordering::Acquire) {
            writes.clear();
            queued_bytes = 0;
            write_offset = 0;
            shutdown.get_or_insert_with(Instant::now);
        } else if flush_pending_writes(
            &mut master,
            &engine,
            &mut writes,
            &mut queued_bytes,
            &mut write_offset,
        )
        .is_err()
        {
            shutdown.get_or_insert_with(Instant::now);
        }
        if exit.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    exit = Some((status.code(), Instant::now()));
                    alive.store(false, Ordering::Release);
                }
                Ok(None) => {}
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    exit = Some((None, Instant::now()));
                    alive.store(false, Ordering::Release);
                }
            }
        }
        if let Some((code, at)) = exit
            && (read_closed || !changed || at.elapsed() > Duration::from_millis(200))
        {
            // Exit must survive a full queue without keeping the reaper thread
            // alive after the UI stops consuming events.
            let _ = events.force_send(TerminalEvent::Exit(code));
            break;
        }
        if read_closed && shutdown.is_none() {
            shutdown = Some(Instant::now());
        }
        let mut polls = [
            libc::pollfd {
                fd: if read_closed { -1 } else { master.as_raw_fd() },
                events: if queued_bytes >= MAX_PENDING_PTY_WRITE_BYTES {
                    0
                } else {
                    libc::POLLIN
                } | if writes.is_empty() { 0 } else { libc::POLLOUT },
                revents: 0,
            },
            libc::pollfd {
                fd: signal.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // Input/resize/close wake immediately. The timeout is only for child
        // reaping (descendants can retain the slave) and shutdown escalation.
        unsafe {
            libc::poll(
                polls.as_mut_ptr(),
                2,
                if shutdown.is_some() { 10 } else { 100 },
            );
        }
        let mut notifications = [0u8; 256];
        while let Ok(n) = signal.read(&mut notifications) {
            if n == 0 {
                break;
            }
        }
    }
}
impl Drop for GhosttyTerminal {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl TerminalHandle for GhosttyTerminal {
    fn events(&self) -> Receiver<TerminalEvent> {
        self.events.clone()
    }
    fn send_input(&self, input: Vec<u8>) -> Result<()> {
        if !self.alive.load(Ordering::Acquire) {
            bail!("the terminal has already exited")
        }
        if input.len() > MAX_PTY_INPUT_BYTES {
            bail!("terminal input is too large")
        }
        self.enqueue_input(PtyInput::Bytes(input))
    }
    fn send_key_input(&self, input: TerminalKeyInput) -> Result<()> {
        if !self.alive.load(Ordering::Acquire) {
            bail!("the terminal has already exited")
        }
        self.enqueue_input(PtyInput::Key(input))
    }
    fn resize(&self, size: TerminalSize) -> Result<()> {
        if self.engine.failed.load(Ordering::Acquire) {
            return Err(self.engine.failure());
        }
        *self
            .pending_resize
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(size);
        let _ = (&self.signal).write(&[1]);
        Ok(())
    }
    fn scroll(&self, lines: i32) {
        let _ = self.engine.update(|engine| {
            unsafe { vg_scroll(engine.p(), -i64::from(lines)) };
            engine.dirty = true;
            Ok(())
        });
    }
    fn clear_scrollback(&self) {
        let _ = self.engine.update(|engine| {
            checked(unsafe { vg_clear_history(engine.p()) })?;
            engine.dirty = true;
            Ok(())
        });
    }
    fn snapshot(&self) -> Arc<TerminalSnapshot> {
        let mut snapshot = self
            .engine
            .last_snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Ok(current) = self.engine.update(|engine| engine.snapshot()) {
            *snapshot = current;
        }
        snapshot.clone()
    }
    fn input_mode(&self) -> TerminalInputMode {
        self.engine
            .update(|engine| engine.try_mode())
            .unwrap_or_default()
    }
    fn current_working_directory(&self) -> Option<PathBuf> {
        if !self.alive.load(Ordering::Acquire) {
            return None;
        }
        process_working_directory(self.pid)
            .or_else(|| self.foreground().and_then(process_working_directory))
    }
    fn foreground_process_name(&self) -> Option<String> {
        self.foreground()
            .and_then(process_invoked_name)
            .or_else(|| {
                self.alive
                    .load(Ordering::Acquire)
                    .then(|| process_invoked_name(self.pid))
                    .flatten()
            })
    }
    fn foreground_process_id(&self) -> Option<u32> {
        self.foreground()
    }
    fn recent_text(&self, lines: usize) -> Option<String> {
        let e = self.engine.lock().ok()?;
        let mut n = 0;
        let p = unsafe { vg_recent_text(e.p(), &mut n, lines.max(1)) };
        if p.is_null() {
            return None;
        }
        let text = String::from_utf8_lossy(unsafe { bytes(p, n) }).into_owned();
        unsafe { vg_buffer_free(p, n) };
        Some(text)
    }
    fn clear_selection(&self) {
        let _ = self.engine.update(|engine| {
            engine.try_select(
                0,
                TerminalSelectionType::Simple,
                TerminalPoint::default(),
                TerminalCellSide::Left,
            )
        });
    }
    fn start_selection(
        &self,
        kind: TerminalSelectionType,
        point: TerminalPoint,
        side: TerminalCellSide,
    ) {
        let _ = self
            .engine
            .update(|engine| engine.try_select(1, kind, point, side));
    }
    fn update_selection(&self, point: TerminalPoint, side: TerminalCellSide) {
        let _ = self
            .engine
            .update(|engine| engine.try_select(2, TerminalSelectionType::Simple, point, side));
    }
    fn selection_text(&self) -> Option<String> {
        self.engine.lock().ok()?.text(true)
    }
    fn search(&self, query: &str, direction: TerminalSearchDirection) -> Result<bool> {
        self.engine.update(|e| {
            let r = unsafe {
                vg_search(
                    e.p(),
                    query.as_ptr(),
                    query.len(),
                    (direction == TerminalSearchDirection::Previous).into(),
                )
            };
            e.dirty = true;
            if r < 0 {
                bail!("Ghostty search: {r}")
            }
            Ok(r == 1)
        })
    }
    fn search_step(
        &self,
        query: &str,
        direction: TerminalSearchDirection,
        continuation: bool,
    ) -> Result<Option<bool>> {
        self.engine.update(|e| {
            let result = unsafe {
                vg_search_step(
                    e.p(),
                    query.as_ptr(),
                    query.len(),
                    (direction == TerminalSearchDirection::Previous).into(),
                    continuation.into(),
                )
            };
            e.dirty = true;
            match result {
                0 => Ok(Some(false)),
                1 => Ok(Some(true)),
                2 => Ok(None),
                code => bail!("Ghostty search: {code}"),
            }
        })
    }
    fn hyperlink_at(&self, point: TerminalPoint) -> Option<String> {
        let s = self.snapshot();
        let line = s.lines.get(point.row)?;
        let cell = line.get(point.column)?;
        if let Some(uri) = &cell.hyperlink {
            return Some(uri.to_string());
        }
        plain_hyperlink(line, point.column)
    }
    fn acknowledge_wakeup(&self) {
        self.wakeup.store(false, Ordering::Release);
    }
    fn shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::Release);
        let _ = (&self.signal).write(&[1]);
    }
}

fn plain_hyperlink(line: &[TerminalCell], column: usize) -> Option<String> {
    let mut start = column;
    let mut end = column + 1;
    let whitespace = |c: &TerminalCell| c.text().chars().all(char::is_whitespace);
    if whitespace(line.get(column)?) {
        return None;
    }
    while start > 0 && !whitespace(&line[start - 1]) {
        start -= 1;
    }
    while end < line.len() && !whitespace(&line[end]) {
        end += 1;
    }
    let raw = line[start..end]
        .iter()
        .map(TerminalCell::text)
        .collect::<String>();
    let cursor_byte: usize = line[start..column].iter().map(|c| c.text().len()).sum();
    let trimmed = raw.trim_start_matches(['(', '[', '{', '<', '\'', '"']);
    let leading = raw.len() - trimmed.len();
    let uri =
        trimmed.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '\'', '"']);
    (cursor_byte >= leading && cursor_byte < leading + uri.len() && is_safe_hyperlink(uri))
        .then(|| uri.to_owned())
}

#[cfg(test)]
mod tests;
