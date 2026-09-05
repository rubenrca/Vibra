//! Experimental libghostty-vt backend. The terminal state is protected by one
//! mutex; callbacks only enqueue data and never reenter it. The PTY worker owns
//! the child and reaps it, independently of the lifetime of the UI handle.
use super::alacritty::{
    COLOR_SUPPRESSING_ENV, indexed_color_with, process_invoked_name, process_working_directory,
    terminal_child_environment,
};
use crate::{ports::terminal::*, ui::theme};
use anyhow::{Context, Result, bail};
use async_channel::{Receiver, Sender};
use std::{
    collections::{HashMap, VecDeque},
    ffi::c_void,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr::NonNull,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[repr(C)]
#[derive(Default)]
struct Info {
    columns: u16,
    rows: u16,
    cursor_x: u16,
    cursor_y: u16,
    cursor_visible: u8,
    cursor_blinking: u8,
    cursor_style: u8,
    kitty: u8,
    history: u64,
    offset: u64,
    modes: u32,
}
#[repr(C)]
struct Cell {
    foreground: [u8; 3],
    background: [u8; 3],
    underline_color: [u8; 3],
    bold: u8,
    italic: u8,
    underline: u8,
    strikeout: u8,
    hidden: u8,
    wide_spacer: u8,
    selected: u8,
}
type EventFn = unsafe extern "C" fn(*mut c_void, i32, *const u8, usize);
type PaintFn =
    unsafe extern "C" fn(*mut c_void, u16, u16, *const Cell, *const u8, usize, *const u8, usize);
unsafe extern "C" {
    fn vg_new(c: u16, r: u16, event: EventFn, data: *mut c_void) -> *mut c_void;
    fn vg_free(p: *mut c_void);
    fn vg_feed(p: *mut c_void, s: *const u8, n: usize);
    fn vg_resize(p: *mut c_void, c: u16, r: u16, w: u32, h: u32) -> i32;
    fn vg_palette(p: *mut c_void, rgb: *const u8) -> i32;
    fn vg_snapshot(
        p: *mut c_void,
        info: *mut Info,
        paint: Option<PaintFn>,
        data: *mut c_void,
    ) -> i32;
    fn vg_scroll(p: *mut c_void, delta: i64);
    fn vg_bottom(p: *mut c_void);
    fn vg_clear_history(p: *mut c_void) -> i32;
    fn vg_select(p: *mut c_void, action: i32, kind: i32, x: u16, y: u16, right: i32) -> i32;
    fn vg_search(p: *mut c_void, s: *const u8, n: usize, previous: i32) -> i32;
    fn vg_text(p: *mut c_void, n: *mut usize, selection: i32) -> *mut u8;
    fn vg_buffer_free(p: *mut u8, n: usize);
    fn vg_recent_text(p: *mut c_void, n: *mut usize, lines: usize) -> *mut u8;
}
fn checked(code: i32) -> Result<()> {
    if code != 0 {
        bail!("libghostty-vt devolvió {code}")
    }
    Ok(())
}
struct Callbacks {
    events: Sender<TerminalEvent>,
    replies: VecDeque<u8>,
}
unsafe fn bytes<'a>(p: *const u8, n: usize) -> &'a [u8] {
    if n == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(p, n) }
    }
}
unsafe extern "C" fn event(data: *mut c_void, kind: i32, p: *const u8, n: usize) {
    // SAFETY: userdata points to a stable Box owned by Engine; terminal calls
    // and destruction are serialized by the engine mutex.
    let context = unsafe { &mut *data.cast::<Callbacks>() };
    let data = unsafe { bytes(p, n) };
    match kind {
        0 => context.replies.extend(data),
        1 => {
            let _ = context.events.try_send(TerminalEvent::Bell);
        }
        2 => {
            let title = String::from_utf8_lossy(data).into_owned();
            let _ = context.events.try_send(if title.is_empty() {
                TerminalEvent::ResetTitle
            } else {
                TerminalEvent::Title(title)
            });
        }
        3 => {
            let _ = context.events.try_send(TerminalEvent::ClipboardStore(
                String::from_utf8_lossy(data).into_owned(),
            ));
        }
        _ => {}
    }
}
unsafe extern "C" fn paint(
    data: *mut c_void,
    x: u16,
    y: u16,
    cell: *const Cell,
    p: *const u8,
    n: usize,
    uri: *const u8,
    uri_len: usize,
) {
    let lines = unsafe { &mut *data.cast::<Vec<Vec<TerminalCell>>>() };
    let c = unsafe { &*cell };
    let Some(row) = lines.get_mut(y as usize) else {
        return;
    };
    let Some(slot) = row.get_mut(x as usize) else {
        return;
    };
    let rgb = |a: [u8; 3]| TerminalRgb::new(a[0], a[1], a[2]);
    let text = String::from_utf8_lossy(unsafe { bytes(p, n) });
    let mut out = TerminalCell::with_text(
        y as usize,
        x as usize,
        if text.is_empty() { " " } else { &text },
        rgb(c.foreground),
        rgb(c.background),
    );
    out.underline_color = rgb(c.underline_color);
    out.bold = c.bold != 0;
    out.italic = c.italic != 0;
    out.underline = match c.underline {
        1 => TerminalUnderline::Single,
        2 => TerminalUnderline::Double,
        3 => TerminalUnderline::Curly,
        4 => TerminalUnderline::Dotted,
        5 => TerminalUnderline::Dashed,
        _ => TerminalUnderline::None,
    };
    out.strikeout = c.strikeout != 0;
    out.hidden = c.hidden != 0;
    out.wide_spacer = c.wide_spacer != 0;
    out.selected = c.selected != 0;
    if uri_len != 0 {
        out.set_hyperlink(Some(&String::from_utf8_lossy(unsafe {
            bytes(uri, uri_len)
        })));
    }
    *slot = out;
}
struct Engine {
    ptr: NonNull<c_void>,
    callbacks: Box<Callbacks>,
    cache: Option<Arc<TerminalSnapshot>>,
    dirty: bool,
    theme_generation: u64,
    size: TerminalSize,
}
// SAFETY: no upstream state is accessed concurrently. Engine is only exposed
// through Mutex, and borrowed callback/string pointers never escape a call.
unsafe impl Send for Engine {}
impl Drop for Engine {
    fn drop(&mut self) {
        unsafe { vg_free(self.ptr.as_ptr()) }
    }
}
impl Engine {
    fn new(size: TerminalSize, events: Sender<TerminalEvent>) -> Result<Self> {
        let mut callbacks = Box::new(Callbacks {
            events,
            replies: VecDeque::new(),
        });
        let ptr = NonNull::new(unsafe {
            vg_new(
                size.columns,
                size.rows,
                event,
                (&mut *callbacks as *mut Callbacks).cast(),
            )
        })
        .context("no se pudo crear libghostty-vt")?;
        let mut engine = Self {
            ptr,
            callbacks,
            cache: None,
            dirty: true,
            theme_generation: u64::MAX,
            size,
        };
        engine.resize(size)?;
        engine.update_palette()?;
        Ok(engine)
    }
    fn p(&self) -> *mut c_void {
        self.ptr.as_ptr()
    }
    fn update_palette(&mut self) -> Result<()> {
        let generation = theme::generation();
        if self.theme_generation == generation {
            return Ok(());
        }
        let palette = theme::terminal_palette();
        let mut rgb = Vec::with_capacity(259 * 3);
        for i in 0..259 {
            let c = indexed_color_with(i, &palette);
            rgb.extend([c.red, c.green, c.blue]);
        }
        checked(unsafe { vg_palette(self.p(), rgb.as_ptr()) })?;
        self.theme_generation = generation;
        self.dirty = true;
        Ok(())
    }
    fn feed(&mut self, data: &[u8]) {
        unsafe { vg_feed(self.p(), data.as_ptr(), data.len()) };
        self.dirty = true;
    }
    fn resize(&mut self, size: TerminalSize) -> Result<()> {
        checked(unsafe {
            vg_resize(
                self.p(),
                size.columns.max(1),
                size.rows.max(1),
                size.cell_width.max(1.0) as u32,
                size.cell_height.max(1.0) as u32,
            )
        })?;
        self.size = size;
        self.dirty = true;
        Ok(())
    }
    fn snapshot(&mut self) -> Result<Arc<TerminalSnapshot>> {
        self.update_palette()?;
        if !self.dirty
            && let Some(cache) = &self.cache
        {
            return Ok(cache.clone());
        }
        let mut info = Info::default();
        checked(unsafe { vg_snapshot(self.p(), &mut info, None, std::ptr::null_mut()) })?;
        let mut lines: Vec<Vec<TerminalCell>> = (0..info.rows as usize)
            .map(|y| {
                (0..info.columns as usize)
                    .map(|x| TerminalCell::blank(y, x))
                    .collect()
            })
            .collect();
        checked(unsafe {
            vg_snapshot(
                self.p(),
                &mut info,
                Some(paint),
                (&mut lines as *mut Vec<Vec<TerminalCell>>).cast(),
            )
        })?;
        let lines = lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| {
                if let Some(old) = self.cache.as_ref().and_then(|c| c.lines.get(i))
                    && old.as_ref() == line.as_slice()
                {
                    return old.clone();
                }
                Arc::from(line)
            })
            .collect();
        let cursor = (info.cursor_visible != 0).then_some(TerminalCursor {
            row: info.cursor_y as usize,
            column: info.cursor_x as usize,
            shape: match info.cursor_style {
                0 => TerminalCursorShape::Beam,
                2 => TerminalCursorShape::Underline,
                3 => TerminalCursorShape::HollowBlock,
                _ => TerminalCursorShape::Block,
            },
            blinking: info.cursor_blinking != 0,
        });
        let snapshot = Arc::new(TerminalSnapshot {
            columns: info.columns as usize,
            rows: info.rows as usize,
            lines,
            cursor,
            display_offset: info.offset as usize,
            history_size: info.history as usize,
        });
        self.cache = Some(snapshot.clone());
        self.dirty = false;
        Ok(snapshot)
    }
    fn mode(&self) -> TerminalInputMode {
        let mut i = Info::default();
        if unsafe { vg_snapshot(self.p(), &mut i, None, std::ptr::null_mut()) } != 0 {
            return TerminalInputMode::default();
        }
        let m = |b: u32| i.modes & (1u32 << b) != 0u32;
        let k = |b: u32| i.kitty & (1u8 << b) != 0u8;
        TerminalInputMode {
            application_cursor: m(0),
            bracketed_paste: m(1),
            alternate_screen: m(2),
            alternate_scroll: m(3),
            focus_reporting: m(4),
            mouse_report_click: m(5),
            mouse_drag: m(6),
            mouse_motion: m(7),
            sgr_mouse: m(8),
            utf8_mouse: m(9),
            disambiguate_escape_codes: k(0),
            report_event_types: k(1),
            report_alternate_keys: k(2),
            report_all_keys_as_escape_codes: k(3),
            report_associated_text: k(4),
        }
    }
    fn text(&self, selection: bool) -> Option<String> {
        let mut n = 0;
        let p = unsafe { vg_text(self.p(), &mut n, selection.into()) };
        if p.is_null() {
            return None;
        }
        let s = String::from_utf8_lossy(unsafe { bytes(p, n) }).into_owned();
        unsafe { vg_buffer_free(p, n) };
        Some(s)
    }
    fn select(
        &mut self,
        action: i32,
        kind: TerminalSelectionType,
        point: TerminalPoint,
        side: TerminalCellSide,
    ) {
        let kind = match kind {
            TerminalSelectionType::Simple => 0,
            TerminalSelectionType::Block => 1,
            TerminalSelectionType::Semantic => 2,
            TerminalSelectionType::Lines => 3,
        };
        let x = point
            .column
            .min(self.size.columns.saturating_sub(1) as usize) as u16;
        let y = point.row.min(self.size.rows.saturating_sub(1) as usize) as u16;
        let _ = unsafe {
            vg_select(
                self.p(),
                action,
                kind,
                x,
                y,
                (side == TerminalCellSide::Right).into(),
            )
        };
        self.dirty = true;
    }
}

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
        Ok(
            GhosttyTerminal::spawn(session_id, directory, environment, None)?
                as Arc<dyn TerminalHandle>,
        )
    }
}
enum PtyCommand {
    Input(Vec<u8>),
    Resize(TerminalSize),
    Shutdown,
}
struct GhosttyTerminal {
    engine: Arc<Mutex<Engine>>,
    commands: mpsc::Sender<PtyCommand>,
    events: Receiver<TerminalEvent>,
    wakeup: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    pid: u32,
    probe: File,
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
        let (tx, events) = async_channel::unbounded();
        let engine = Arc::new(Mutex::new(Engine::new(size, tx.clone())?));
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
        let child = command
            .spawn()
            .with_context(|| format!("no se pudo iniciar {program}"))?;
        let pid = child.id();
        let (commands, rx) = mpsc::channel();
        let wakeup = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let worker_engine = engine.clone();
        let worker_wakeup = wakeup.clone();
        let worker_alive = alive.clone();
        // A failed thread spawn drops Child without reaping it; retain it in a
        // shared slot until the worker actually starts to cover that error path.
        let child_slot = Arc::new(Mutex::new(Some(child)));
        let worker_child = child_slot.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("ghostty-pty-{pid}"))
            .spawn(move || {
                let child = worker_child.lock().unwrap().take().unwrap();
                pty_worker(
                    master,
                    child,
                    rx,
                    worker_engine,
                    tx,
                    worker_wakeup,
                    worker_alive,
                );
            })
        {
            if let Some(mut child) = child_slot.lock().unwrap().take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(error.into());
        }
        Ok(Arc::new(Self {
            engine,
            commands,
            events,
            wakeup,
            alive,
            pid,
            probe,
        }))
    }
    fn foreground(&self) -> Option<u32> {
        if !self.alive.load(Ordering::Acquire) {
            return None;
        }
        let pid = unsafe { libc::tcgetpgrp(self.probe.as_raw_fd()) };
        (pid > 0).then_some(pid as u32)
    }
}
fn wake(events: &Sender<TerminalEvent>, pending: &AtomicBool) {
    if !pending.swap(true, Ordering::AcqRel) {
        let _ = events.try_send(TerminalEvent::Wakeup);
    }
}
fn pty_worker(
    mut master: File,
    mut child: Child,
    commands: mpsc::Receiver<PtyCommand>,
    engine: Arc<Mutex<Engine>>,
    events: Sender<TerminalEvent>,
    pending: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
) {
    let mut output = [0u8; 65536];
    let mut writes = VecDeque::new();
    let mut shutdown = None;
    let mut exit = None;
    let mut read_closed = false;
    loop {
        loop {
            match commands.try_recv() {
                Ok(PtyCommand::Input(data)) => writes.extend(data),
                Ok(PtyCommand::Resize(size)) => {
                    let ws = window_size(size);
                    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws) } == 0 {
                        let _ = engine.lock().unwrap().resize(size);
                        wake(&events, &pending);
                    }
                }
                Ok(PtyCommand::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
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
        let mut changed = false;
        // Bound each batch so continuous output cannot starve shutdown/input.
        for _ in 0..16 {
            match master.read(&mut output) {
                Ok(0) => {
                    read_closed = true;
                    break;
                }
                Ok(n) => {
                    let mut e = engine.lock().unwrap();
                    e.feed(&output[..n]);
                    writes.append(&mut e.callbacks.replies);
                    changed = true;
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
            wake(&events, &pending);
        }
        for _ in 0..16 {
            if writes.is_empty() {
                break;
            }
            match master.write(writes.as_slices().0) {
                Ok(0) => break,
                Ok(n) => {
                    writes.drain(..n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    writes.clear();
                    break;
                }
            }
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
            let _ = events.try_send(TerminalEvent::Exit(code));
            break;
        }
        if read_closed && shutdown.is_none() {
            shutdown = Some(Instant::now());
        }
        let mut poll = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN | if writes.is_empty() { 0 } else { libc::POLLOUT },
            revents: 0,
        };
        if read_closed {
            thread::sleep(Duration::from_millis(10));
        } else {
            unsafe {
                libc::poll(&mut poll, 1, 10);
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
            bail!("la terminal ya terminó")
        }
        {
            let mut e = self.engine.lock().unwrap();
            unsafe {
                vg_bottom(e.p());
            }
            e.dirty = true;
        }
        self.commands
            .send(PtyCommand::Input(input))
            .context("PTY cerrado")
    }
    fn resize(&self, size: TerminalSize) -> Result<()> {
        self.commands
            .send(PtyCommand::Resize(size))
            .context("PTY cerrado")
    }
    fn scroll(&self, lines: i32) {
        let mut e = self.engine.lock().unwrap();
        unsafe { vg_scroll(e.p(), -i64::from(lines)) };
        e.dirty = true;
    }
    fn clear_scrollback(&self) {
        let mut e = self.engine.lock().unwrap();
        let _ = unsafe { vg_clear_history(e.p()) };
        e.dirty = true;
    }
    fn snapshot(&self) -> Arc<TerminalSnapshot> {
        self.engine
            .lock()
            .unwrap()
            .snapshot()
            .expect("Ghostty snapshot failed")
    }
    fn input_mode(&self) -> TerminalInputMode {
        self.engine.lock().unwrap().mode()
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
    fn session_process_id(&self) -> Option<u32> {
        self.alive.load(Ordering::Acquire).then_some(self.pid)
    }
    fn recent_text(&self, lines: usize) -> Option<String> {
        let e = self.engine.lock().unwrap();
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
        self.engine.lock().unwrap().select(
            0,
            TerminalSelectionType::Simple,
            TerminalPoint::default(),
            TerminalCellSide::Left,
        );
    }
    fn start_selection(
        &self,
        kind: TerminalSelectionType,
        point: TerminalPoint,
        side: TerminalCellSide,
    ) {
        self.engine.lock().unwrap().select(1, kind, point, side);
    }
    fn update_selection(&self, point: TerminalPoint, side: TerminalCellSide) {
        self.engine
            .lock()
            .unwrap()
            .select(2, TerminalSelectionType::Simple, point, side);
    }
    fn selection_text(&self) -> Option<String> {
        self.engine.lock().unwrap().text(true)
    }
    fn search(&self, query: &str, direction: TerminalSearchDirection) -> Result<bool> {
        let mut e = self.engine.lock().unwrap();
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
        let _ = self.commands.send(PtyCommand::Shutdown);
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
    (cursor_byte >= leading
        && cursor_byte < leading + uri.len()
        && ["http://", "https://", "mailto:", "file://"]
            .iter()
            .any(|s| uri.starts_with(s)))
    .then(|| uri.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn engine() -> Engine {
        let (tx, _) = async_channel::unbounded();
        Engine::new(TerminalSize::default(), tx).unwrap()
    }
    #[test]
    fn fragmented_unicode_styles_modes_and_alternate_screen() {
        let mut e = engine();
        for b in "\x1b[38;2;17;101;221mEspañol 日本語 e\u{301} 🦀\x1b[0m".as_bytes() {
            e.feed(&[*b]);
        }
        let s = e.snapshot().unwrap();
        assert_eq!(s.lines[0][0].foreground, TerminalRgb::new(17, 101, 221));
        assert!(e.text(false).unwrap().contains("日本語 e\u{301} 🦀"));
        assert!(s.lines[0].iter().any(|c| c.wide_spacer));
        e.feed(b"\x1b[?2004h\x1b[?1h\x1b[>31u");
        let m = e.mode();
        assert!(m.bracketed_paste && m.application_cursor && m.report_associated_text);
        e.feed(b"\x1b[?1049hALTERNATE");
        assert!(e.mode().alternate_screen);
        assert!(e.text(false).unwrap().contains("ALTERNATE"));
        e.feed(b"\x1b[?1049l");
        assert!(!e.mode().alternate_screen);
        assert!(e.text(false).unwrap().contains("Español"));
    }
    #[test]
    fn snapshot_cache_selection_search_and_hyperlinks() {
        let mut e = engine();
        e.feed(b"hello world\r\n\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\");
        let a = e.snapshot().unwrap();
        assert_eq!(
            a.lines[1][0].hyperlink.as_deref(),
            Some("https://example.com")
        );
        assert!(Arc::ptr_eq(&a, &e.snapshot().unwrap()));
        e.select(
            1,
            TerminalSelectionType::Semantic,
            TerminalPoint { row: 0, column: 7 },
            TerminalCellSide::Left,
        );
        assert_eq!(e.text(true).as_deref(), Some("world"));
        assert!(e.snapshot().unwrap().lines[0][7].selected);
        e.select(
            0,
            TerminalSelectionType::Simple,
            TerminalPoint::default(),
            TerminalCellSide::Left,
        );
        assert_eq!(unsafe { vg_search(e.p(), b"hello".as_ptr(), 5, 0) }, 1);
        assert_eq!(e.text(true).as_deref(), Some("hello"));
        assert_eq!(
            unsafe { vg_search(e.p(), b"not present".as_ptr(), 11, 0) },
            0
        );
    }
    #[test]
    fn replies_and_scrollback_resize() {
        let mut e = engine();
        e.feed(b"\x1b[6n");
        assert_eq!(
            e.callbacks.replies.drain(..).collect::<Vec<_>>(),
            b"\x1b[1;1R"
        );
        for i in 0..100 {
            e.feed(format!("line {i}\r\n").as_bytes());
        }
        assert!(e.snapshot().unwrap().history_size > 0);
        unsafe { vg_scroll(e.p(), -10) };
        e.dirty = true;
        assert!(e.snapshot().unwrap().display_offset > 0);
        e.resize(TerminalSize {
            columns: 40,
            rows: 12,
            ..TerminalSize::default()
        })
        .unwrap();
        assert_eq!(e.snapshot().unwrap().columns, 40);
        checked(unsafe { vg_clear_history(e.p()) }).unwrap();
        e.dirty = true;
        assert_eq!(e.snapshot().unwrap().history_size, 0);
    }
    fn wait_for(t: &GhosttyTerminal, needle: &str) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let text = t.engine.lock().unwrap().text(false).unwrap_or_default();
            if text.contains(needle) {
                return;
            }
            assert!(Instant::now() < until, "missing {needle:?}: {text:?}");
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn real_pty_input_resize_exit_and_environment() {
        let t=GhosttyTerminal::spawn(Uuid::new_v4(),Path::new("/tmp"),&HashMap::new(),Some(("/bin/sh",&["-c","printf 'READY\\n'; read answer; printf 'ANSWER=%s TERM=%s\\n' \"$answer\" \"$TERM\"; stty size; exit 7"]))).unwrap();
        wait_for(&t, "READY");
        assert!(t.foreground_process_id().is_some());
        t.resize(TerminalSize {
            columns: 100,
            rows: 30,
            ..TerminalSize::default()
        })
        .unwrap();
        t.send_input(b"from-vibra\n".to_vec()).unwrap();
        wait_for(&t, "ANSWER=from-vibra TERM=xterm-256color");
        wait_for(&t, "30 100");
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(TerminalEvent::Exit(code)) = t.events.try_recv() {
                assert_eq!(code, Some(7));
                break;
            }
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(t.send_input(vec![b'x']).is_err());
    }
    #[test]
    fn real_pty_ctrl_c_and_query_reply() {
        let t = GhosttyTerminal::spawn(
            Uuid::new_v4(),
            Path::new("/tmp"),
            &HashMap::new(),
            Some((
                "/bin/sh",
                &[
                    "-c",
                    "trap 'printf INTERRUPTED; exit 0' INT; printf READY; read answer",
                ],
            )),
        )
        .unwrap();
        wait_for(&t, "READY");
        t.send_input(vec![3]).unwrap();
        wait_for(&t, "INTERRUPTED");
        let q = GhosttyTerminal::spawn(Uuid::new_v4(), Path::new("/tmp"), &HashMap::new(),
            Some(("/bin/sh", &["-c", "stty -echo -icanon min 1 time 0; printf '\\033[6n'; dd bs=1 count=6 2>/dev/null | od -An -tx1"]))).unwrap();
        wait_for(&q, "52");
        let reply = q.engine.lock().unwrap().text(false).unwrap();
        assert_eq!(
            reply.split_whitespace().collect::<Vec<_>>(),
            ["1b", "5b", "31", "3b", "31", "52"]
        );
    }

    #[test]
    fn clipboard_write_and_selection_drag() {
        let (tx, rx) = async_channel::unbounded();
        let mut e = Engine::new(TerminalSize::default(), tx).unwrap();
        e.feed(b"\x1b]52;c;aGVsbG8=\x07");
        assert!(
            matches!(rx.try_recv(), Ok(TerminalEvent::ClipboardStore(text)) if text == "hello")
        );
        e.feed(b"hello world");
        e.select(
            1,
            TerminalSelectionType::Simple,
            TerminalPoint { row: 0, column: 0 },
            TerminalCellSide::Left,
        );
        e.select(
            2,
            TerminalSelectionType::Simple,
            TerminalPoint { row: 0, column: 4 },
            TerminalCellSide::Right,
        );
        assert_eq!(e.text(true).as_deref(), Some("hello"));
        e.feed(b"\r\n(https://example.com).");
        let snapshot = e.snapshot().unwrap();
        assert_eq!(
            plain_hyperlink(&snapshot.lines[1], 4).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(plain_hyperlink(&snapshot.lines[1], 0), None);
    }

    #[test]
    fn shutdown_reaps_child() {
        let t = GhosttyTerminal::spawn(
            Uuid::new_v4(),
            Path::new("/tmp"),
            &HashMap::new(),
            Some((
                "/bin/sh",
                &[
                    "-c",
                    "trap '' HUP; printf 'READY\\n'; while :; do sleep 1; done",
                ],
            )),
        )
        .unwrap();
        wait_for(&t, "READY");
        t.shutdown();
        let until = Instant::now() + Duration::from_secs(5);
        while t.alive.load(Ordering::Acquire) {
            assert!(Instant::now() < until, "child not reaped");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { libc::waitpid(t.pid as i32, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
    }
}
