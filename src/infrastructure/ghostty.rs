//! libghostty-vt backend. The terminal state is protected by one
//! mutex; callbacks only enqueue data and never reenter it. The PTY worker owns
//! the child and reaps it, independently of the lifetime of the UI handle.
use super::terminal_support::{
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
        unix::{net::UnixStream, process::CommandExt},
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
    painted_cells: u32,
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
    fn vg_remote_palette(p: *mut c_void, rgb: *mut u8) -> i32;
    fn vg_remote_info(p: *mut c_void, info: *mut Info) -> i32;
    fn vg_remote_row(p: *mut c_void, n: *mut usize, row: u16) -> *mut u8;
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
        force: i32,
    ) -> i32;
    fn vg_scroll(p: *mut c_void, delta: i64);
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
        4 => {
            if let Some((prefix, suffix)) = clipboard_template(data) {
                let _ = context
                    .events
                    .try_send(TerminalEvent::ClipboardLoad(Arc::new(move |text| {
                        use base64::Engine as _;
                        format!(
                            "{}{}{}",
                            prefix,
                            base64::engine::general_purpose::STANDARD.encode(text),
                            suffix
                        )
                    })));
            }
        }
        _ => {}
    }
}

// Only accept an empty OSC 52 response. Destination is limited to the protocol's
// clipboard selectors; arbitrary terminal output cannot become a reply template.
fn clipboard_template(data: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(data).ok()?;
    let body = text.strip_prefix("\x1b]52;")?;
    let (destination, suffix) = body.split_once(';')?;
    if !destination.bytes().all(|c| b"cps01234567".contains(&c))
        || !matches!(suffix, "\x07" | "\x1b\\")
    {
        return None;
    }
    Some((format!("\x1b]52;{destination};"), suffix.to_owned()))
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
    let lines = unsafe { &mut *data.cast::<Vec<Arc<[TerminalCell]>>>() };
    let c = unsafe { &*cell };
    let Some(row) = lines.get_mut(y as usize) else {
        return;
    };
    let Some(slot) = Arc::make_mut(row).get_mut(x as usize) else {
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
    #[cfg(test)]
    last_painted_cells: u32,
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
            #[cfg(test)]
            last_painted_cells: 0,
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
        checked(unsafe { vg_snapshot(self.p(), &mut info, None, std::ptr::null_mut(), 0) })?;
        let force = self
            .cache
            .as_ref()
            .is_none_or(|s| s.columns != info.columns as usize || s.rows != info.rows as usize);
        let mut lines: Vec<Arc<[TerminalCell]>> = if force {
            (0..info.rows as usize)
                .map(|y| {
                    (0..info.columns as usize)
                        .map(|x| TerminalCell::blank(y, x))
                        .collect::<Vec<_>>()
                        .into()
                })
                .collect()
        } else {
            self.cache.as_ref().unwrap().lines.clone()
        };
        checked(unsafe {
            vg_snapshot(
                self.p(),
                &mut info,
                Some(paint),
                (&mut lines as *mut Vec<Arc<[TerminalCell]>>).cast(),
                force.into(),
            )
        })?;
        if let Some(old) = &self.cache {
            for (line, previous) in lines.iter_mut().zip(&old.lines) {
                if !Arc::ptr_eq(line, previous) && line.as_ref() == previous.as_ref() {
                    *line = previous.clone();
                }
            }
        }
        #[cfg(test)]
        {
            self.last_painted_cells = info.painted_cells;
        }
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
        if unsafe { vg_snapshot(self.p(), &mut i, None, std::ptr::null_mut(), 0) } != 0 {
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
        let handle = GhosttyTerminal::spawn(session_id, directory, environment, None)?
            as Arc<dyn TerminalHandle>;
        super::remote::hub().register(session_id, &handle);
        Ok(handle)
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
    signal: UnixStream,
    ownership: Mutex<(TerminalSize, bool)>,
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
        let (signal, worker_signal) = UnixStream::pair()?;
        signal.set_nonblocking(true)?;
        worker_signal.set_nonblocking(true)?;
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
                    (rx, worker_signal),
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
            signal,
            ownership: Mutex::new((size, false)),
        }))
    }
    fn dispatch(&self, command: PtyCommand) -> Result<()> {
        self.commands.send(command).context("PTY cerrado")?;
        // A full socket already contains a wakeup. The command queue is the
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
fn wake(events: &Sender<TerminalEvent>, pending: &AtomicBool) {
    if !pending.swap(true, Ordering::AcqRel) {
        let _ = events.try_send(TerminalEvent::Wakeup);
    }
}
fn pty_worker(
    mut master: File,
    mut child: Child,
    control: (mpsc::Receiver<PtyCommand>, UnixStream),
    engine: Arc<Mutex<Engine>>,
    events: Sender<TerminalEvent>,
    pending: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
) {
    let (commands, mut signal) = control;
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
        // Resize can emit in-band reports even when the child is waiting and
        // produces no further output. Flush every callback batch, not just feed.
        writes.append(&mut engine.lock().unwrap().callbacks.replies);
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
        let mut polls = [
            libc::pollfd {
                fd: if read_closed { -1 } else { master.as_raw_fd() },
                events: libc::POLLIN | if writes.is_empty() { 0 } else { libc::POLLOUT },
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
            bail!("la terminal ya terminó")
        }
        let ownership = self.ownership.lock().unwrap();
        if ownership.1 {
            bail!("El iPhone controla esta terminal; recupera el control desde el menú del pane")
        }
        self.dispatch(PtyCommand::Input(input))
    }
    fn resize(&self, size: TerminalSize) -> Result<()> {
        let mut ownership = self.ownership.lock().unwrap();
        ownership.0 = size;
        if ownership.1 {
            return Ok(());
        }
        self.dispatch(PtyCommand::Resize(size))
    }
    fn remote_controlled(&self) -> bool {
        self.ownership.lock().unwrap().1
    }
    fn remote_claim(&self, size: TerminalSize) -> Result<()> {
        let mut ownership = self.ownership.lock().unwrap();
        self.dispatch(PtyCommand::Resize(size))?;
        ownership.1 = true;
        Ok(())
    }
    fn remote_resize(&self, size: TerminalSize) -> Result<()> {
        let ownership = self.ownership.lock().unwrap();
        if !ownership.1 {
            bail!("remote lease released")
        }
        self.dispatch(PtyCommand::Resize(size))
    }
    fn remote_release(&self) {
        let mut ownership = self.ownership.lock().unwrap();
        if ownership.1 {
            let _ = self.dispatch(PtyCommand::Resize(ownership.0));
            ownership.1 = false;
        }
    }
    fn remote_input(&self, input: Vec<u8>) -> Result<()> {
        let ownership = self.ownership.lock().unwrap();
        if !ownership.1 || !self.alive.load(Ordering::Acquire) {
            bail!("No remote controller")
        }
        self.dispatch(PtyCommand::Input(input))
    }
    fn remote_size(&self) -> TerminalSize {
        self.engine.lock().unwrap().size
    }
    fn remote_frame(&self) -> Result<RemoteFrame> {
        if !self.alive.load(Ordering::Acquire) {
            bail!("terminal closed")
        }
        let e = self.engine.lock().unwrap();
        let mut info = Info::default();
        checked(unsafe { vg_remote_info(e.p(), &mut info) })?;
        let mut lines = Vec::with_capacity(info.rows as usize);
        for row in 0..info.rows {
            let mut n = 0;
            let p = unsafe { vg_remote_row(e.p(), &mut n, row) };
            if p.is_null() {
                bail!("remote formatter failed")
            }
            let text =
                String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, n) }).into_owned();
            unsafe { vg_buffer_free(p, n) };
            lines.push(text);
        }
        let mut rgb = [0u8; 259 * 3];
        checked(unsafe { vg_remote_palette(e.p(), rgb.as_mut_ptr()) })?;
        let mut palette = String::new();
        for (index, color) in rgb.chunks_exact(3).enumerate() {
            let code = if index < 256 {
                format!("4;{index}")
            } else {
                (index - 256 + 10).to_string()
            };
            palette.push_str(&format!(
                "\x1b]{code};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                color[0], color[1], color[2]
            ));
        }
        Ok(RemoteFrame {
            columns: info.columns,
            rows: info.rows,
            lines,
            palette,
            cursor: format!(
                "\x1b[0m\x1b[{} q\x1b[{};{}H\x1b[?25{}",
                (match info.cursor_style {
                    0 => 6,
                    2 => 4,
                    _ => 2,
                }) - u8::from(info.cursor_blinking != 0),
                info.cursor_y + 1,
                info.cursor_x + 1,
                if info.cursor_visible != 0 { "h" } else { "l" }
            ),
        })
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
        let _ = self.dispatch(PtyCommand::Shutdown);
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
    fn remote_export_preserves_local_viewport_selection_and_damage() {
        let t = GhosttyTerminal::spawn(
            Uuid::new_v4(),
            Path::new("/tmp"),
            &HashMap::new(),
            Some(("/bin/sleep", &["60"])),
        )
        .unwrap();
        {
            let mut e = t.engine.lock().unwrap();
            for n in 0..100 {
                e.feed(format!("history-{n}\r\n").as_bytes());
            }
            e.feed("\x1b[2J\x1b[HEspañol 日本語 🦀\r\n\x1b[38;2;17;101;221mBLUE".as_bytes());
        }
        t.scroll(-40);
        t.start_selection(
            TerminalSelectionType::Simple,
            TerminalPoint { row: 0, column: 0 },
            TerminalCellSide::Left,
        );
        t.update_selection(TerminalPoint { row: 0, column: 4 }, TerminalCellSide::Right);
        let before = t.snapshot();
        let selection = t.selection_text();
        let frame = t.remote_frame().unwrap();
        assert!(frame.lines[0].contains("Español 日本語 🦀"));
        assert!(!frame.lines.iter().any(|l| l.contains("history-")));
        assert_eq!(t.snapshot(), before);
        assert_eq!(t.selection_text(), selection);
        let mut replay = engine();
        replay.feed(super::super::remote::draw(&frame, None).as_bytes());
        let rendered = replay.snapshot().unwrap();
        assert_eq!(rendered.lines[1][0].text(), "B");
        assert_eq!(
            rendered.lines[1][0].foreground,
            TerminalRgb::new(17, 101, 221)
        );
        assert!(replay.text(false).unwrap().contains("Español 日本語 🦀"));
        t.scroll(-i32::MAX);
        let clean = t.snapshot();
        t.engine.lock().unwrap().feed(b"\x1b[1;1HZ");
        let changed = t.remote_frame().unwrap();
        assert!(changed.lines[0].contains('Z'));
        if let Some(path) = std::env::var_os("VIBRA_SCREEN_FIXTURE") {
            let data = serde_json::json!({"full":super::super::remote::draw(&frame,None),"patch":super::super::remote::draw(&changed,Some(&frame))});
            std::fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
        }
        let painted = t.snapshot();
        assert!(clean != painted);
        assert_eq!(painted.lines[0][0].text(), "Z");
        t.engine.lock().unwrap().feed(b"\x1b[?1049h\x1b[HALTERNATE");
        assert!(t.remote_frame().unwrap().lines[0].contains("ALTERNATE"));
        t.shutdown();
    }
    #[test]
    fn remote_ownership_restores_latest_local_size_and_blocks_local_input() {
        let t = GhosttyTerminal::spawn(
            Uuid::new_v4(),
            Path::new("/tmp"),
            &HashMap::new(),
            Some(("/bin/sleep", &["60"])),
        )
        .unwrap();
        let wait_size = |cols| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while t.remote_size().columns != cols {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(5));
            }
        };
        t.resize(TerminalSize {
            columns: 100,
            ..TerminalSize::default()
        })
        .unwrap();
        wait_size(100);
        t.remote_claim(TerminalSize {
            columns: 40,
            rows: 20,
            ..TerminalSize::default()
        })
        .unwrap();
        wait_size(40);
        assert!(t.remote_controlled());
        assert!(t.send_input(b"local".to_vec()).is_err());
        t.resize(TerminalSize {
            columns: 120,
            ..TerminalSize::default()
        })
        .unwrap();
        assert_eq!(t.remote_size().columns, 40);
        t.remote_input(b"remote".to_vec()).unwrap();
        t.remote_release();
        wait_size(120);
        assert!(!t.remote_controlled());
        assert!(t.remote_resize(TerminalSize::default()).is_err());
        assert!(t.remote_input(b"stale".to_vec()).is_err());
        t.shutdown();
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
    fn clipboard_reads_are_owned_and_wait_for_consent() {
        use base64::Engine as _;
        let (tx, rx) = async_channel::unbounded();
        let mut e = Engine::new(TerminalSize::default(), tx).unwrap();
        for byte in b"\x1b]52;c;?\x07\x1b]52;p;?\x1b\\" {
            e.feed(&[*byte]);
        }
        assert!(
            e.callbacks.replies.is_empty(),
            "must not answer before consent"
        );
        let TerminalEvent::ClipboardLoad(first) = rx.try_recv().unwrap() else {
            panic!("missing consent event")
        };
        let TerminalEvent::ClipboardLoad(second) = rx.try_recv().unwrap() else {
            panic!("missing consent event")
        };
        // Call after further parser mutations and terminal destruction: callbacks
        // must contain owned protocol strings, never a borrowed Ghostty request.
        e.feed(b"still responsive");
        drop(e);
        let secret = "Español\n\x1b]52;c;injection";
        let encoded = base64::engine::general_purpose::STANDARD.encode(secret);
        assert_eq!(first(secret), format!("\x1b]52;c;{encoded}\x07"));
        assert_eq!(second("hello"), "\x1b]52;p;aGVsbG8=\x1b\\");
        assert!(clipboard_template(b"\x1b]52;c;payload\x07").is_none());
        assert!(clipboard_template(b"\x1b]52;x;\x07").is_none());
    }

    #[test]
    fn literal_search_preserves_case_whitespace_and_wraps() {
        let mut e = engine();
        e.feed(b"Hello hello HELLO  src/(main|lib).rs\r\nhello");
        for needle in ["hello", "Hello", "HELLO", "src/(main|lib).rs", "  "] {
            assert_eq!(
                unsafe { vg_search(e.p(), needle.as_ptr(), needle.len(), 0) },
                1,
                "{needle}"
            );
            // Selection formatter trims whitespace for copying; compare that policy.
            assert_eq!(e.text(true).unwrap().trim_end(), needle.trim_end());
        }
        assert_eq!(unsafe { vg_search(e.p(), b"hElLo".as_ptr(), 5, 0) }, 0);
        for direction in [0, 0, 0, 1, 1] {
            assert_eq!(
                unsafe { vg_search(e.p(), b"hello".as_ptr(), 5, direction) },
                1
            );
            assert_eq!(e.text(true).as_deref(), Some("hello"));
        }
    }

    #[test]
    fn incremental_snapshot_keeps_clean_rows_and_clears_dirty_cells() {
        let mut e = engine();
        e.feed(b"row zero\r\nrow one\r\nrow two");
        let before = e.snapshot().unwrap();
        e.feed(b"\x1b[2;1H\x1b[2Kchanged");
        let _ = e.mode(); // Querying modes must not consume pending cell damage.
        let after = e.snapshot().unwrap();
        assert!(Arc::ptr_eq(&before.lines[0], &after.lines[0]));
        assert!(Arc::ptr_eq(&before.lines[2], &after.lines[2]));
        assert!(!Arc::ptr_eq(&before.lines[1], &after.lines[1]));
        assert!(
            e.last_painted_cells <= 160,
            "clean rows must not cross FFI: {}",
            e.last_painted_cells
        );
        assert_eq!(after.lines[1][0].text(), "c");
        assert_eq!(after.lines[1][7].text(), " ");
        e.feed(b"\x1b[2J");
        assert!(
            e.snapshot()
                .unwrap()
                .lines
                .iter()
                .flat_map(|r| r.iter())
                .all(|c| c.text() == " ")
        );
    }

    #[test]
    fn real_pty_sustained_output_and_recent_text_ignore_viewport() {
        let t = GhosttyTerminal::spawn(Uuid::new_v4(), Path::new("/tmp"), &HashMap::new(),
            Some(("/bin/sh", &["-c", "i=0; while [ $i -lt 12000 ]; do printf 'line-%s abcdefghijklmnopqrstuvwxyz0123456789\\n' $i; i=$((i+1)); done; printf 'FINAL-TAIL'; read answer"]))).unwrap();
        wait_for(&t, "FINAL-TAIL");
        let before = t.snapshot();
        assert!(before.history_size > 0);
        t.scroll(50);
        let scrolled = t.snapshot();
        assert!(scrolled.display_offset > 0);
        assert_ne!(before.lines[0], scrolled.lines[0]);
        let recent = t.recent_text(3).unwrap();
        assert!(recent.contains("FINAL-TAIL"));
        assert!(!recent.contains("line-0 "));
        t.shutdown();
    }

    #[test]
    fn real_vim_enters_and_leaves_alternate_screen() {
        let t = GhosttyTerminal::spawn(
            Uuid::new_v4(),
            Path::new("/tmp"),
            &HashMap::new(),
            Some(("/usr/bin/vim", &["-Nu", "NONE", "-n", "-i", "NONE"])),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !t.input_mode().alternate_screen {
            assert!(
                Instant::now() < deadline,
                "vim did not enter alternate screen"
            );
            thread::sleep(Duration::from_millis(5));
        }
        t.send_input(b":q!\r".to_vec()).unwrap();
        while t.alive.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "vim did not exit");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!t.input_mode().alternate_screen);
    }

    #[test]
    #[ignore = "manual performance measurement, not a timing-sensitive CI assertion"]
    fn profile_sessions() {
        fn peak_rss() -> i64 {
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
            assert_eq!(
                unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
                0
            );
            unsafe { usage.assume_init().ru_maxrss }
        }
        let baseline = peak_rss();
        let mut engines = Vec::new();
        for _ in 0..8 {
            let mut e = engine();
            e.resize(TerminalSize {
                columns: 120,
                rows: 40,
                ..TerminalSize::default()
            })
            .unwrap();
            e.feed(
                "build: abcdefghijklmnopqrstuvwxyz 0123456789\r\n"
                    .repeat(4000)
                    .as_bytes(),
            );
            e.snapshot().unwrap();
            engines.push(e);
        }
        let mut elapsed = Vec::new();
        let mut cells = 0u64;
        for i in 0..800 {
            let e = &mut engines[i % 8];
            let start = Instant::now();
            e.feed(format!("\x1b[2;1Htick {i:08}").as_bytes());
            e.snapshot().unwrap();
            elapsed.push(start.elapsed().as_secs_f64() * 1000.);
            cells += u64::from(e.last_painted_cells);
        }
        elapsed.sort_by(f64::total_cmp);
        println!(
            "PROFILE 8 sessions: update median_ms={:.3} p95_ms={:.3}; cells={cells}/{} full-screen baseline; peak_rss_before={} after={} bytes",
            elapsed[400],
            elapsed[760],
            800 * 120 * 40,
            baseline,
            peak_rss()
        );
        let t = GhosttyTerminal::spawn(
            Uuid::new_v4(),
            Path::new("/tmp"),
            &HashMap::new(),
            Some((
                "/bin/sh",
                &[
                    "-c",
                    "printf 'READY\\n'; while read value; do printf 'ACK-%s\\n' \"$value\"; done",
                ],
            )),
        )
        .unwrap();
        wait_for(&t, "READY");
        let mut latency = Vec::new();
        for i in 0..30 {
            let token = format!("ACK-{i:04}");
            let start = Instant::now();
            t.send_input(format!("{i:04}\n").into_bytes()).unwrap();
            loop {
                if t.recent_text(40).unwrap().contains(&token) {
                    break;
                }
                assert!(start.elapsed() < Duration::from_secs(5));
                thread::sleep(Duration::from_micros(100));
            }
            latency.push(start.elapsed().as_secs_f64() * 1000.);
        }
        latency.sort_by(f64::total_cmp);
        println!(
            "PROFILE PTY roundtrip (100us observation polling): median_ms={:.3} p95_ms={:.3}",
            latency[15], latency[28]
        );
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
