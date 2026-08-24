use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{
    Config, LineDamageBounds, Term, TermDamage, TermMode, point_to_viewport, viewport_to_point,
};
use alacritty_terminal::tty::{self, Options};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use anyhow::{Context as _, Result};
use async_channel::{Receiver, Sender};
use uuid::Uuid;

use crate::ports::terminal::{
    TerminalCell, TerminalCellSide, TerminalCursor, TerminalCursorShape, TerminalEvent,
    TerminalHandle, TerminalInputMode, TerminalPoint, TerminalPort, TerminalRgb,
    TerminalSearchDirection, TerminalSelectionType, TerminalSize, TerminalSnapshot,
    TerminalUnderline,
};

const FOREGROUND: TerminalRgb = TerminalRgb::new(0xe5, 0xe5, 0xe6);
const BACKGROUND: TerminalRgb = TerminalRgb::new(0x10, 0x10, 0x11);
const CURSOR: TerminalRgb = TerminalRgb::new(0xe8, 0xe8, 0xe8);

/// Host tools (CI, agent shells, cargo wrappers) often export these to force
/// monochrome output. Interactive agent CLIs inside Vibra panes should not
/// inherit that — dock-launched and agent-launched builds must look the same.
const COLOR_SUPPRESSING_ENV: &[&str] = &[
    "NO_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "FORCE_COLOR",
    "CARGO_TERM_COLOR",
    "NPM_CONFIG_COLOR",
    "PIP_NO_COLOR",
    "PY_COLORS",
    "NOCOLOR",
];

const ANSI: [TerminalRgb; 16] = [
    TerminalRgb::new(0x23, 0x23, 0x26),
    TerminalRgb::new(0xd9, 0x6c, 0x75),
    TerminalRgb::new(0x78, 0xb8, 0x8b),
    TerminalRgb::new(0xcf, 0xac, 0x68),
    TerminalRgb::new(0x72, 0x9c, 0xbe),
    TerminalRgb::new(0xa8, 0x82, 0xb3),
    TerminalRgb::new(0x72, 0xab, 0xa7),
    TerminalRgb::new(0xb7, 0xb7, 0xbb),
    TerminalRgb::new(0x64, 0x64, 0x69),
    TerminalRgb::new(0xe4, 0x7d, 0x85),
    TerminalRgb::new(0x89, 0xc6, 0x9b),
    TerminalRgb::new(0xdb, 0xba, 0x78),
    TerminalRgb::new(0x83, 0xaa, 0xc8),
    TerminalRgb::new(0xb8, 0x91, 0xc1),
    TerminalRgb::new(0x84, 0xba, 0xb5),
    TerminalRgb::new(0xe5, 0xe5, 0xe6),
];

#[derive(Default)]
pub struct AlacrittyTerminalPort;

impl TerminalPort for AlacrittyTerminalPort {
    fn backend_name(&self) -> &'static str {
        "terminal local"
    }

    fn spawn(
        &self,
        session_id: Uuid,
        working_directory: &Path,
        environment: &HashMap<String, String>,
    ) -> Result<Arc<dyn TerminalHandle>> {
        AlacrittyTerminal::spawn(session_id, working_directory, environment)
            .map(|terminal| terminal as Arc<dyn TerminalHandle>)
    }
}

#[derive(Clone)]
struct Listener {
    events: Sender<TerminalEvent>,
    event_loop: Arc<Mutex<Option<EventLoopSender>>>,
    size: Arc<Mutex<TerminalSize>>,
    wakeup_pending: Arc<AtomicBool>,
    snapshot_dirty: Arc<AtomicBool>,
}

impl Listener {
    fn send_to_pty(&self, text: String) {
        if let Ok(sender) = self.event_loop.lock()
            && let Some(sender) = sender.as_ref()
        {
            let _ = sender.send(Msg::Input(Cow::Owned(text.into_bytes())));
        }
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {
                self.snapshot_dirty.store(true, Ordering::Release);
                if !self.wakeup_pending.swap(true, Ordering::AcqRel) {
                    let _ = self.events.try_send(TerminalEvent::Wakeup);
                }
            }
            Event::Title(title) => {
                let _ = self.events.try_send(TerminalEvent::Title(title));
            }
            Event::ResetTitle => {
                let _ = self.events.try_send(TerminalEvent::ResetTitle);
            }
            Event::ClipboardStore(_, text) => {
                let _ = self.events.try_send(TerminalEvent::ClipboardStore(text));
            }
            Event::ClipboardLoad(_, formatter) => {
                let _ = self
                    .events
                    .try_send(TerminalEvent::ClipboardLoad(formatter));
            }
            Event::ColorRequest(index, formatter) => {
                self.send_to_pty(formatter(to_alacritty_rgb(indexed_color(index))));
            }
            Event::PtyWrite(text) => self.send_to_pty(text),
            Event::TextAreaSizeRequest(formatter) => {
                if let Ok(size) = self.size.lock() {
                    self.send_to_pty(formatter(window_size(*size)));
                }
            }
            Event::Bell => {
                let _ = self.events.try_send(TerminalEvent::Bell);
            }
            Event::Exit => {
                let _ = self.events.try_send(TerminalEvent::Exit(None));
            }
            Event::ChildExit(status) => {
                let _ = self.events.try_send(TerminalEvent::Exit(status.code()));
            }
        }
    }
}

struct TerminalDimensions {
    columns: usize,
    rows: usize,
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

struct AlacrittyTerminal {
    process_id: u32,
    #[cfg(unix)]
    pty_probe_fd: Option<OwnedFd>,
    terminal: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    events: Receiver<TerminalEvent>,
    size: Arc<Mutex<TerminalSize>>,
    wakeup_pending: Arc<AtomicBool>,
    snapshot_dirty: Arc<AtomicBool>,
    snapshot_fully_dirty: AtomicBool,
    snapshot_cache: Mutex<Option<Arc<TerminalSnapshot>>>,
    closed: AtomicBool,
}

impl AlacrittyTerminal {
    fn spawn(
        session_id: Uuid,
        working_directory: &Path,
        environment: &HashMap<String, String>,
    ) -> Result<Arc<Self>> {
        Self::spawn_with_shell(session_id, working_directory, None, environment)
    }

    fn spawn_with_shell(
        session_id: Uuid,
        working_directory: &Path,
        shell: Option<tty::Shell>,
        extra_environment: &HashMap<String, String>,
    ) -> Result<Arc<Self>> {
        let size = TerminalSize::default();
        let (events_tx, events_rx) = async_channel::unbounded();
        let event_loop_sender = Arc::new(Mutex::new(None));
        let shared_size = Arc::new(Mutex::new(size));
        let wakeup_pending = Arc::new(AtomicBool::new(false));
        let snapshot_dirty = Arc::new(AtomicBool::new(true));
        let listener = Listener {
            events: events_tx,
            event_loop: event_loop_sender.clone(),
            size: shared_size.clone(),
            wakeup_pending: wakeup_pending.clone(),
            snapshot_dirty: snapshot_dirty.clone(),
        };
        let dimensions = TerminalDimensions {
            columns: usize::from(size.columns),
            rows: usize::from(size.rows),
        };
        let terminal = Arc::new(FairMutex::new(Term::new(
            Config {
                kitty_keyboard: true,
                ..Config::default()
            },
            &dimensions,
            listener.clone(),
        )));

        let env = terminal_child_environment(session_id, extra_environment);
        let options = Options {
            shell,
            working_directory: Some(working_directory.to_path_buf()),
            drain_on_exit: true,
            env,
        };
        let window_id = u64::from_le_bytes(session_id.as_bytes()[..8].try_into().unwrap());
        // Alacritty merges Options.env into the inherited process environment and
        // cannot remove keys. Strip color-suppression vars around spawn so panes
        // match a dock-launched release even when Vibra itself was started from a
        // monochrome host (agent shells, CI wrappers, etc.).
        let pty = with_cleared_color_suppression(|| {
            tty::new(&options, window_size(size), window_id).with_context(|| {
                format!(
                    "no se pudo iniciar el PTY en {}",
                    working_directory.display()
                )
            })
        })?;
        #[cfg(unix)]
        let pty_probe_fd = {
            let file_descriptor = unsafe { libc::dup(pty.file().as_raw_fd()) };
            (file_descriptor >= 0).then(|| unsafe { OwnedFd::from_raw_fd(file_descriptor) })
        };
        let process_id = pty.child().id();
        let event_loop = EventLoop::new(terminal.clone(), listener, pty, true, false)
            .context("no se pudo crear el event loop del PTY")?;
        let sender = event_loop.channel();
        *event_loop_sender
            .lock()
            .expect("event loop sender poisoned") = Some(sender.clone());
        event_loop.spawn();

        Ok(Arc::new(Self {
            process_id,
            #[cfg(unix)]
            pty_probe_fd,
            terminal,
            sender,
            events: events_rx,
            size: shared_size,
            wakeup_pending,
            snapshot_dirty,
            snapshot_fully_dirty: AtomicBool::new(true),
            snapshot_cache: Mutex::new(None),
            closed: AtomicBool::new(false),
        }))
    }

    fn invalidate_snapshot(&self, fully: bool) {
        self.snapshot_dirty.store(true, Ordering::Release);
        if fully {
            self.snapshot_fully_dirty.store(true, Ordering::Release);
        }
    }

    #[cfg(unix)]
    fn foreground_process_group_id(&self) -> Option<u32> {
        let file_descriptor = self.pty_probe_fd.as_ref()?.as_raw_fd();
        let process_group = unsafe { libc::tcgetpgrp(file_descriptor) };
        (process_group > 0).then_some(process_group as u32)
    }

    #[cfg(not(unix))]
    fn foreground_process_group_id(&self) -> Option<u32> {
        None
    }
}

impl TerminalHandle for AlacrittyTerminal {
    fn events(&self) -> Receiver<TerminalEvent> {
        self.events.clone()
    }

    fn send_input(&self, input: Vec<u8>) -> Result<()> {
        if input.is_empty() || self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        self.sender
            .send(Msg::Input(Cow::Owned(input)))
            .context("no se pudo escribir al PTY")
    }

    fn resize(&self, size: TerminalSize) -> Result<()> {
        if size.columns == 0 || size.rows == 0 {
            return Ok(());
        }
        let mut current = self.size.lock().expect("terminal size poisoned");
        if current.columns == size.columns
            && current.rows == size.rows
            && current.cell_width == size.cell_width
            && current.cell_height == size.cell_height
        {
            return Ok(());
        }
        *current = size;
        drop(current);
        self.terminal.lock().resize(TerminalDimensions {
            columns: usize::from(size.columns),
            rows: usize::from(size.rows),
        });
        self.invalidate_snapshot(true);
        self.sender
            .send(Msg::Resize(window_size(size)))
            .context("no se pudo redimensionar el PTY")
    }

    fn scroll(&self, lines: i32) {
        if lines != 0 {
            let mut terminal = self.terminal.lock();
            let previous_offset = terminal.grid().display_offset();
            terminal.scroll_display(Scroll::Delta(lines));
            let changed = previous_offset != terminal.grid().display_offset();
            drop(terminal);
            if changed {
                self.invalidate_snapshot(true);
            }
        }
    }

    fn clear_scrollback(&self) {
        let mut terminal = self.terminal.lock();
        terminal.grid_mut().clear_history();
        drop(terminal);
        self.invalidate_snapshot(true);
    }

    fn snapshot(&self) -> Arc<TerminalSnapshot> {
        if !self.snapshot_dirty.swap(false, Ordering::AcqRel)
            && let Some(snapshot) = self
                .snapshot_cache
                .lock()
                .expect("terminal snapshot cache poisoned")
                .as_ref()
        {
            return snapshot.clone();
        }

        let mut terminal = self.terminal.lock();
        let damage = match terminal.damage() {
            TermDamage::Full => None,
            TermDamage::Partial(lines) => Some(lines.collect::<Vec<_>>()),
        };
        let fully_dirty = self.snapshot_fully_dirty.swap(false, Ordering::AcqRel);
        let previous = self
            .snapshot_cache
            .lock()
            .expect("terminal snapshot cache poisoned")
            .clone();
        let requires_full_snapshot = fully_dirty
            || damage.is_none()
            || previous.as_ref().is_none_or(|snapshot| {
                snapshot.columns != terminal.grid().columns()
                    || snapshot.rows != terminal.grid().screen_lines()
                    || snapshot.display_offset != terminal.grid().display_offset()
            });
        let snapshot = if requires_full_snapshot {
            terminal_snapshot(&terminal)
        } else {
            terminal_snapshot_with_damage(
                &terminal,
                previous.as_deref().expect("snapshot cache checked above"),
                damage.as_deref().unwrap_or_default(),
            )
        };
        terminal.reset_damage();
        let snapshot = Arc::new(snapshot);
        *self
            .snapshot_cache
            .lock()
            .expect("terminal snapshot cache poisoned") = Some(snapshot.clone());
        snapshot
    }

    fn input_mode(&self) -> TerminalInputMode {
        let terminal = self.terminal.lock();
        let mode = *terminal.mode();
        TerminalInputMode {
            application_cursor: mode.contains(TermMode::APP_CURSOR),
            application_keypad: mode.contains(TermMode::APP_KEYPAD),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            alternate_screen: mode.contains(TermMode::ALT_SCREEN),
            alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
            focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
            mouse_report_click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
            mouse_drag: mode.contains(TermMode::MOUSE_DRAG),
            mouse_motion: mode.contains(TermMode::MOUSE_MOTION),
            sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
            utf8_mouse: mode.contains(TermMode::UTF8_MOUSE),
            disambiguate_escape_codes: mode.contains(TermMode::DISAMBIGUATE_ESC_CODES),
            report_event_types: mode.contains(TermMode::REPORT_EVENT_TYPES),
            report_alternate_keys: mode.contains(TermMode::REPORT_ALTERNATE_KEYS),
            report_all_keys_as_escape_codes: mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
            report_associated_text: mode.contains(TermMode::REPORT_ASSOCIATED_TEXT),
        }
    }

    fn current_working_directory(&self) -> Option<PathBuf> {
        // Prefer the session shell: interactive `cd` updates the shell, not a
        // short-lived foreground tool. Fall back to the foreground process when
        // the shell path is unavailable (e.g. mid-reap).
        process_working_directory(self.process_id).or_else(|| {
            self.foreground_process_group_id()
                .filter(|pid| *pid != self.process_id)
                .and_then(process_working_directory)
        })
    }

    fn foreground_process_name(&self) -> Option<String> {
        self.foreground_process_group_id()
            .and_then(process_invoked_name)
            .or_else(|| process_invoked_name(self.process_id))
    }

    fn foreground_process_id(&self) -> Option<u32> {
        self.foreground_process_group_id()
    }

    fn session_process_id(&self) -> Option<u32> {
        Some(self.process_id)
    }

    fn recent_text(&self, lines: usize) -> Option<String> {
        let terminal = self.terminal.lock();
        let grid = terminal.grid();
        let take = lines.max(1).min(grid.total_lines().max(1));
        let bottom = grid.bottommost_line().0;
        let top = grid.topmost_line().0;
        let start = (bottom - take as i32 + 1).max(top);
        let mut text = String::new();
        for line_index in start..=bottom {
            let row = &grid[Line(line_index)];
            let mut line = String::new();
            for cell in row {
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                line.push(cell.c);
                if let Some(zerowidth) = cell.zerowidth() {
                    line.extend(zerowidth);
                }
            }
            while line.ends_with(' ') {
                line.pop();
            }
            text.push_str(&line);
            text.push('\n');
        }
        Some(text)
    }

    fn clear_selection(&self) {
        let changed = self.terminal.lock().selection.take().is_some();
        if changed {
            self.invalidate_snapshot(true);
        }
    }

    fn start_selection(
        &self,
        selection_type: TerminalSelectionType,
        point: TerminalPoint,
        side: TerminalCellSide,
    ) {
        let mut terminal = self.terminal.lock();
        let point = viewport_point_to_terminal(&terminal, point);
        terminal.selection = Some(Selection::new(
            selection_type_to_alacritty(selection_type),
            point,
            side_to_alacritty(side),
        ));
        drop(terminal);
        self.invalidate_snapshot(true);
    }

    fn update_selection(&self, point: TerminalPoint, side: TerminalCellSide) {
        let mut terminal = self.terminal.lock();
        let point = viewport_point_to_terminal(&terminal, point);
        if let Some(selection) = terminal.selection.as_mut() {
            selection.update(point, side_to_alacritty(side));
        }
        drop(terminal);
        self.invalidate_snapshot(true);
    }

    fn selection_text(&self) -> Option<String> {
        self.terminal.lock().selection_to_string()
    }

    fn search(&self, query: &str, direction: TerminalSearchDirection) -> Result<bool> {
        let mut terminal = self.terminal.lock();
        if query.is_empty() {
            let changed = terminal.selection.take().is_some();
            drop(terminal);
            if changed {
                self.invalidate_snapshot(true);
            }
            return Ok(false);
        }

        let mut regex = RegexSearch::new(&regex_escape(query))
            .map_err(|error| anyhow::anyhow!("no se pudo preparar la busqueda: {error}"))?;
        let selection_range = terminal
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&terminal));
        let (origin, direction, side) = match direction {
            TerminalSearchDirection::Next => {
                let origin = selection_range
                    .map(|range| range.end.add(&*terminal, Boundary::Grid, 1))
                    .unwrap_or(terminal.grid().cursor.point);
                (origin, Direction::Right, Side::Left)
            }
            TerminalSearchDirection::Previous => {
                let origin = selection_range
                    .map(|range| range.start.sub(&*terminal, Boundary::Grid, 1))
                    .unwrap_or(terminal.grid().cursor.point);
                (origin, Direction::Left, Side::Right)
            }
        };

        let Some(regex_match) = terminal.search_next(&mut regex, origin, direction, side, None)
        else {
            return Ok(false);
        };
        let (start, end) = (*regex_match.start(), *regex_match.end());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        terminal.selection = Some(selection);
        terminal.scroll_to_point(start);
        drop(terminal);
        self.invalidate_snapshot(true);
        Ok(true)
    }

    fn hyperlink_at(&self, point: TerminalPoint) -> Option<String> {
        let terminal = self.terminal.lock();
        let point = viewport_point_to_terminal(&terminal, point);
        terminal.grid()[point]
            .hyperlink()
            .map(|hyperlink| hyperlink.uri().to_owned())
            .or_else(|| plain_hyperlink_at(&terminal, point))
    }

    fn acknowledge_wakeup(&self) {
        self.wakeup_pending.store(false, Ordering::Release);
    }

    fn shutdown(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        #[cfg(unix)]
        unsafe {
            // Alacritty creates a new session, so the shell PID is also its process group.
            libc::kill(-(self.process_id as i32), libc::SIGHUP);
        }
        let _ = self.sender.send(Msg::Shutdown);
    }
}

#[cfg(target_os = "macos")]
fn process_working_directory(process_id: u32) -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::mem::{MaybeUninit, size_of};

    let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let info_size = size_of::<libc::proc_vnodepathinfo>();
    let bytes = unsafe {
        libc::proc_pidinfo(
            process_id as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            info_size as libc::c_int,
        )
    };
    if bytes != info_size as libc::c_int {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast::<libc::c_char>()) };
    let path = PathBuf::from(path.to_string_lossy().into_owned());
    path.is_dir().then_some(path)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_working_directory(process_id: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{process_id}/cwd")).ok()
}

#[cfg(not(unix))]
fn process_working_directory(_: u32) -> Option<PathBuf> {
    None
}

/// Name used to invoke a process. This preserves wrapper identities such as
/// Cursor's `cursor-agent`, whose script replaces itself with a `node` binary
/// while retaining the original argv[0].
fn process_invoked_name(process_id: u32) -> Option<String> {
    process_argv0(process_id)
        .or_else(|| process_executable_name(process_id))
        .and_then(|name| {
            Path::new(&name)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
}

#[cfg(target_os = "macos")]
fn process_argv0(process_id: u32) -> Option<String> {
    use std::mem::size_of;

    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        process_id as libc::c_int,
    ];
    let mut size = 0usize;
    let size_result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if size_result != 0 || size <= size_of::<libc::c_int>() || size > 1024 * 1024 {
        return None;
    }

    let mut buffer = vec![0u8; size];
    let read_result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read_result != 0 {
        return None;
    }
    buffer.truncate(size);
    argv0_from_macos_procargs(&buffer)
}

#[cfg(target_os = "macos")]
fn argv0_from_macos_procargs(buffer: &[u8]) -> Option<String> {
    use std::mem::size_of;

    let mut offset = size_of::<libc::c_int>();
    offset += buffer.get(offset..)?.iter().position(|byte| *byte == 0)?;
    while buffer.get(offset) == Some(&0) {
        offset += 1;
    }
    let end = offset + buffer.get(offset..)?.iter().position(|byte| *byte == 0)?;
    (!buffer[offset..end].is_empty())
        .then(|| String::from_utf8_lossy(&buffer[offset..end]).into_owned())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_argv0(process_id: u32) -> Option<String> {
    let command_line = std::fs::read(format!("/proc/{process_id}/cmdline")).ok()?;
    let end = command_line.iter().position(|byte| *byte == 0)?;
    (!command_line[..end].is_empty())
        .then(|| String::from_utf8_lossy(&command_line[..end]).into_owned())
}

#[cfg(not(unix))]
fn process_argv0(_: u32) -> Option<String> {
    None
}

/// Executable basename of a process (e.g. `codex`, `claude`, `grok`).
#[cfg(target_os = "macos")]
fn process_executable_name(process_id: u32) -> Option<String> {
    use std::ffi::CStr;

    let mut buffer = [0i8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let bytes = unsafe {
        libc::proc_pidpath(
            process_id as libc::c_int,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        )
    };
    if bytes <= 0 {
        return None;
    }
    let path = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_executable_name(process_id: u32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{process_id}/exe")).ok()?;
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

#[cfg(not(unix))]
fn process_executable_name(_: u32) -> Option<String> {
    None
}

impl Drop for AlacrittyTerminal {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn terminal_snapshot<T: EventListener>(terminal: &Term<T>) -> TerminalSnapshot {
    let columns = terminal.grid().columns();
    let rows = terminal.grid().screen_lines();
    let history_size = terminal.grid().history_size();
    let cursor_blinking = terminal.cursor_style().blinking;
    let renderable = terminal.renderable_content();
    let display_offset = renderable.display_offset;
    let colors = renderable.colors;
    let selection = renderable.selection;
    let cursor_point = renderable.cursor.point;
    let cursor_shape = renderable.cursor.shape;
    let mut lines = blank_lines(rows, columns);

    for indexed in renderable.display_iter {
        let selected = selection
            .is_some_and(|selection| selection.contains_cell(&indexed, cursor_point, cursor_shape));
        let Some(point) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let cell = indexed.cell;
        let mut foreground = cell.fg;
        let mut background = cell.bg;
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut foreground, &mut background);
        }
        let mut text = String::from(cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            text.extend(zerowidth);
        }
        let foreground = resolve_color(foreground, cell.flags, colors, true);
        let underline_color = cell
            .underline_color()
            .map(|color| resolve_color(color, cell.flags, colors, true))
            .unwrap_or(foreground);
        let mut painted = TerminalCell::with_text(
            point.line,
            point.column.0,
            &text,
            foreground,
            resolve_color(background, cell.flags, colors, false),
        );
        painted.underline_color = underline_color;
        painted.bold = cell.flags.contains(Flags::BOLD);
        painted.italic = cell.flags.contains(Flags::ITALIC);
        painted.underline = underline_style(cell.flags);
        painted.strikeout = cell.flags.contains(Flags::STRIKEOUT);
        painted.hidden = cell.flags.contains(Flags::HIDDEN);
        painted.wide_spacer = cell.flags.contains(Flags::WIDE_CHAR_SPACER);
        painted.selected = selected;
        painted.set_hyperlink(cell.hyperlink().as_ref().map(|hyperlink| hyperlink.uri()));
        lines[point.line][point.column.0] = painted;
    }

    let cursor = point_to_viewport(display_offset, renderable.cursor.point)
        .filter(|point| point.line < rows && point.column.0 < columns)
        .map(|point| TerminalCursor {
            row: point.line,
            column: point.column.0,
            shape: match renderable.cursor.shape {
                CursorShape::Block => TerminalCursorShape::Block,
                CursorShape::Underline => TerminalCursorShape::Underline,
                CursorShape::Beam => TerminalCursorShape::Beam,
                CursorShape::HollowBlock => TerminalCursorShape::HollowBlock,
                CursorShape::Hidden => TerminalCursorShape::Hidden,
            },
            blinking: cursor_blinking,
        });

    TerminalSnapshot {
        columns,
        rows,
        lines: lines.into_iter().map(Arc::from).collect(),
        cursor,
        display_offset,
        history_size,
    }
}

fn terminal_snapshot_with_damage<T: EventListener>(
    terminal: &Term<T>,
    previous: &TerminalSnapshot,
    damage: &[LineDamageBounds],
) -> TerminalSnapshot {
    let columns = terminal.grid().columns();
    let rows = terminal.grid().screen_lines();
    let history_size = terminal.grid().history_size();
    let cursor_blinking = terminal.cursor_style().blinking;
    let renderable = terminal.renderable_content();
    let display_offset = renderable.display_offset;
    let colors = renderable.colors;
    let selection = renderable.selection;
    let cursor_point = renderable.cursor.point;
    let cursor_shape = renderable.cursor.shape;
    let mut dirty_rows = vec![false; rows];
    let mut lines = previous.lines.clone();

    for line in damage {
        if line.line < rows {
            dirty_rows[line.line] = true;
            lines[line.line] = Arc::from(
                (0..columns)
                    .map(|column| blank_cell(line.line, column))
                    .collect::<Vec<_>>(),
            );
        }
    }

    for indexed in renderable.display_iter {
        let Some(point) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        if point.line >= rows || !dirty_rows[point.line] {
            continue;
        }
        let selected = selection
            .is_some_and(|selection| selection.contains_cell(&indexed, cursor_point, cursor_shape));
        let cell = indexed.cell;
        let mut foreground = cell.fg;
        let mut background = cell.bg;
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut foreground, &mut background);
        }
        let mut text = String::from(cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            text.extend(zerowidth);
        }
        let foreground = resolve_color(foreground, cell.flags, colors, true);
        let underline_color = cell
            .underline_color()
            .map(|color| resolve_color(color, cell.flags, colors, true))
            .unwrap_or(foreground);
        let mut painted = TerminalCell::with_text(
            point.line,
            point.column.0,
            &text,
            foreground,
            resolve_color(background, cell.flags, colors, false),
        );
        painted.underline_color = underline_color;
        painted.bold = cell.flags.contains(Flags::BOLD);
        painted.italic = cell.flags.contains(Flags::ITALIC);
        painted.underline = underline_style(cell.flags);
        painted.strikeout = cell.flags.contains(Flags::STRIKEOUT);
        painted.hidden = cell.flags.contains(Flags::HIDDEN);
        painted.wide_spacer = cell.flags.contains(Flags::WIDE_CHAR_SPACER);
        painted.selected = selected;
        painted.set_hyperlink(cell.hyperlink().as_ref().map(|hyperlink| hyperlink.uri()));
        Arc::make_mut(&mut lines[point.line])[point.column.0] = painted;
    }

    let cursor = point_to_viewport(display_offset, renderable.cursor.point)
        .filter(|point| point.line < rows && point.column.0 < columns)
        .map(|point| TerminalCursor {
            row: point.line,
            column: point.column.0,
            shape: match renderable.cursor.shape {
                CursorShape::Block => TerminalCursorShape::Block,
                CursorShape::Underline => TerminalCursorShape::Underline,
                CursorShape::Beam => TerminalCursorShape::Beam,
                CursorShape::HollowBlock => TerminalCursorShape::HollowBlock,
                CursorShape::Hidden => TerminalCursorShape::Hidden,
            },
            blinking: cursor_blinking,
        });

    TerminalSnapshot {
        columns,
        rows,
        lines,
        cursor,
        display_offset,
        history_size,
    }
}

fn blank_lines(rows: usize, columns: usize) -> Vec<Vec<TerminalCell>> {
    (0..rows)
        .map(|row| (0..columns).map(|column| blank_cell(row, column)).collect())
        .collect()
}

fn window_size(size: TerminalSize) -> WindowSize {
    WindowSize {
        num_lines: size.rows,
        num_cols: size.columns,
        cell_width: size.cell_width.round().clamp(1.0, u16::MAX as f32) as u16,
        cell_height: size.cell_height.round().clamp(1.0, u16::MAX as f32) as u16,
    }
}

fn resolve_color(color: Color, flags: Flags, colors: &Colors, foreground: bool) -> TerminalRgb {
    let color = match color {
        Color::Named(named) if foreground && flags.contains(Flags::BOLD) => {
            Color::Named(named.to_bright())
        }
        Color::Named(named) if flags.contains(Flags::DIM) => Color::Named(named.to_dim()),
        color => color,
    };
    let resolved = match color {
        Color::Spec(rgb) => from_alacritty_rgb(rgb),
        Color::Indexed(index) => colors[usize::from(index)]
            .map(from_alacritty_rgb)
            .unwrap_or_else(|| indexed_color(usize::from(index))),
        Color::Named(named) => colors[named]
            .map(from_alacritty_rgb)
            .unwrap_or_else(|| named_color(named)),
    };
    if foreground && flags.contains(Flags::DIM) && !matches!(color, Color::Named(_)) {
        dim(resolved)
    } else {
        resolved
    }
}

fn named_color(color: NamedColor) -> TerminalRgb {
    let index = color as usize;
    match color {
        NamedColor::Foreground | NamedColor::BrightForeground => FOREGROUND,
        NamedColor::Background => BACKGROUND,
        NamedColor::Cursor => CURSOR,
        _ if index < ANSI.len() => ANSI[index],
        NamedColor::DimBlack => dim(ANSI[0]),
        NamedColor::DimRed => dim(ANSI[1]),
        NamedColor::DimGreen => dim(ANSI[2]),
        NamedColor::DimYellow => dim(ANSI[3]),
        NamedColor::DimBlue => dim(ANSI[4]),
        NamedColor::DimMagenta => dim(ANSI[5]),
        NamedColor::DimCyan => dim(ANSI[6]),
        NamedColor::DimWhite => dim(ANSI[7]),
        NamedColor::DimForeground => dim(FOREGROUND),
        _ => FOREGROUND,
    }
}

fn indexed_color(index: usize) -> TerminalRgb {
    match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let value = index - 16;
            let component = |part: usize| [0, 95, 135, 175, 215, 255][part];
            TerminalRgb::new(
                component(value / 36),
                component((value / 6) % 6),
                component(value % 6),
            )
        }
        232..=255 => {
            let gray = 8 + ((index - 232) * 10) as u8;
            TerminalRgb::new(gray, gray, gray)
        }
        256 => FOREGROUND,
        257 => BACKGROUND,
        258 => CURSOR,
        _ => FOREGROUND,
    }
}

fn dim(color: TerminalRgb) -> TerminalRgb {
    const DIM_NUMERATOR: u16 = 66;
    const DIM_DENOMINATOR: u16 = 100;
    let component =
        |component: u8| ((u16::from(component) * DIM_NUMERATOR) / DIM_DENOMINATOR) as u8;
    TerminalRgb::new(
        component(color.red),
        component(color.green),
        component(color.blue),
    )
}

fn blank_cell(row: usize, column: usize) -> TerminalCell {
    let mut cell = TerminalCell::blank(row, column);
    cell.foreground = FOREGROUND;
    cell.background = BACKGROUND;
    cell.underline_color = FOREGROUND;
    cell
}

fn underline_style(flags: Flags) -> TerminalUnderline {
    if flags.contains(Flags::UNDERCURL) {
        TerminalUnderline::Curly
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        TerminalUnderline::Double
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        TerminalUnderline::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        TerminalUnderline::Dashed
    } else if flags.contains(Flags::UNDERLINE) {
        TerminalUnderline::Single
    } else {
        TerminalUnderline::None
    }
}

fn viewport_point_to_terminal(terminal: &Term<Listener>, point: TerminalPoint) -> Point {
    let row = point.row.min(terminal.screen_lines().saturating_sub(1));
    let column = Column(point.column.min(terminal.columns().saturating_sub(1)));
    viewport_to_point(terminal.grid().display_offset(), Point::new(row, column))
}

fn selection_type_to_alacritty(selection_type: TerminalSelectionType) -> SelectionType {
    match selection_type {
        TerminalSelectionType::Simple => SelectionType::Simple,
        TerminalSelectionType::Block => SelectionType::Block,
        TerminalSelectionType::Semantic => SelectionType::Semantic,
        TerminalSelectionType::Lines => SelectionType::Lines,
    }
}

fn side_to_alacritty(side: TerminalCellSide) -> Side {
    match side {
        TerminalCellSide::Left => Side::Left,
        TerminalCellSide::Right => Side::Right,
    }
}

fn plain_hyperlink_at<T: EventListener>(terminal: &Term<T>, point: Point) -> Option<String> {
    let columns = terminal.columns();
    if point.column.0 >= columns {
        return None;
    }
    let row = &terminal.grid()[point.line];
    let mut start = point.column.0;
    while start > 0 && !row[Column(start - 1)].c.is_whitespace() {
        start -= 1;
    }
    let mut end = point.column.0;
    while end + 1 < columns && !row[Column(end + 1)].c.is_whitespace() {
        end += 1;
    }

    let raw = (start..=end)
        .map(|column| row[Column(column)].c)
        .collect::<String>();
    let leading = raw.len()
        - raw
            .trim_start_matches(['(', '[', '{', '<', '\'', '"'])
            .len();
    let candidate = raw
        .trim_start_matches(['(', '[', '{', '<', '\'', '"'])
        .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '\'', '"']);
    let candidate_end = leading + candidate.len();
    let cursor_byte = raw
        .char_indices()
        .nth(point.column.0 - start)
        .map(|(index, _)| index)
        .unwrap_or(raw.len());
    if cursor_byte < leading || cursor_byte >= candidate_end || !is_supported_uri(candidate) {
        return None;
    }
    Some(candidate.to_owned())
}

fn is_supported_uri(uri: &str) -> bool {
    ["https://", "http://", "mailto:", "file://"]
        .iter()
        .any(|scheme| uri.starts_with(scheme))
}

fn regex_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(
            character,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn from_alacritty_rgb(color: Rgb) -> TerminalRgb {
    TerminalRgb::new(color.r, color.g, color.b)
}

fn to_alacritty_rgb(color: TerminalRgb) -> Rgb {
    Rgb {
        r: color.red,
        g: color.green,
        b: color.blue,
    }
}

fn is_color_suppressing_env(key: &str) -> bool {
    COLOR_SUPPRESSING_ENV
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(key))
}

fn terminal_child_environment(
    session_id: Uuid,
    extra_environment: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("TERM_PROGRAM".into(), "Vibra".into());
    env.insert(
        "TERM_PROGRAM_VERSION".into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    env.insert("VIBRA_SESSION_ID".into(), session_id.to_string());
    for (key, value) in extra_environment {
        if is_color_suppressing_env(key) {
            continue;
        }
        env.insert(key.clone(), value.clone());
    }
    env
}

/// Remove monochrome-forcing variables for the duration of `f`, then restore them.
///
/// `std::process::Command` (used by alacritty's PTY spawn) inherits the current
/// process environment; Options.env can only set keys, not unset them.
fn with_cleared_color_suppression<T>(f: impl FnOnce() -> T) -> T {
    // Serialize so concurrent pane spawns don't restore vars mid-flight.
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().expect("color-suppression lock poisoned");

    let mut saved = Vec::new();
    for key in COLOR_SUPPRESSING_ENV {
        if let Ok(value) = std::env::var(key) {
            saved.push((*key, value));
            // SAFETY: guarded by LOCK; only used around PTY spawn.
            unsafe {
                std::env::remove_var(key);
            }
        }
    }

    let result = f();

    for (key, value) in saved {
        // SAFETY: same lock as removal; restore host environment as found.
        unsafe {
            std::env::set_var(key, value);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::Line;
    use std::time::{Duration, Instant};

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_procargs_preserves_a_wrapper_argv0() {
        let mut procargs = 3i32.to_ne_bytes().to_vec();
        procargs
            .extend_from_slice(b"/private/cursor/node\0\0/usr/local/bin/cursor-agent\0index.js\0");
        assert_eq!(
            argv0_from_macos_procargs(&procargs).as_deref(),
            Some("/usr/local/bin/cursor-agent")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn invoked_name_uses_argv0_before_the_runtime_executable() {
        use std::os::unix::process::CommandExt;

        let mut child = std::process::Command::new("/bin/sleep")
            .arg0("/usr/local/bin/cursor-agent")
            .arg("5")
            .spawn()
            .unwrap();
        assert_eq!(
            process_invoked_name(child.id()).as_deref(),
            Some("cursor-agent")
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn maps_the_entire_xterm_color_cube() {
        assert_eq!(indexed_color(16), TerminalRgb::new(0, 0, 0));
        assert_eq!(indexed_color(21), TerminalRgb::new(0, 0, 255));
        assert_eq!(indexed_color(231), TerminalRgb::new(255, 255, 255));
        assert_eq!(indexed_color(255), TerminalRgb::new(238, 238, 238));
    }

    #[test]
    fn child_environment_drops_color_suppression_keys() {
        let mut extra = HashMap::new();
        extra.insert("NO_COLOR".into(), "1".into());
        extra.insert("FORCE_COLOR".into(), "0".into());
        extra.insert("VIBRA_PANE_ID".into(), "pane".into());
        let env = terminal_child_environment(Uuid::nil(), &extra);
        assert!(!env.contains_key("NO_COLOR"));
        assert!(!env.contains_key("FORCE_COLOR"));
        assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));
        assert_eq!(env.get("VIBRA_PANE_ID").map(String::as_str), Some("pane"));
    }

    #[test]
    fn clearing_color_suppression_is_restored() {
        // SAFETY: test-only mutation of process env under the same helper lock path.
        unsafe {
            std::env::set_var("NO_COLOR", "1");
            std::env::set_var("FORCE_COLOR", "0");
        }
        let inside = with_cleared_color_suppression(|| {
            (
                std::env::var_os("NO_COLOR").is_none(),
                std::env::var_os("FORCE_COLOR").is_none(),
            )
        });
        assert_eq!(inside, (true, true));
        assert_eq!(std::env::var("NO_COLOR").ok().as_deref(), Some("1"));
        assert_eq!(std::env::var("FORCE_COLOR").ok().as_deref(), Some("0"));
        unsafe {
            std::env::remove_var("NO_COLOR");
            std::env::remove_var("FORCE_COLOR");
        }
    }

    #[test]
    fn window_size_never_reports_zero_sized_cells() {
        let size = window_size(TerminalSize {
            cell_width: 0.2,
            cell_height: 0.0,
            ..TerminalSize::default()
        });
        assert_eq!(size.cell_width, 1);
        assert_eq!(size.cell_height, 1);
    }

    #[test]
    fn literal_search_escapes_regex_metacharacters() {
        assert_eq!(regex_escape("src/(main|lib).rs"), r"src/\(main\|lib\)\.rs");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_the_current_process_working_directory() {
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        let actual = process_working_directory(std::process::id())
            .unwrap()
            .canonicalize()
            .unwrap();

        assert_eq!(actual, expected);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_login_shell_reports_directory_changes() {
        let start = std::env::temp_dir();
        let target = start.join(format!("vibra-cwd-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&target).unwrap();
        let terminal = AlacrittyTerminal::spawn(Uuid::new_v4(), &start, &HashMap::new()).unwrap();
        terminal
            .send_input(format!("cd -- '{}'\r", target.display()).into_bytes())
            .unwrap();

        let expected = target.canonicalize().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut actual = None;
        while Instant::now() < deadline {
            actual = terminal
                .current_working_directory()
                .and_then(|path| path.canonicalize().ok());
            if actual.as_ref() == Some(&expected) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        terminal.shutdown();
        std::fs::remove_dir_all(target).unwrap();

        assert_eq!(actual, Some(expected));
    }

    #[test]
    fn dimming_uses_alacrittys_sixty_six_percent_factor() {
        assert_eq!(
            dim(TerminalRgb::new(100, 200, 255)),
            TerminalRgb::new(66, 132, 168)
        );
    }

    #[test]
    fn snapshots_are_dense_and_preserve_selection_and_hyperlinks() {
        let dimensions = TerminalDimensions {
            columns: 4,
            rows: 3,
        };
        let mut terminal = Term::new(
            Config::default(),
            &dimensions,
            alacritty_terminal::event::VoidListener,
        );
        terminal.grid_mut()[Line(1)][Column(1)].c = 'A';
        terminal.grid_mut()[Line(1)][Column(1)].set_hyperlink(Some(
            alacritty_terminal::term::cell::Hyperlink::new(
                Some("test"),
                "https://example.com".to_owned(),
            ),
        ));
        let mut selection = Selection::new(
            SelectionType::Simple,
            Point::new(Line(1), Column(1)),
            Side::Left,
        );
        selection.update(Point::new(Line(1), Column(2)), Side::Right);
        terminal.selection = Some(selection);

        let snapshot = terminal_snapshot(&terminal);

        assert_eq!(snapshot.lines.len(), 3);
        assert!(snapshot.lines.iter().all(|line| line.len() == 4));
        assert_eq!(snapshot.lines[0][0].row, 0);
        assert_eq!(snapshot.lines[0][0].text(), " ");
        assert_eq!(snapshot.lines[1][1].text(), "A");
        assert_eq!(
            snapshot.lines[1][1].hyperlink.as_deref(),
            Some("https://example.com")
        );
        assert!(snapshot.lines[1][1].selected);
        assert!(snapshot.lines[1][2].selected);
        assert_eq!(snapshot.lines[2][3].row, 2);
        assert_eq!(snapshot.lines[2][3].column, 3);
    }

    #[test]
    fn damaged_snapshots_reuse_unchanged_rows() {
        let dimensions = TerminalDimensions {
            columns: 4,
            rows: 3,
        };
        let mut terminal = Term::new(
            Config::default(),
            &dimensions,
            alacritty_terminal::event::VoidListener,
        );
        terminal.grid_mut()[Line(0)][Column(0)].c = 'A';
        let previous = terminal_snapshot(&terminal);
        terminal.grid_mut()[Line(1)][Column(2)].c = 'B';

        let current =
            terminal_snapshot_with_damage(&terminal, &previous, &[LineDamageBounds::new(1, 2, 2)]);

        assert!(Arc::ptr_eq(&previous.lines[0], &current.lines[0]));
        assert!(!Arc::ptr_eq(&previous.lines[1], &current.lines[1]));
        assert!(Arc::ptr_eq(&previous.lines[2], &current.lines[2]));
        assert_eq!(current.lines[1][2].text(), "B");
    }

    #[test]
    fn detects_plain_links_and_trims_sentence_punctuation() {
        let dimensions = TerminalDimensions {
            columns: 32,
            rows: 1,
        };
        let mut terminal = Term::new(
            Config::default(),
            &dimensions,
            alacritty_terminal::event::VoidListener,
        );
        let text = "see (https://example.com), now";
        for (column, character) in text.chars().enumerate() {
            terminal.grid_mut()[Line(0)][Column(column)].c = character;
        }

        assert_eq!(
            plain_hyperlink_at(&terminal, Point::new(Line(0), Column(12))).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            plain_hyperlink_at(&terminal, Point::new(Line(0), Column(3))),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn real_pty_parses_truecolor_output_and_reports_exit() {
        let terminal = AlacrittyTerminal::spawn_with_shell(
            Uuid::new_v4(),
            &std::env::temp_dir(),
            Some(tty::Shell::new(
                "/bin/sh".into(),
                vec![
                    "-c".into(),
                    r"printf '\033[38;2;12;34;56mPTY_OK\033[0m'".into(),
                ],
            )),
            &HashMap::new(),
        )
        .expect("PTY should start");

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut exited = false;
        let mut found = false;
        while Instant::now() < deadline && (!found || !exited) {
            while let Ok(event) = terminal.events.try_recv() {
                if matches!(event, TerminalEvent::Exit(_)) {
                    exited = true;
                }
                if matches!(event, TerminalEvent::Wakeup) {
                    terminal.acknowledge_wakeup();
                }
            }
            let snapshot = terminal.snapshot();
            found = snapshot
                .lines
                .iter()
                .flat_map(|line| line.iter())
                .any(|cell| cell.text() == "P" && cell.foreground == TerminalRgb::new(12, 34, 56))
                && snapshot
                    .lines
                    .iter()
                    .map(|line| line.iter().map(|cell| cell.text()).collect::<String>())
                    .any(|line| line.contains("PTY_OK"));
            std::thread::sleep(Duration::from_millis(10));
        }
        terminal.shutdown();

        assert!(found, "PTY output did not reach the terminal grid");
        assert!(exited, "PTY child exit was not reported");
    }

    #[cfg(unix)]
    #[test]
    fn real_pty_exposes_alternate_screen_and_mouse_modes() {
        let terminal = AlacrittyTerminal::spawn_with_shell(
            Uuid::new_v4(),
            &std::env::temp_dir(),
            Some(tty::Shell::new(
                "/bin/sh".into(),
                vec![
                    "-c".into(),
                    r"printf '\033[?1049h\033[?1000h\033[?1006h'; sleep 1".into(),
                ],
            )),
            &HashMap::new(),
        )
        .expect("PTY should start");

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut mode = TerminalInputMode::default();
        while Instant::now() < deadline {
            mode = terminal.input_mode();
            if mode.alternate_screen && mode.mouse_report_click && mode.sgr_mouse {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        terminal.shutdown();

        assert!(mode.alternate_screen);
        assert!(mode.mouse_report_click);
        assert!(mode.sgr_mouse);
    }

    #[cfg(unix)]
    #[test]
    fn real_pty_handles_sustained_output_without_losing_the_tail() {
        let terminal = AlacrittyTerminal::spawn_with_shell(
            Uuid::new_v4(),
            &std::env::temp_dir(),
            Some(tty::Shell::new(
                "/bin/sh".into(),
                vec![
                    "-c".into(),
                    r#"i=0; while [ "$i" -lt 4000 ]; do printf 'line-%04d\n' "$i"; i=$((i+1)); done; printf 'STRESS_DONE\n'"#
                        .into(),
                ],
            )),
            &HashMap::new(),
        )
        .expect("PTY should start");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found_tail = false;
        let mut history_size = 0;
        while Instant::now() < deadline && !found_tail {
            while let Ok(event) = terminal.events.try_recv() {
                if matches!(event, TerminalEvent::Wakeup) {
                    terminal.acknowledge_wakeup();
                }
            }
            let snapshot = terminal.snapshot();
            history_size = snapshot.history_size;
            found_tail = snapshot
                .lines
                .iter()
                .map(|line| line.iter().map(|cell| cell.text()).collect::<String>())
                .any(|line| line.contains("STRESS_DONE"));
            std::thread::sleep(Duration::from_millis(10));
        }
        terminal.scroll(i32::MAX);
        let recent = terminal.recent_text(20).unwrap();
        terminal.shutdown();

        assert!(found_tail, "the final PTY output was not rendered");
        assert!(history_size >= 3_000, "scrollback dropped too much output");
        assert!(
            recent.contains("STRESS_DONE"),
            "recent text followed the scrolled viewport instead of the live tail"
        );
    }
}
