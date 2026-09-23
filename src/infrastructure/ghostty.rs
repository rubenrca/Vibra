//! libghostty-vt backend. The terminal state is protected by one
//! mutex; callbacks only enqueue data and never reenter it. The PTY worker owns
//! the child and reaps it, independently of the lifetime of the UI handle.
use super::terminal_support::{
    COLOR_SUPPRESSING_ENV, indexed_color_with, process_invoked_name, process_working_directory,
    terminal_child_environment,
};
use crate::ports::terminal_keyboard::TerminalKeyInput;
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

const MAX_TERMINAL_EVENTS: usize = 32;
const MAX_QUEUED_PTY_INPUTS: usize = 32;
// A 1 MiB clipboard response grows to ~1.34 MiB after OSC 52 base64.
const MAX_PTY_INPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CLIPBOARD_STORE_BYTES: usize = 1024 * 1024;
const MAX_PENDING_PTY_WRITE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TERMINAL_TITLE_BYTES: usize = 4096;

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
    fn vg_search_step(
        p: *mut c_void,
        s: *const u8,
        n: usize,
        previous: i32,
        continuation: i32,
    ) -> i32;
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
            if data.len() > MAX_TERMINAL_TITLE_BYTES {
                return;
            }
            let title = String::from_utf8_lossy(data).into_owned();
            let _ = context.events.try_send(if title.is_empty() {
                TerminalEvent::ResetTitle
            } else {
                TerminalEvent::Title(title)
            });
        }
        3 => {
            if data.len() > MAX_CLIPBOARD_STORE_BYTES {
                return;
            }
            let _ = context.events.try_send(TerminalEvent::ClipboardStore(
                String::from_utf8_lossy(data).into_owned(),
            ));
        }
        4 => {
            if let Some((prefix, suffix)) = clipboard_template(data) {
                let empty_response = format!("{prefix}{suffix}");
                if context
                    .events
                    .try_send(TerminalEvent::ClipboardLoad(Arc::new(move |text| {
                        use base64::Engine as _;
                        format!(
                            "{}{}{}",
                            prefix,
                            base64::engine::general_purpose::STANDARD.encode(text),
                            suffix
                        )
                    })))
                    .is_err()
                {
                    // The parser already denied the synchronous read. If the
                    // UI queue is full, still complete the OSC 52 query.
                    context.replies.extend(empty_response.bytes());
                }
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
    engine: Arc<Mutex<Engine>>,
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
        let (inputs, input_rx) = mpsc::sync_channel(MAX_QUEUED_PTY_INPUTS);
        let wakeup = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let shutdown_requested = Arc::new(AtomicBool::new(false));
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
                let child = worker_child.lock().unwrap().take().unwrap();
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
            if let Some(mut child) = child_slot.lock().unwrap().take() {
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
        self.inputs.try_send(input).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => {
                anyhow::anyhow!("la cola de entrada de la terminal está llena")
            }
            mpsc::TrySendError::Disconnected(_) => anyhow::anyhow!("PTY cerrado"),
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
    engine: &Arc<Mutex<Engine>>,
    writes: &mut VecDeque<(PtyInput, usize)>,
    queued_bytes: &mut usize,
    write_offset: &mut usize,
) {
    for _ in 0..16 {
        let Some((input, cost)) = writes.front_mut() else {
            break;
        };
        let encoded = if let PtyInput::Key(key) = input {
            // Encode after reading output, which may have changed keyboard mode.
            Some(key.bytes(engine.lock().unwrap().mode()))
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
}

fn pty_worker(
    mut master: File,
    mut child: Child,
    control: PtyWorkerControl,
    engine: Arc<Mutex<Engine>>,
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
            && let Some(size) = pending_resize.lock().unwrap().take()
        {
            let mut engine = engine.lock().unwrap();
            if engine.size != size {
                let ws = window_size(size);
                if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws) } == 0
                    && engine.resize(size).is_ok()
                {
                    refresh_needed = true;
                }
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
        enqueue_replies(&mut writes, &mut queued_bytes, &mut engine.lock().unwrap());
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
                    let mut e = engine.lock().unwrap();
                    e.feed(&output[..n]);
                    enqueue_replies(&mut writes, &mut queued_bytes, &mut e);
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
            refresh_needed = true;
        }
        if refresh_needed && wake(&events, &pending) {
            refresh_needed = false;
        }
        flush_pending_writes(
            &mut master,
            &engine,
            &mut writes,
            &mut queued_bytes,
            &mut write_offset,
        );
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
            bail!("la terminal ya terminó")
        }
        if input.len() > MAX_PTY_INPUT_BYTES {
            bail!("entrada de terminal demasiado grande")
        }
        self.enqueue_input(PtyInput::Bytes(input))
    }
    fn send_key_input(&self, input: TerminalKeyInput) -> Result<()> {
        if !self.alive.load(Ordering::Acquire) {
            bail!("la terminal ya terminó")
        }
        self.enqueue_input(PtyInput::Key(input))
    }
    fn resize(&self, size: TerminalSize) -> Result<()> {
        *self.pending_resize.lock().unwrap() = Some(size);
        let _ = (&self.signal).write(&[1]);
        Ok(())
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
    fn search_step(
        &self,
        query: &str,
        direction: TerminalSearchDirection,
        continuation: bool,
    ) -> Result<Option<bool>> {
        let mut e = self.engine.lock().unwrap();
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
