use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, ElementInputHandler, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, Hsla, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels,
    Render, ShapedLine, SharedString, StrikethroughStyle, Subscription, Task, TextRun, Timer,
    UTF16Selection, UnderlineStyle, Window, canvas, div, fill, outline, point, prelude::*, px,
    rgba, size,
};
use uuid::Uuid;

use crate::domain::agents::AgentKind;
use crate::ports::terminal::{
    TerminalAgentPresence, TerminalCell, TerminalCellSide, TerminalCursor, TerminalCursorShape,
    TerminalEvent, TerminalHandle, TerminalInputMode, TerminalPoint, TerminalPort, TerminalRgb,
    TerminalSearchDirection, TerminalSelectionType, TerminalSize, TerminalSnapshot,
    TerminalUnderline, is_safe_hyperlink,
};
use crate::ports::terminal_keyboard::{
    TerminalKeyEventType, TerminalKeyInput, TerminalKeystroke, TerminalModifiers,
};
use crate::ui::theme::{
    self, MONO_FONT, colors, floating_surface, popover_surface, surface, surface_tint,
};
use crate::{
    ClearTerminalScrollback, CopyTerminal, DecreaseTerminalFontSize, IncreaseTerminalFontSize,
    PasteTerminal, ResetTerminalFontSize, SearchTerminal, SearchTerminalNext,
    SearchTerminalPrevious,
};

mod agent_presence;
use agent_presence::{detect_agent_presence, is_interactive_shell_process_name};

const TERMINAL_FONT_SIZE: f32 = 12.0;
const TERMINAL_LINE_HEIGHT: f32 = 16.0;
/// Keeps the terminal grid from visually touching the rounded panel edges.
const TERMINAL_VERTICAL_PADDING: f32 = 4.0;
/// Match `PANEL_RADIUS` so the canvas fill doesn't square off card corners.
const SURFACE_CORNER_RADIUS: f32 = 10.0;
const MIN_TERMINAL_FONT_SIZE: f32 = 8.0;
const MAX_TERMINAL_FONT_SIZE: f32 = 32.0;

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);
/// Poll shell/foreground cwd often enough that `cd` feels live in the chrome.
const WORKING_DIRECTORY_POLL_INTERVAL: Duration = Duration::from_millis(500);
const BELL_FLASH_DURATION: Duration = Duration::from_millis(140);
const INPUT_ERROR_VISIBLE_DURATION: Duration = Duration::from_secs(4);
const PROCESS_PROBE_INTERVAL: Duration = Duration::from_millis(250);
const SEARCH_STEP_DELAY: Duration = Duration::from_millis(1);
const MAX_CLIPBOARD_CONFIRMATIONS: usize = 4;
const MAX_CLIPBOARD_READ_BYTES: usize = 1024 * 1024;
const MAX_EXTERNAL_PASTE_BYTES: usize = 1024 * 1024;
const MAX_PENDING_EXTERNAL_PASTES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalInsertStatus {
    Accepted,
    Pending,
    Rejected,
}

#[derive(Clone, Debug)]
pub enum TerminalViewEvent {
    TitleChanged {
        session_id: Uuid,
        title: String,
    },
    WorkingDirectoryChanged {
        session_id: Uuid,
        path: PathBuf,
    },
    Exited {
        session_id: Uuid,
        code: Option<i32>,
    },
    AgentPresenceChanged {
        session_id: Uuid,
        presence: Option<TerminalAgentPresence>,
    },
    FontSizeChanged {
        size: f32,
    },
    Activated {
        session_id: Uuid,
    },
    ContextMenuRequested {
        session_id: Uuid,
        x: f32,
        y: f32,
    },
    ExternalPasteResolved {
        session_id: Uuid,
        token: Uuid,
        accepted: bool,
    },
}

#[derive(Clone)]
enum TerminalConfirmation {
    Paste {
        text: String,
        external_token: Option<Uuid>,
    },
    ClipboardRead {
        contents: String,
        formatter: Arc<dyn Fn(&str) -> String + Send + Sync>,
    },
}

impl TerminalConfirmation {
    fn title(&self) -> String {
        match self {
            Self::Paste { text, .. } => {
                let line_count = text.lines().count().max(1);
                format!("Pegar {line_count} líneas en la terminal?")
            }
            Self::ClipboardRead { .. } => {
                "¿Permitir que la terminal lea el portapapeles?".to_owned()
            }
        }
    }

    fn preview(&self) -> String {
        let text = match self {
            Self::Paste { text, .. } => text,
            Self::ClipboardRead { contents, .. } => contents,
        };
        let mut preview = text
            .chars()
            .take(240)
            .collect::<String>()
            .replace(['\r', '\n'], " ↵ ");
        if text.chars().count() > 240 {
            preview.push('…');
        }
        if preview.is_empty() {
            preview.push_str("(vacío)");
        }
        preview
    }

    fn hint(&self) -> &'static str {
        match self {
            Self::Paste { .. } => "↵ pegar    esc cancelar",
            Self::ClipboardRead { .. } => "↵ permitir    esc denegar",
        }
    }

    fn warning(&self) -> Option<&'static str> {
        matches!(self, Self::ClipboardRead { .. }).then_some(
            "El proceso activo recibirá el contenido mostrado. Acepta solo si confías en él.",
        )
    }
}

pub struct TerminalView {
    session_id: Uuid,
    handle: Option<Arc<dyn TerminalHandle>>,
    focus_handle: FocusHandle,
    title: String,
    working_directory: PathBuf,
    error: Option<SharedString>,
    input_error: Option<SharedString>,
    exited: bool,
    marked_text: String,
    font_size: f32,
    last_cursor_bounds: Option<Bounds<Pixels>>,
    last_terminal_bounds: Option<Bounds<Pixels>>,
    last_cell_width: Option<Pixels>,
    last_line_height: Option<Pixels>,
    last_grid_size: Option<(usize, usize)>,
    last_mouse_point: Option<TerminalPoint>,
    accumulated_scroll_y: f32,
    hovered_hyperlink: Option<String>,
    search_active: bool,
    search_query: String,
    search_match_found: bool,
    search_pending: bool,
    search_generation: u64,
    last_process_probe: Option<Instant>,
    pending_confirmations: VecDeque<TerminalConfirmation>,
    pressed_key_foregrounds: HashMap<String, Option<u32>>,
    cursor_visible: bool,
    cursor_blinking: bool,
    terminal_focused: bool,
    bell_active: bool,
    agent_presence: Option<TerminalAgentPresence>,
    render_cache: Arc<Mutex<TerminalRenderCache>>,
    last_requested_size: Arc<Mutex<Option<TerminalSize>>>,
    surface_visible: bool,
    _focus_subscriptions: Vec<Subscription>,
    _event_task: Option<Task<()>>,
    _cursor_task: Task<()>,
    _working_directory_task: Task<()>,
    _bell_task: Option<Task<()>>,
    _search_task: Option<Task<()>>,
    _input_error_task: Option<Task<()>>,
}

/// A non-interactive, frozen rendering of a terminal used while its pane is
/// being dragged. Keeping it separate from `TerminalView` ensures the drag
/// image cannot resize or send input to the live terminal.
#[derive(Clone)]
pub(crate) struct TerminalDragPreview {
    snapshot: Arc<TerminalSnapshot>,
    width: f32,
    height: f32,
    font_size: f32,
    focused: bool,
    cursor_visible: bool,
    render_cache: Arc<Mutex<TerminalRenderCache>>,
}

impl TerminalView {
    pub fn new_with_environment(
        session_id: Uuid,
        title: String,
        working_directory: &Path,
        terminal_port: Arc<dyn TerminalPort>,
        environment: std::collections::HashMap<String, String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let (handle, error) = match terminal_port.spawn(session_id, working_directory, &environment)
        {
            Ok(handle) => (Some(handle), None),
            Err(error) => (
                None,
                Some(SharedString::from(format!(
                    "No se pudo abrir {}: {error:#}",
                    terminal_port.backend_name()
                ))),
            ),
        };
        let event_task = handle.as_ref().map(|handle| {
            let events = handle.events();
            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    if this
                        .update(cx, |this, cx| this.handle_terminal_event(event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        let cursor_task = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(CURSOR_BLINK_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        if crate::ui::idle::should_run_cursor_blink(
                            this.terminal_focused,
                            this.cursor_blinking,
                        ) {
                            this.cursor_visible = !this.cursor_visible;
                            cx.notify();
                        } else if !this.cursor_visible {
                            this.cursor_visible = true;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let working_directory_task = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(WORKING_DIRECTORY_POLL_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        if !crate::ui::idle::should_poll_terminal_idle(this.surface_visible) {
                            return;
                        }
                        this.refresh_working_directory(cx);
                        // A foreground job can replace the shell without writing a
                        // recognizable banner. Poll its identity as a backstop to
                        // terminal wakeups so its mark still appears promptly.
                        this.refresh_agent_presence(cx);
                        this.last_process_probe = Some(Instant::now());
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        Self {
            session_id,
            handle,
            focus_handle,
            title,
            working_directory: working_directory.to_path_buf(),
            error,
            input_error: None,
            exited: false,
            marked_text: String::new(),
            font_size: TERMINAL_FONT_SIZE,
            last_cursor_bounds: None,
            last_terminal_bounds: None,
            last_cell_width: None,
            last_line_height: None,
            last_grid_size: None,
            last_mouse_point: None,
            accumulated_scroll_y: 0.0,
            hovered_hyperlink: None,
            search_active: false,
            search_query: String::new(),
            search_match_found: false,
            search_pending: false,
            search_generation: 0,
            last_process_probe: None,
            pending_confirmations: VecDeque::new(),
            pressed_key_foregrounds: HashMap::new(),
            cursor_visible: true,
            cursor_blinking: false,
            terminal_focused: false,
            bell_active: false,
            agent_presence: None,
            render_cache: Arc::new(Mutex::new(TerminalRenderCache::default())),
            last_requested_size: Arc::new(Mutex::new(None)),
            surface_visible: true,
            _focus_subscriptions: Vec::new(),
            _event_task: event_task,
            _cursor_task: cursor_task,
            _working_directory_task: working_directory_task,
            _bell_task: None,
            _search_task: None,
            _input_error_task: None,
        }
    }

    pub fn shutdown(&self) {
        if let Some(handle) = &self.handle {
            handle.shutdown();
        }
    }

    pub fn set_surface_visible(&mut self, visible: bool) {
        self.surface_visible = visible;
    }

    /// Frozen visual copy used as the drag image for a pane. Rendering this copy
    /// never resizes or otherwise interacts with the terminal's live PTY.
    pub(crate) fn drag_preview(&self) -> TerminalDragPreview {
        let (width, height) = self
            .last_terminal_bounds
            .map(|bounds| {
                (
                    Into::<f32>::into(bounds.size.width),
                    Into::<f32>::into(bounds.size.height),
                )
            })
            .unwrap_or((640.0, 400.0));
        TerminalDragPreview {
            snapshot: self.snapshot(),
            width,
            height,
            font_size: self.font_size,
            focused: self.terminal_focused,
            cursor_visible: self.cursor_visible,
            render_cache: Arc::new(Mutex::new(TerminalRenderCache::default())),
        }
    }

    #[cfg(test)]
    pub fn is_surface_visible(&self) -> bool {
        self.surface_visible
    }

    pub fn current_working_directory(&self) -> PathBuf {
        self.handle
            .as_ref()
            .and_then(|handle| handle.current_working_directory())
            .unwrap_or_else(|| self.working_directory.clone())
    }

    pub fn cached_working_directory(&self) -> &Path {
        &self.working_directory
    }

    pub fn foreground_process_name(&self) -> Option<String> {
        self.handle
            .as_ref()
            .and_then(|handle| handle.foreground_process_name())
    }

    pub fn apply_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.update_font_size(size, false, cx);
    }

    fn refresh_agent_presence(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.handle.as_ref() else {
            return;
        };
        let process_name = handle.foreground_process_name();
        let process_id = handle.foreground_process_id();
        if process_name
            .as_deref()
            .is_some_and(is_interactive_shell_process_name)
            && self.agent_presence.is_none()
        {
            return;
        }
        let snapshot = handle.snapshot();
        let recent_text = handle.recent_text(14);
        let presence = detect_agent_presence(
            &self.title,
            &snapshot,
            recent_text.as_deref(),
            process_name.as_deref(),
            process_id,
        );
        if presence != self.agent_presence {
            self.agent_presence.clone_from(&presence);
            cx.emit(TerminalViewEvent::AgentPresenceChanged {
                session_id: self.session_id,
                presence,
            });
        }
    }

    fn refresh_working_directory(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .handle
            .as_ref()
            .and_then(|handle| handle.current_working_directory())
        else {
            return;
        };
        if path == self.working_directory {
            return;
        }
        self.working_directory.clone_from(&path);
        cx.emit(TerminalViewEvent::WorkingDirectoryChanged {
            session_id: self.session_id,
            path,
        });
    }

    fn refresh_process_state_if_due(&mut self, cx: &mut Context<Self>) {
        if self
            .last_process_probe
            .is_none_or(|last| last.elapsed() >= PROCESS_PROBE_INTERVAL)
        {
            self.last_process_probe = Some(Instant::now());
            self.refresh_working_directory(cx);
            self.refresh_agent_presence(cx);
        }
    }

    fn set_title(&mut self, title: String, cx: &mut Context<Self>) {
        if self.title == title {
            return;
        }
        self.title.clone_from(&title);
        self.refresh_process_state_if_due(cx);
        cx.emit(TerminalViewEvent::TitleChanged {
            session_id: self.session_id,
            title,
        });
        cx.notify();
    }

    fn emit_external_paste_resolution(&self, token: Uuid, accepted: bool, cx: &mut Context<Self>) {
        cx.emit(TerminalViewEvent::ExternalPasteResolved {
            session_id: self.session_id,
            token,
            accepted,
        });
    }

    fn handle_terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::Wakeup => {
                self.refresh_process_state_if_due(cx);
                if let Some(handle) = &self.handle {
                    handle.acknowledge_wakeup();
                }
                self.cursor_visible = true;
                cx.notify();
            }
            TerminalEvent::Title(title) => self.set_title(title, cx),
            TerminalEvent::ResetTitle => self.set_title("Terminal".to_owned(), cx),
            TerminalEvent::ClipboardStore(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            TerminalEvent::ClipboardLoad(formatter) => {
                let contents = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                let pending_reads = self
                    .pending_confirmations
                    .iter()
                    .filter(|item| matches!(item, TerminalConfirmation::ClipboardRead { .. }))
                    .count();
                if contents.len() > MAX_CLIPBOARD_READ_BYTES
                    || pending_reads >= MAX_CLIPBOARD_CONFIRMATIONS
                {
                    self.send_protocol(formatter("").into_bytes(), cx);
                    return;
                }
                self.pending_confirmations
                    .push_back(TerminalConfirmation::ClipboardRead {
                        contents,
                        formatter,
                    });
                cx.notify();
            }
            TerminalEvent::Bell => {
                self.bell_active = true;
                let timer = cx.background_executor().timer(BELL_FLASH_DURATION);
                self._bell_task = Some(cx.spawn(async move |this, cx| {
                    timer.await;
                    let _ = this.update(cx, |this, cx| {
                        this.bell_active = false;
                        cx.notify();
                    });
                }));
                cx.notify();
            }
            TerminalEvent::Exit(code) => {
                if !self.exited {
                    self.exited = true;
                    let unresolved = self
                        .pending_confirmations
                        .drain(..)
                        .filter_map(|confirmation| match confirmation {
                            TerminalConfirmation::Paste { external_token, .. } => external_token,
                            TerminalConfirmation::ClipboardRead { .. } => None,
                        })
                        .collect::<Vec<_>>();
                    for token in unresolved {
                        self.emit_external_paste_resolution(token, false, cx);
                    }
                    cx.emit(TerminalViewEvent::Exited {
                        session_id: self.session_id,
                        code,
                    });
                    cx.notify();
                }
            }
        }
    }

    fn record_input_result(&mut self, result: anyhow::Result<()>, cx: &mut Context<Self>) -> bool {
        match result {
            Ok(()) => {
                if self.input_error.take().is_some() {
                    self._input_error_task = None;
                    cx.notify();
                }
                true
            }
            Err(error) => {
                self.input_error =
                    Some(format!("No se pudo enviar a la terminal: {error:#}").into());
                self._input_error_task = Some(cx.spawn(async move |this, cx| {
                    Timer::after(INPUT_ERROR_VISIBLE_DURATION).await;
                    let _ = this.update(cx, |this, cx| {
                        this.input_error = None;
                        cx.notify();
                    });
                }));
                cx.notify();
                false
            }
        }
    }

    fn send(&mut self, input: Vec<u8>, cx: &mut Context<Self>) -> bool {
        let Some(handle) = &self.handle else {
            return false;
        };
        handle.clear_selection();
        handle.scroll(i32::MIN);
        self.record_input_result(handle.send_input(input), cx)
    }

    /// Types a command line into the shell and submits it, as the user would.
    /// A freshly spawned shell reads it once its prompt is ready.
    pub fn run_command(&mut self, command: &str, cx: &mut Context<Self>) -> bool {
        let command = command.trim();
        if command.is_empty() || command.contains(['\n', '\r']) {
            return false;
        }
        let mut input = command.as_bytes().to_vec();
        input.push(b'\r');
        self.send(input, cx)
    }

    fn send_protocol(&mut self, input: Vec<u8>, cx: &mut Context<Self>) -> bool {
        let Some(handle) = &self.handle else {
            return false;
        };
        self.record_input_result(handle.send_input(input), cx)
    }

    fn send_key(
        &mut self,
        key: &Keystroke,
        event_type: TerminalKeyEventType,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(handle) = &self.handle else {
            return false;
        };
        handle.clear_selection();
        handle.scroll(i32::MIN);
        self.record_input_result(
            handle.send_key_input(TerminalKeyInput {
                keystroke: terminal_keystroke(key),
                event_type,
            }),
            cx,
        )
    }

    fn paste(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let mode = self
            .handle
            .as_ref()
            .map(|handle| handle.input_mode())
            .unwrap_or_default();
        self.send(paste_bytes(text, mode.bracketed_paste), cx)
    }

    /// Submit text from another view and report when a required paste
    /// confirmation is resolved. The caller owns `token` and can retain its
    /// source data until it sees Accepted or a successful resolution event.
    pub fn insert_external_text(
        &mut self,
        text: &str,
        token: Uuid,
        cx: &mut Context<Self>,
    ) -> TerminalInsertStatus {
        let Some(handle) = &self.handle else {
            return TerminalInsertStatus::Rejected;
        };
        if text.is_empty() || text.len() > MAX_EXTERNAL_PASTE_BYTES {
            return TerminalInsertStatus::Rejected;
        }
        if !handle.input_mode().bracketed_paste && paste_requires_confirmation(text) {
            let pending = self
                .pending_confirmations
                .iter()
                .filter(|confirmation| {
                    matches!(
                        confirmation,
                        TerminalConfirmation::Paste {
                            external_token: Some(_),
                            ..
                        }
                    )
                })
                .count();
            if pending >= MAX_PENDING_EXTERNAL_PASTES {
                return TerminalInsertStatus::Rejected;
            }
            self.pending_confirmations
                .push_back(TerminalConfirmation::Paste {
                    text: text.to_owned(),
                    external_token: Some(token),
                });
            cx.notify();
            return TerminalInsertStatus::Pending;
        }
        if self.paste(text, cx) {
            self.reset_cursor_blink();
            cx.notify();
            TerminalInsertStatus::Accepted
        } else {
            TerminalInsertStatus::Rejected
        }
    }

    fn request_paste(&mut self, text: String, cx: &mut Context<Self>) {
        // When the app enabled bracketed paste, inject immediately (Warp/iTerm).
        // Confirm only for raw pastes that could execute as typed input.
        let bracketed = self
            .handle
            .as_ref()
            .map(|handle| handle.input_mode().bracketed_paste)
            .unwrap_or(false);
        if !bracketed && paste_requires_confirmation(&text) {
            self.pending_confirmations
                .push_back(TerminalConfirmation::Paste {
                    text,
                    external_token: None,
                });
            cx.notify();
        } else {
            self.paste(&text, cx);
            self.reset_cursor_blink();
            cx.notify();
        }
    }

    /// System paste (⌘V / Edit → Paste), following Warp's terminal paste path:
    /// `warpdotdev/warp` → `app/src/terminal/view.rs` → `TerminalView::paste`.
    ///
    /// 1. CLI agent + clipboard image (macOS): write Ctrl+V (`C0::SYN` / `0x16`)
    ///    to the PTY so the agent reads the image from the system clipboard.
    /// 2. Otherwise: inject clipboard text, with bracketed paste when enabled.
    fn paste_from_system_clipboard(&mut self, cx: &mut Context<Self>) {
        let item = cx.read_from_clipboard();
        let has_image = item.as_ref().is_some_and(clipboard_has_image);

        // Warp: `is_cli_agent_paste && clipboard_content.has_image_data()` then
        // `write_user_bytes_to_pty(vec![escape_sequences::C0::SYN], …)` on !windows.
        if has_image && self.has_active_cli_agent() {
            self.send_protocol(vec![0x16], cx);
            self.reset_cursor_blink();
            cx.notify();
            return;
        }

        if let Some(text) = item
            .and_then(|item| item.text())
            .filter(|text| !text.is_empty())
        {
            self.request_paste(text, cx);
        }
    }

    /// Whether a CLI coding agent is the foreground context for paste routing.
    fn has_active_cli_agent(&self) -> bool {
        if self.agent_presence.is_some() {
            return true;
        }
        self.foreground_process_name()
            .as_deref()
            .and_then(AgentKind::from_process_name)
            .is_some()
    }

    fn confirm_pending_action(&mut self, cx: &mut Context<Self>) {
        match self.pending_confirmations.pop_front() {
            Some(TerminalConfirmation::Paste {
                text,
                external_token,
            }) => {
                let accepted = self.paste(&text, cx);
                if accepted {
                    self.reset_cursor_blink();
                }
                if let Some(token) = external_token {
                    self.emit_external_paste_resolution(token, accepted, cx);
                }
            }
            Some(TerminalConfirmation::ClipboardRead {
                contents,
                formatter,
            }) => {
                self.send_protocol(formatter(&contents).into_bytes(), cx);
            }
            None => {}
        }
        cx.notify();
    }

    fn cancel_pending_action(&mut self, cx: &mut Context<Self>) {
        match self.pending_confirmations.pop_front() {
            Some(TerminalConfirmation::ClipboardRead { formatter, .. }) => {
                // Complete the protocol with an empty clipboard; never leave
                // the requesting program waiting or disclose captured data.
                self.send_protocol(formatter("").into_bytes(), cx);
            }
            Some(TerminalConfirmation::Paste {
                external_token: Some(token),
                ..
            }) => {
                self.emit_external_paste_resolution(token, false, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn copy_selection(&self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self
            .handle
            .as_ref()
            .and_then(|handle| handle.selection_text())
            .filter(|text| !text.is_empty())
        else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn start_search(&mut self, cx: &mut Context<Self>) {
        self.search_active = true;
        self.marked_text.clear();
        if !self.search_query.is_empty() {
            self.refresh_search(cx);
        }
        cx.notify();
    }

    fn close_search(&mut self, clear_selection: bool, cx: &mut Context<Self>) {
        self.search_active = false;
        self.search_pending = false;
        self.search_generation = self.search_generation.wrapping_add(1);
        self._search_task = None;
        self.marked_text.clear();
        if let Some(handle) = &self.handle {
            let _ = handle.search_step("", TerminalSearchDirection::Next, false);
        }
        if clear_selection && let Some(handle) = &self.handle {
            handle.clear_selection();
        }
        cx.notify();
    }

    fn search(&mut self, direction: TerminalSearchDirection, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self._search_task = None;
        let generation = self.search_generation;
        let query = self.search_query.clone();
        let Some(handle) = self.handle.clone() else {
            self.search_pending = false;
            self.search_match_found = false;
            return;
        };
        match handle.search_step(&query, direction, false) {
            Ok(Some(found)) => {
                self.search_match_found = found;
                self.search_pending = false;
            }
            Ok(None) => {
                self.search_match_found = false;
                self.search_pending = true;
                self._search_task = Some(cx.spawn(async move |this, cx| {
                    loop {
                        Timer::after(SEARCH_STEP_DELAY).await;
                        let active = this.update(cx, |this, cx| {
                            if this.search_generation != generation || !this.search_active {
                                return false;
                            }
                            match handle.search_step(&query, direction, true) {
                                Ok(Some(found)) => {
                                    this.search_match_found = found;
                                    this.search_pending = false;
                                    cx.notify();
                                    false
                                }
                                Ok(None) => true,
                                Err(_) => {
                                    this.search_match_found = false;
                                    this.search_pending = false;
                                    cx.notify();
                                    false
                                }
                            }
                        });
                        if !matches!(active, Ok(true)) {
                            break;
                        }
                    }
                }));
            }
            Err(_) => {
                self.search_match_found = false;
                self.search_pending = false;
            }
        }
    }

    fn refresh_search(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.handle {
            handle.clear_selection();
        }
        self.search(TerminalSearchDirection::Next, cx);
    }

    fn reset_cursor_blink(&mut self) {
        self.cursor_visible = true;
    }

    fn copy_action(&mut self, _: &CopyTerminal, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    fn paste_action(&mut self, _: &PasteTerminal, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_from_system_clipboard(cx);
    }

    fn search_action(&mut self, _: &SearchTerminal, _: &mut Window, cx: &mut Context<Self>) {
        self.start_search(cx);
    }

    fn search_next_action(
        &mut self,
        _: &SearchTerminalNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.search_active {
            self.search_active = true;
        }
        self.search(TerminalSearchDirection::Next, cx);
        cx.notify();
    }

    fn search_previous_action(
        &mut self,
        _: &SearchTerminalPrevious,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.search_active {
            self.search_active = true;
        }
        self.search(TerminalSearchDirection::Previous, cx);
        cx.notify();
    }

    fn increase_font_size_action(
        &mut self,
        _: &IncreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_font_size(self.font_size + 1.0, cx);
    }

    fn decrease_font_size_action(
        &mut self,
        _: &DecreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_font_size(self.font_size - 1.0, cx);
    }

    fn reset_font_size_action(
        &mut self,
        _: &ResetTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_font_size(TERMINAL_FONT_SIZE, cx);
    }

    fn clear_scrollback_action(
        &mut self,
        _: &ClearTerminalScrollback,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(handle) = &self.handle {
            handle.clear_scrollback();
        }
        self.send_protocol(vec![0x0c], cx);
        self.reset_cursor_blink();
        cx.notify();
    }

    fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.update_font_size(size, true, cx);
    }

    fn update_font_size(&mut self, size: f32, emit: bool, cx: &mut Context<Self>) {
        let size = size.clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE);
        if self.font_size != size {
            self.font_size = size;
            if emit {
                cx.emit(TerminalViewEvent::FontSizeChanged { size });
            }
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.to_ascii_lowercase();

        if !self.pending_confirmations.is_empty() {
            match key.as_str() {
                "enter" | "return" => self.confirm_pending_action(cx),
                "escape" | "esc" => self.cancel_pending_action(cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }

        if self.search_active {
            match key.as_str() {
                "escape" | "esc" => self.close_search(true, cx),
                "enter" | "return" => {
                    let direction = if keystroke.modifiers.shift {
                        TerminalSearchDirection::Previous
                    } else {
                        TerminalSearchDirection::Next
                    };
                    self.search(direction, cx);
                    cx.notify();
                }
                "backspace" => {
                    self.search_query.pop();
                    self.refresh_search(cx);
                    cx.notify();
                }
                _ if keystroke.modifiers.platform && key == "g" && keystroke.modifiers.shift => {
                    self.search(TerminalSearchDirection::Previous, cx);
                    cx.notify();
                }
                _ if keystroke.modifiers.platform && key == "g" => {
                    self.search(TerminalSearchDirection::Next, cx);
                    cx.notify();
                }
                _ if keystroke.modifiers.platform && key == "f" => {}
                _ if keystroke.modifiers.platform => {}
                _ if is_terminal_special_key(&key) => {}
                _ => return,
            }
            cx.stop_propagation();
            return;
        }

        if keystroke.modifiers.platform {
            match key.as_str() {
                "c" => {
                    self.copy_selection(cx);
                    cx.stop_propagation();
                }
                "v" => {
                    self.paste_from_system_clipboard(cx);
                    cx.stop_propagation();
                }
                "f" => {
                    self.start_search(cx);
                    cx.stop_propagation();
                }
                "=" | "+" => {
                    self.set_font_size(self.font_size + 1.0, cx);
                    cx.stop_propagation();
                }
                "-" => {
                    self.set_font_size(self.font_size - 1.0, cx);
                    cx.stop_propagation();
                }
                "0" => {
                    self.set_font_size(TERMINAL_FONT_SIZE, cx);
                    cx.stop_propagation();
                }
                "k" => {
                    if let Some(handle) = &self.handle {
                        handle.clear_scrollback();
                    }
                    self.send_protocol(vec![0x0c], cx);
                    self.reset_cursor_blink();
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }

        let mode = self
            .handle
            .as_ref()
            .map(|handle| handle.input_mode())
            .unwrap_or_default();
        let event_type = if event.is_held {
            TerminalKeyEventType::Repeat
        } else {
            TerminalKeyEventType::Press
        };
        if key_event_bytes(keystroke, mode, event_type).is_some() {
            if event_type == TerminalKeyEventType::Press {
                self.pressed_key_foregrounds.insert(
                    key,
                    self.handle
                        .as_ref()
                        .and_then(|handle| handle.foreground_process_id()),
                );
            }
            self.send_key(keystroke, event_type, cx);
            self.reset_cursor_blink();
            cx.stop_propagation();
        }
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let pressed_foreground = self
            .pressed_key_foregrounds
            .remove(&event.keystroke.key.to_ascii_lowercase());
        if self.search_active || event.keystroke.modifiers.platform {
            return;
        }
        let Some(pressed_foreground) = pressed_foreground else {
            return;
        };
        let current_foreground = self
            .handle
            .as_ref()
            .and_then(|handle| handle.foreground_process_id());
        if matches!((pressed_foreground, current_foreground), (Some(pressed), Some(current)) if pressed != current)
        {
            return;
        }
        let mode = self
            .handle
            .as_ref()
            .map(|handle| handle.input_mode())
            .unwrap_or_default();
        if key_event_bytes(&event.keystroke, mode, TerminalKeyEventType::Release).is_some() {
            self.send_key(&event.keystroke, TerminalKeyEventType::Release, cx);
            cx.stop_propagation();
        }
    }

    fn on_scroll(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line_height = self
            .last_line_height
            .map(f32::from)
            .unwrap_or(TERMINAL_LINE_HEIGHT);
        let pixels: f32 = event.delta.pixel_delta(px(line_height)).y.into();
        self.accumulated_scroll_y += pixels;
        let lines = (self.accumulated_scroll_y / line_height).trunc() as i32;
        if lines == 0 {
            return;
        }
        self.accumulated_scroll_y -= lines as f32 * line_height;

        let Some(handle) = &self.handle else {
            return;
        };
        let mode = handle.input_mode();
        let point = self
            .terminal_point(event.position, true)
            .map(|(point, _)| point)
            .unwrap_or_default();
        if mode.mouse_mode() && !event.modifiers.shift {
            let button = if lines > 0 { 64 } else { 65 };
            for _ in 0..lines.unsigned_abs() {
                if let Some(bytes) = mouse_report_bytes(
                    point,
                    button,
                    MouseReportState::Pressed,
                    event.modifiers,
                    mode,
                ) {
                    self.send_protocol(bytes, cx);
                }
            }
        } else if mode.alternate_screen && mode.alternate_scroll && !event.modifiers.shift {
            let sequence = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
            self.send_protocol(sequence.repeat(lines.unsigned_abs() as usize), cx);
        } else {
            handle.scroll(lines);
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        cx.emit(TerminalViewEvent::Activated {
            session_id: self.session_id,
        });
        self.reset_cursor_blink();
        let Some((point, side)) = self.terminal_point(event.position, true) else {
            return;
        };
        self.last_mouse_point = Some(point);

        if event.button == MouseButton::Left
            && event.modifiers.platform
            && let Some(uri) = self
                .handle
                .as_ref()
                .and_then(|handle| handle.hyperlink_at(point))
                .filter(|uri| is_safe_hyperlink(uri))
        {
            cx.open_url(&uri);
            cx.stop_propagation();
            return;
        }

        let Some(handle) = &self.handle else {
            return;
        };
        let mode = handle.input_mode();
        if mode.mouse_mode() && !event.modifiers.shift {
            if let Some(button) = mouse_button_code(event.button)
                && let Some(bytes) = mouse_report_bytes(
                    point,
                    button,
                    MouseReportState::Pressed,
                    event.modifiers,
                    mode,
                )
            {
                self.send_protocol(bytes, cx);
                cx.stop_propagation();
            }
            return;
        }

        if event.button == MouseButton::Left {
            let selection_type = match event.click_count {
                count if count >= 3 && !event.modifiers.control => TerminalSelectionType::Lines,
                2 if !event.modifiers.control => TerminalSelectionType::Semantic,
                _ if event.modifiers.control => TerminalSelectionType::Block,
                _ => TerminalSelectionType::Simple,
            };
            handle.start_selection(selection_type, point, side);
            cx.notify();
            cx.stop_propagation();
            return;
        }

        if event.button == MouseButton::Right {
            let x: f32 = event.position.x.into();
            let y: f32 = event.position.y.into();
            cx.emit(TerminalViewEvent::ContextMenuRequested {
                session_id: self.session_id,
                x,
                y,
            });
            cx.stop_propagation();
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((point, side)) = self.terminal_point(event.position, true) else {
            return;
        };
        let Some(handle) = &self.handle else {
            return;
        };
        let mode = handle.input_mode();
        if mode.mouse_mode() && !event.modifiers.shift {
            if let Some(button) = mouse_button_code(event.button)
                && let Some(bytes) = mouse_report_bytes(
                    point,
                    button,
                    MouseReportState::Released,
                    event.modifiers,
                    mode,
                )
            {
                self.send_protocol(bytes, cx);
                cx.stop_propagation();
            }
        } else if event.button == MouseButton::Left {
            handle.update_selection(point, side);
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inside = self.terminal_point(event.position, false);
        let hovered_hyperlink = inside
            .and_then(|(point, _)| self.handle.as_ref()?.hyperlink_at(point))
            .filter(|uri| is_safe_hyperlink(uri));
        if hovered_hyperlink != self.hovered_hyperlink {
            self.hovered_hyperlink = hovered_hyperlink;
            cx.notify();
        }

        let Some((point, side)) = self.terminal_point(event.position, true) else {
            return;
        };
        let cell_changed = self.last_mouse_point != Some(point);
        self.last_mouse_point = Some(point);
        let selecting = event.dragging() || event.pressed_button == Some(MouseButton::Left);
        let outside_vertically = self.last_terminal_bounds.is_some_and(|bounds| {
            event.position.y < bounds.top() || event.position.y >= bounds.bottom()
        });
        if !(cell_changed || selecting && outside_vertically) {
            return;
        }
        let Some(handle) = &self.handle else {
            return;
        };
        let mode = handle.input_mode();
        if mode.mouse_mode() && !event.modifiers.shift {
            let should_report =
                mode.mouse_motion || (mode.mouse_drag && event.pressed_button.is_some());
            if should_report {
                let button = event
                    .pressed_button
                    .and_then(mouse_button_code)
                    .map(|button| button + 32)
                    .unwrap_or(35);
                if let Some(bytes) = mouse_report_bytes(
                    point,
                    button,
                    MouseReportState::Pressed,
                    event.modifiers,
                    mode,
                ) {
                    self.send_protocol(bytes, cx);
                    cx.stop_propagation();
                }
            }
        } else if selecting {
            if let Some(bounds) = self.last_terminal_bounds {
                if event.position.y < bounds.top() {
                    handle.scroll(1);
                } else if event.position.y >= bounds.bottom() {
                    handle.scroll(-1);
                }
            }
            handle.update_selection(point, side);
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn terminal_point(
        &self,
        position: gpui::Point<Pixels>,
        clamp: bool,
    ) -> Option<(TerminalPoint, TerminalCellSide)> {
        let bounds = self.last_terminal_bounds?;
        let cell_width: f32 = self.last_cell_width?.into();
        let line_height: f32 = self.last_line_height?.into();
        if !clamp && !bounds.contains(&position) {
            return None;
        }
        let x: f32 = (position.x - bounds.left()).into();
        let y: f32 = (position.y - bounds.top()).into();
        let (columns, rows) = self.last_grid_size?;
        if columns == 0 || rows == 0 {
            return None;
        }
        let column = (x / cell_width)
            .floor()
            .clamp(0.0, columns.saturating_sub(1) as f32) as usize;
        let row = (y / line_height)
            .floor()
            .clamp(0.0, rows.saturating_sub(1) as f32) as usize;
        let cell_x = x.max(0.0) % cell_width;
        let side = if cell_x > cell_width / 2.0 {
            TerminalCellSide::Right
        } else {
            TerminalCellSide::Left
        };
        Some((TerminalPoint { row, column }, side))
    }

    fn install_focus_observers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self._focus_subscriptions.is_empty() {
            return;
        }
        let focus_handle = self.focus_handle.clone();
        let focus = cx.on_focus(&focus_handle, window, |this, _, cx| {
            this.terminal_focused = true;
            this.reset_cursor_blink();
            let mode = this
                .handle
                .as_ref()
                .map(|handle| handle.input_mode())
                .unwrap_or_default();
            if mode.focus_reporting {
                this.send_protocol(b"\x1b[I".to_vec(), cx);
            }
            cx.notify();
        });
        let blur = cx.on_blur(&focus_handle, window, |this, _, cx| {
            this.terminal_focused = false;
            this.cursor_visible = true;
            let mode = this
                .handle
                .as_ref()
                .map(|handle| handle.input_mode())
                .unwrap_or_default();
            if mode.focus_reporting {
                this.send_protocol(b"\x1b[O".to_vec(), cx);
            }
            cx.notify();
        });
        self._focus_subscriptions.extend([focus, blur]);
    }

    fn snapshot(&self) -> Arc<TerminalSnapshot> {
        self.handle
            .as_ref()
            .map(|handle| handle.snapshot())
            .unwrap_or_else(|| {
                Arc::new(TerminalSnapshot {
                    columns: 0,
                    rows: 0,
                    lines: Vec::new(),
                    cursor: None,
                    display_offset: 0,
                    history_size: 0,
                })
            })
    }
}

impl TerminalDragPreview {
    pub(crate) fn empty() -> Self {
        Self {
            snapshot: Arc::new(TerminalSnapshot {
                columns: 0,
                rows: 0,
                lines: Vec::new(),
                cursor: None,
                display_offset: 0,
                history_size: 0,
            }),
            width: 640.0,
            height: 400.0,
            font_size: TERMINAL_FONT_SIZE,
            focused: false,
            cursor_visible: false,
            render_cache: Arc::new(Mutex::new(TerminalRenderCache::default())),
        }
    }
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf16: Vec<_> = self.marked_text.encode_utf16().collect();
        let start = range.start.min(utf16.len());
        let end = range.end.min(utf16.len()).max(start);
        actual_range.replace(start..end);
        String::from_utf16(&utf16[start..end]).ok()
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.marked_text.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked_text.is_empty()).then(|| 0..self.marked_text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text.clear();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text.clear();
        if !new_text.is_empty() {
            if self.search_active {
                self.search_query.push_str(new_text);
                self.refresh_search(cx);
            } else if new_text.chars().count() > 1 {
                self.paste(new_text, cx);
                self.reset_cursor_blink();
            } else {
                self.send(new_text.as_bytes().to_vec(), cx);
                self.reset_cursor_blink();
            }
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text.clear();
        self.marked_text.push_str(new_text);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.last_cursor_bounds.or(Some(Bounds::new(
            bounds.origin,
            size(px(1.0), px(TERMINAL_LINE_HEIGHT)),
        )))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

struct TerminalPaintState {
    lines: Vec<ShapedLine>,
    /// Adjacent cells share a tint quad, including selections and TUI fills.
    backgrounds: Vec<TerminalBackgroundRun>,
    grid_bounds: Bounds<Pixels>,
    /// Live surface color for the canvas, including padding outside the grid.
    surface: Hsla,
    cursor: Option<PaintQuad>,
    cursor_bounds: Option<Bounds<Pixels>>,
    composition: Option<ShapedLine>,
    cursor_blinking: bool,
    cell_width: Pixels,
    line_height: Pixels,
    grid_size: (usize, usize),
}

#[derive(Debug)]
struct TerminalBackgroundRun {
    row: usize,
    columns: Range<usize>,
    color: Hsla,
}

impl TerminalPaintState {
    fn paint_backgrounds(&self, bounds: Bounds<Pixels>, floating: bool, window: &mut Window) {
        let grid = self.grid_bounds;
        let background = if floating {
            floating_surface(self.surface)
        } else {
            surface(self.surface)
        };
        window.paint_quad(fill(bounds, background).corner_radii(px(SURFACE_CORNER_RADIUS)));
        let scale = window.scale_factor();
        // Shared edges land on the same physical pixel, preventing translucent
        // seams when pane dimensions are not divisible by the terminal grid.
        let snap = |value: Pixels| (value * scale).round() / scale;
        for run in &self.backgrounds {
            if run.color == self.surface {
                continue;
            }
            let left = snap(grid.left() + self.cell_width * run.columns.start);
            let top = snap(grid.top() + self.line_height * run.row);
            let width = snap(grid.left() + self.cell_width * run.columns.end) - left;
            let height = snap(grid.top() + self.line_height * (run.row + 1)) - top;
            if width > px(0.0) && height > px(0.0) {
                window.paint_quad(fill(
                    Bounds::new(point(left, top), size(width, height)),
                    surface_tint(run.color.into(), self.surface.into()),
                ));
            }
        }
    }
}

fn terminal_grid_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let padding = px(TERMINAL_VERTICAL_PADDING).min(bounds.size.height / 2.0);
    Bounds::new(
        point(bounds.left(), bounds.top() + padding),
        size(bounds.size.width, bounds.size.height - padding * 2.0),
    )
}

#[derive(Default)]
struct TerminalRenderCache {
    snapshot: Option<Arc<TerminalSnapshot>>,
    lines: Vec<ShapedLine>,
    font_size: f32,
    cell_width: f32,
    focused: bool,
    cursor_visible: bool,
    theme_generation: u64,
}

struct TerminalShapeContext<'a> {
    base_font: &'a gpui::Font,
    font_size: Pixels,
    cell_width: Pixels,
    focused: bool,
    cursor_visible: bool,
    window: &'a Window,
}

fn search_overlay(
    query: String,
    marked_text: SharedString,
    found: bool,
    pending: bool,
) -> impl IntoElement {
    let status = if query.is_empty() {
        "Escribe para buscar"
    } else if pending {
        "Buscando…"
    } else if found {
        "↵ siguiente  ⇧↵ anterior"
    } else {
        "Sin resultados"
    };
    let status_color = if found || pending || query.is_empty() {
        colors().muted
    } else {
        colors().danger
    };
    div()
        .absolute()
        .top_3()
        .left_3()
        .right_3()
        .max_w(px(420.0))
        .ml_auto()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(colors().border_subtle)
        .bg(popover_surface())
        .shadow_sm()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .truncate()
                .text_sm()
                .text_color(colors().foreground)
                .child(format!("Buscar  {query}{marked_text}")),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(status_color)
                .child(status),
        )
}

fn confirmation_overlay(confirmation: TerminalConfirmation) -> impl IntoElement {
    let title = confirmation.title();
    let preview = confirmation.preview();
    let hint = confirmation.hint();
    let warning = confirmation.warning();
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(colors().overlay())
        .child(
            div()
                .w(px(480.0))
                .max_w_full()
                .mx_4()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(colors().border_subtle)
                .bg(popover_surface())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().text_color(colors().foreground).child(title))
                .when_some(warning, |dialog, warning| {
                    dialog.child(div().text_xs().text_color(colors().danger).child(warning))
                })
                .child(
                    div()
                        .px_2()
                        .py_2()
                        .rounded_sm()
                        .bg(surface_tint(colors().terminal, colors().sidebar))
                        .text_xs()
                        .text_color(colors().muted)
                        .overflow_hidden()
                        .child(preview),
                )
                .child(div().text_xs().text_color(colors().muted).child(hint)),
        )
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.install_focus_observers(window, cx);
        let entity = cx.entity();
        let handle = self.handle.clone();
        let marked_text: SharedString = self.marked_text.clone().into();
        let canvas_marked_text = marked_text.clone();
        let error = self.error.clone();
        let input_error = self.input_error.clone();
        let exited = self.exited;
        let search_active = self.search_active;
        let search_query = self.search_query.clone();
        let search_match_found = self.search_match_found;
        let search_pending = self.search_pending;
        let pending_confirmation = self.pending_confirmations.front().cloned();
        let cursor_visible = self.cursor_visible;
        let bell_active = self.bell_active;
        let hyperlink_hovered = self.hovered_hyperlink.is_some();
        let render_cache = self.render_cache.clone();
        let last_requested_size = self.last_requested_size.clone();
        let font_size = self.font_size;
        let line_height = font_size + (TERMINAL_LINE_HEIGHT - TERMINAL_FONT_SIZE);

        div()
            .id(SharedString::from(format!("terminal-{}", self.session_id)))
            .size_full()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .rounded(px(SURFACE_CORNER_RADIUS))
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .on_action(cx.listener(Self::copy_action))
            .on_action(cx.listener(Self::paste_action))
            .on_action(cx.listener(Self::search_action))
            .on_action(cx.listener(Self::search_next_action))
            .on_action(cx.listener(Self::search_previous_action))
            .on_action(cx.listener(Self::increase_font_size_action))
            .on_action(cx.listener(Self::decrease_font_size_action))
            .on_action(cx.listener(Self::reset_font_size_action))
            .on_action(cx.listener(Self::clear_scrollback_action))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .font_family(MONO_FONT)
            .font_weight(gpui::FontWeight::LIGHT)
            .text_size(px(font_size))
            .line_height(px(line_height))
            .border_1()
            .border_color(if bell_active {
                colors().danger
            } else {
                gpui::rgba(0x00000000)
            })
            .cursor(gpui::CursorStyle::IBeam)
            .when(hyperlink_hovered, |terminal| terminal.cursor_pointer())
            .child(
                canvas(
                    {
                        let entity = entity.clone();
                        move |bounds, window, cx| {
                            let bounds = terminal_grid_bounds(bounds);
                            let style = window.text_style();
                            let rem_size = window.rem_size();
                            let font_size = style.font_size.to_pixels(rem_size);
                            let line_height = style.line_height_in_pixels(rem_size);
                            let base_font = style.font();
                            let measure = window.text_system().shape_line(
                                "M".into(),
                                font_size,
                                &[TextRun {
                                    len: 1,
                                    font: base_font.clone(),
                                    color: style.color,
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                }],
                                None,
                            );
                            // Target grid from natural glyph metrics (what the PTY should be).
                            let natural_cell_width: f32 = measure.width.ceil().into();
                            let natural_line_height: f32 = line_height.into();
                            let width: f32 = bounds.size.width.into();
                            let height: f32 = bounds.size.height.into();
                            let target_columns = (width / natural_cell_width)
                                .floor()
                                .clamp(2.0, u16::MAX as f32)
                                as u16;
                            let target_rows = (height / natural_line_height)
                                .floor()
                                .clamp(1.0, u16::MAX as f32)
                                as u16;
                            if let Some(handle) = &handle {
                                let size = TerminalSize {
                                    columns: target_columns,
                                    rows: target_rows,
                                    cell_width: natural_cell_width,
                                    cell_height: natural_line_height,
                                };
                                let mut last = last_requested_size
                                    .lock()
                                    .expect("terminal resize cache poisoned");
                                if *last != Some(size) && handle.resize(size).is_ok() {
                                    *last = Some(size);
                                }
                            }
                            let snapshot = entity.read(cx).snapshot();
                            // Stretch paint metrics to the *live* snapshot grid so every
                            // cell covers the pane. Using target_* while the PTY still has
                            // fewer columns/rows left a gray strip where full-screen TUIs
                            // looked cut off (cols_snap * width/cols_target < width).
                            let paint_columns = snapshot.columns.max(1) as f32;
                            let paint_rows = snapshot.rows.max(1) as f32;
                            let cell_width_f32 = width / paint_columns;
                            let line_height_f32 = height / paint_rows;
                            let cell_width = px(cell_width_f32);
                            let line_height = px(line_height_f32);
                            let focused = entity.read(cx).focus_handle.is_focused(window);
                            let shape_context = TerminalShapeContext {
                                base_font: &base_font,
                                font_size,
                                cell_width,
                                focused,
                                cursor_visible,
                                window,
                            };
                            let lines = shape_snapshot_cached(
                                &mut render_cache.lock().expect("terminal render cache poisoned"),
                                snapshot.clone(),
                                &shape_context,
                            );
                            let backgrounds = collect_background_runs(&snapshot);
                            let surface = snapshot_surface_color(&snapshot);
                            let cursor_bounds = snapshot.cursor.map(|cursor| {
                                cursor_bounds(bounds, cursor, cell_width, line_height)
                            });
                            let cursor = snapshot.cursor.and_then(|cursor| {
                                (!cursor.blinking || cursor_visible).then(|| {
                                    cursor_quad(bounds, cursor, cell_width, line_height, focused)
                                })?
                            });
                            let composition = (!search_active && !canvas_marked_text.is_empty())
                                .then(|| {
                                    window.text_system().shape_line(
                                        canvas_marked_text.clone(),
                                        font_size,
                                        &[TextRun {
                                            len: canvas_marked_text.len(),
                                            font: base_font,
                                            color: colors().foreground.into(),
                                            background_color: Some(colors().elevated.into()),
                                            underline: Some(UnderlineStyle {
                                                thickness: px(1.0),
                                                color: Some(colors().foreground.into()),
                                                wavy: false,
                                            }),
                                            strikethrough: None,
                                        }],
                                        Some(cell_width),
                                    )
                                });
                            TerminalPaintState {
                                lines,
                                backgrounds,
                                grid_bounds: bounds,
                                surface,
                                cursor,
                                cursor_bounds,
                                composition,
                                cursor_blinking: snapshot
                                    .cursor
                                    .is_some_and(|cursor| cursor.blinking),
                                cell_width,
                                line_height,
                                grid_size: (snapshot.columns, snapshot.rows),
                            }
                        }
                    },
                    move |bounds, state, window, cx| {
                        state.paint_backgrounds(bounds, false, window);
                        let bounds = state.grid_bounds;
                        window.handle_input(
                            &entity.read(cx).focus_handle,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        if let Some(cursor) = state.cursor {
                            window.paint_quad(cursor);
                        }
                        for (row, line) in state.lines.iter().enumerate() {
                            let origin =
                                point(bounds.left(), bounds.top() + state.line_height * row);
                            let _ = line.paint(origin, state.line_height, window, cx);
                        }
                        if let (Some(composition), Some(cursor_bounds)) =
                            (state.composition, state.cursor_bounds)
                        {
                            let _ = composition.paint_background(
                                cursor_bounds.origin,
                                state.line_height,
                                window,
                                cx,
                            );
                            let _ = composition.paint(
                                cursor_bounds.origin,
                                state.line_height,
                                window,
                                cx,
                            );
                        }
                        entity.update(cx, |this, _| {
                            this.last_cursor_bounds = state.cursor_bounds;
                            this.last_terminal_bounds = Some(bounds);
                            this.last_cell_width = Some(state.cell_width);
                            this.last_line_height = Some(state.line_height);
                            this.last_grid_size = Some(state.grid_size);
                            this.cursor_blinking = state.cursor_blinking;
                        });
                    },
                )
                .size_full(),
            )
            .when(search_active, |terminal| {
                terminal.child(search_overlay(
                    search_query,
                    marked_text,
                    search_match_found,
                    search_pending,
                ))
            })
            .when_some(pending_confirmation, |terminal, confirmation| {
                terminal.child(confirmation_overlay(confirmation))
            })
            .when_some(error, |terminal, error| {
                terminal.child(
                    div()
                        .absolute()
                        .inset_0()
                        .p_5()
                        .font_family(MONO_FONT)
                        .text_sm()
                        .text_color(colors().danger)
                        .child(error),
                )
            })
            .when_some(input_error, |terminal, input_error| {
                terminal.child(
                    div()
                        .absolute()
                        .left_3()
                        .bottom_3()
                        .max_w(px(500.0))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(colors().danger)
                        .bg(popover_surface())
                        .text_xs()
                        .text_color(colors().danger)
                        .child(input_error),
                )
            })
            .when(exited, |terminal| {
                terminal.child(
                    div()
                        .absolute()
                        .right_3()
                        .bottom_3()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(popover_surface())
                        .text_xs()
                        .text_color(colors().muted)
                        .child("proceso finalizado"),
                )
            })
    }
}

impl Render for TerminalDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.snapshot.clone();
        let render_cache = self.render_cache.clone();
        let focused = self.focused;
        let cursor_visible = self.cursor_visible;
        let font_size = self.font_size;
        let line_height = font_size + (TERMINAL_LINE_HEIGHT - TERMINAL_FONT_SIZE);

        div()
            .w(px(self.width))
            .h(px(self.height))
            .relative()
            .overflow_hidden()
            .rounded(px(SURFACE_CORNER_RADIUS))
            .font_family(MONO_FONT)
            .font_weight(gpui::FontWeight::LIGHT)
            .text_size(px(font_size))
            .line_height(px(line_height))
            .child(
                canvas(
                    move |bounds, window, _| {
                        let style = window.text_style();
                        let rem_size = window.rem_size();
                        let font_size = style.font_size.to_pixels(rem_size);
                        let base_font = style.font();
                        let snapshot = snapshot.clone();
                        let paint_columns = snapshot.columns.max(1) as f32;
                        let paint_rows = snapshot.rows.max(1) as f32;
                        let width: f32 = bounds.size.width.into();
                        let height: f32 = bounds.size.height.into();
                        let cell_width = px(width / paint_columns);
                        let line_height = px(height / paint_rows);
                        let shape_context = TerminalShapeContext {
                            base_font: &base_font,
                            font_size,
                            cell_width,
                            focused,
                            cursor_visible,
                            window,
                        };
                        let lines = shape_snapshot_cached(
                            &mut render_cache
                                .lock()
                                .expect("terminal drag render cache poisoned"),
                            snapshot.clone(),
                            &shape_context,
                        );
                        let backgrounds = collect_background_runs(&snapshot);
                        let surface = snapshot_surface_color(&snapshot);
                        let cursor_bounds = snapshot
                            .cursor
                            .map(|cursor| cursor_bounds(bounds, cursor, cell_width, line_height));
                        let cursor = snapshot.cursor.and_then(|cursor| {
                            (!cursor.blinking || cursor_visible)
                                .then(|| {
                                    cursor_quad(bounds, cursor, cell_width, line_height, focused)
                                })
                                .flatten()
                        });
                        TerminalPaintState {
                            lines,
                            backgrounds,
                            grid_bounds: bounds,
                            surface,
                            cursor,
                            cursor_bounds,
                            composition: None,
                            cursor_blinking: snapshot.cursor.is_some_and(|cursor| cursor.blinking),
                            cell_width,
                            line_height,
                            grid_size: (snapshot.columns, snapshot.rows),
                        }
                    },
                    move |bounds, state, window, cx| {
                        // Match the terminal canvas paint order, but keep this copy
                        // read-only so dragging never affects the live pane.
                        state.paint_backgrounds(bounds, true, window);
                        if let Some(cursor) = state.cursor {
                            window.paint_quad(cursor);
                        }
                        for (row, line) in state.lines.iter().enumerate() {
                            let origin =
                                point(bounds.left(), bounds.top() + state.line_height * row);
                            let _ = line.paint(origin, state.line_height, window, cx);
                        }
                    },
                )
                .size_full(),
            )
    }
}

fn shape_snapshot_cached(
    cache: &mut TerminalRenderCache,
    snapshot: Arc<TerminalSnapshot>,
    context: &TerminalShapeContext<'_>,
) -> Vec<ShapedLine> {
    let font_size_f32: f32 = context.font_size.into();
    let cell_width_f32: f32 = context.cell_width.into();
    let theme_generation = theme::generation();
    let full_rebuild = cache.lines.len() != snapshot.rows
        || cache.font_size != font_size_f32
        || cache.cell_width != cell_width_f32
        || cache.theme_generation != theme_generation;
    if full_rebuild {
        cache.lines.clear();
        cache.lines.reserve(snapshot.rows);
        for row in 0..snapshot.rows {
            cache
                .lines
                .push(shape_snapshot_line(&snapshot, row, context));
        }
    } else {
        for row in 0..snapshot.rows {
            let cells_changed = cache.snapshot.as_ref().is_none_or(|previous| {
                previous
                    .lines
                    .get(row)
                    .zip(snapshot.lines.get(row))
                    .is_none_or(|(previous, current)| previous.as_ref() != current.as_ref())
            });
            let cursor_row_changed = cache.snapshot.as_ref().is_some_and(|previous| {
                previous.cursor.map(|cursor| cursor.row) != snapshot.cursor.map(|cursor| cursor.row)
                    && (previous.cursor.is_some_and(|cursor| cursor.row == row)
                        || snapshot.cursor.is_some_and(|cursor| cursor.row == row))
            });
            let cursor_style_changed = (cache.focused != context.focused
                || cache.cursor_visible != context.cursor_visible)
                && snapshot.cursor.is_some_and(|cursor| cursor.row == row);
            if cells_changed || cursor_row_changed || cursor_style_changed {
                cache.lines[row] = shape_snapshot_line(&snapshot, row, context);
            }
        }
    }

    cache.snapshot = Some(snapshot);
    cache.font_size = font_size_f32;
    cache.cell_width = cell_width_f32;
    cache.focused = context.focused;
    cache.cursor_visible = context.cursor_visible;
    cache.theme_generation = theme_generation;
    cache.lines.clone()
}

fn shape_snapshot_line(
    snapshot: &TerminalSnapshot,
    row: usize,
    context: &TerminalShapeContext<'_>,
) -> ShapedLine {
    let cells = snapshot.lines.get(row).map(AsRef::as_ref).unwrap_or(&[]);
    let mut text = String::with_capacity(snapshot.columns);
    let mut runs = Vec::with_capacity(cells.len());
    for cell in cells {
        let piece = display_text(cell);
        let mut font = context.base_font.clone();
        if cell.bold {
            font.weight = gpui::FontWeight::MEDIUM;
        }
        if cell.italic {
            font = font.italic();
        }
        let cursor_block = context.focused
            && context.cursor_visible
            && snapshot.cursor.is_some_and(|cursor| {
                cursor.row == cell.row
                    && cursor.column == cell.column
                    && cursor.shape == TerminalCursorShape::Block
                    && (!cursor.blinking || context.cursor_visible)
            });
        let foreground = if cursor_block {
            cell.background
        } else if cell.selected {
            theme::to_terminal_rgb(colors().foreground)
        } else {
            cell.foreground
        };
        let underline = match cell.underline {
            TerminalUnderline::None => None,
            TerminalUnderline::Single | TerminalUnderline::Dotted | TerminalUnderline::Dashed => {
                Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(to_hsla(cell.underline_color)),
                    wavy: false,
                })
            }
            TerminalUnderline::Double => Some(UnderlineStyle {
                thickness: px(2.0),
                color: Some(to_hsla(cell.underline_color)),
                wavy: false,
            }),
            TerminalUnderline::Curly => Some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(to_hsla(cell.underline_color)),
                wavy: true,
            }),
        };
        // Backgrounds are painted as per-cell quads; keep runs transparent so
        // glyph advances never leave a background gap at the pane edge.
        runs.push(TextRun {
            len: piece.len(),
            font,
            color: to_hsla(foreground),
            background_color: None,
            underline,
            strikethrough: cell.strikeout.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(to_hsla(foreground)),
            }),
        });
        text.push_str(piece);
    }
    context.window.text_system().shape_line(
        text.into(),
        context.font_size,
        &runs,
        Some(context.cell_width),
    )
}

fn collect_background_runs(snapshot: &TerminalSnapshot) -> Vec<TerminalBackgroundRun> {
    let fallback = snapshot_surface_color(snapshot);
    let selection = colors().selection.into();
    let columns = snapshot.columns.max(1);
    let mut backgrounds: Vec<TerminalBackgroundRun> = Vec::new();
    for row in 0..snapshot.rows.max(1) {
        let line = snapshot.lines.get(row);
        for column in 0..columns {
            let color = line
                .and_then(|line| line.get(column))
                .map_or(fallback, |cell| {
                    if cell.selected {
                        selection
                    } else {
                        to_hsla(cell.background)
                    }
                });
            if let Some(previous) = backgrounds.last_mut()
                && previous.row == row
                && previous.color == color
            {
                previous.columns.end = column + 1;
            } else {
                backgrounds.push(TerminalBackgroundRun {
                    row,
                    columns: column..column + 1,
                    color,
                });
            }
        }
    }
    backgrounds
}

/// Match padding and missing cells to the live TUI without changing shell colors.
fn snapshot_surface_color(snapshot: &TerminalSnapshot) -> Hsla {
    let sample = snapshot
        .lines
        .last()
        .and_then(|line| line.first())
        .or_else(|| snapshot.lines.first().and_then(|line| line.first()))
        .map(|cell| cell.background);
    sample
        .map(to_hsla)
        .unwrap_or_else(|| colors().terminal.into())
}

fn display_text(cell: &TerminalCell) -> &str {
    if cell.hidden || cell.wide_spacer || cell.text().contains('\n') {
        " "
    } else {
        cell.text()
    }
}

fn cursor_bounds(
    bounds: Bounds<Pixels>,
    cursor: TerminalCursor,
    cell_width: Pixels,
    line_height: Pixels,
) -> Bounds<Pixels> {
    Bounds::new(
        point(
            bounds.left() + cell_width * cursor.column,
            bounds.top() + line_height * cursor.row,
        ),
        size(cell_width, line_height),
    )
}

fn cursor_quad(
    bounds: Bounds<Pixels>,
    cursor: TerminalCursor,
    cell_width: Pixels,
    line_height: Pixels,
    focused: bool,
) -> Option<PaintQuad> {
    if cursor.shape == TerminalCursorShape::Hidden {
        return None;
    }
    let bounds = cursor_bounds(bounds, cursor, cell_width, line_height);
    let color: Hsla = if focused {
        colors().foreground.into()
    } else {
        colors().subtle.into()
    };
    if !focused && cursor.shape == TerminalCursorShape::Block {
        return Some(outline(bounds, color, Default::default()));
    }
    Some(match cursor.shape {
        TerminalCursorShape::Block => fill(bounds, color),
        TerminalCursorShape::HollowBlock => outline(bounds, color, Default::default()),
        TerminalCursorShape::Underline => fill(
            Bounds::new(
                point(bounds.left(), bounds.bottom() - px(2.0)),
                size(bounds.size.width, px(2.0)),
            ),
            color,
        ),
        TerminalCursorShape::Beam => fill(
            Bounds::new(bounds.origin, size(px(2.0), bounds.size.height)),
            color,
        ),
        TerminalCursorShape::Hidden => return None,
    })
}

fn to_hsla(color: TerminalRgb) -> Hsla {
    rgba(
        (u32::from(color.red) << 24)
            | (u32::from(color.green) << 16)
            | (u32::from(color.blue) << 8)
            | 0xff,
    )
    .into()
}

fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let filtered = text
        .replace("\x1b[201~", "")
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\r' | '\n' | '\t'))
        .collect::<String>();
    if bracketed {
        let mut bytes = Vec::with_capacity(filtered.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(filtered.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        filtered
            .replace("\r\n", "\r")
            .replace('\n', "\r")
            .into_bytes()
    }
}

fn paste_requires_confirmation(text: &str) -> bool {
    text.contains(['\r', '\n'])
        || text
            .chars()
            .any(|character| character.is_control() && character != '\t')
}

fn clipboard_has_image(item: &ClipboardItem) -> bool {
    item.entries()
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::Image(_)))
}

#[cfg(test)]
fn key_bytes(key: &Keystroke, mode: TerminalInputMode) -> Option<Vec<u8>> {
    key_event_bytes(key, mode, TerminalKeyEventType::Press)
}
fn key_event_bytes(
    key: &Keystroke,
    mode: TerminalInputMode,
    event: TerminalKeyEventType,
) -> Option<Vec<u8>> {
    crate::infrastructure::terminal_keyboard::key_event_bytes(&terminal_keystroke(key), mode, event)
}

fn terminal_keystroke(key: &Keystroke) -> TerminalKeystroke {
    TerminalKeystroke {
        key: key.key.clone(),
        key_char: key.key_char.clone(),
        modifiers: TerminalModifiers {
            shift: key.modifiers.shift,
            alt: key.modifiers.alt,
            control: key.modifiers.control,
            platform: key.modifiers.platform,
        },
    }
}

fn is_terminal_special_key(key: &str) -> bool {
    matches!(
        key,
        "enter"
            | "return"
            | "tab"
            | "backspace"
            | "escape"
            | "esc"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "insert"
            | "delete"
            | "pageup"
            | "page-up"
            | "pagedown"
            | "page-down"
    ) || key
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=20).contains(&number))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseReportState {
    Pressed,
    Released,
}

fn mouse_button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

fn mouse_report_bytes(
    point: TerminalPoint,
    button: u8,
    state: MouseReportState,
    modifiers: Modifiers,
    mode: TerminalInputMode,
) -> Option<Vec<u8>> {
    let modifier_code =
        modifiers.shift as u8 * 4 + modifiers.alt as u8 * 8 + modifiers.control as u8 * 16;
    let button = button + modifier_code;
    if mode.sgr_mouse {
        let terminator = if state == MouseReportState::Pressed {
            'M'
        } else {
            'm'
        };
        return Some(
            format!(
                "\x1b[<{button};{};{}{terminator}",
                point.column + 1,
                point.row + 1
            )
            .into_bytes(),
        );
    }

    let button = if state == MouseReportState::Released {
        3 + modifier_code
    } else {
        button
    };
    let max_coordinate = if mode.utf8_mouse { 2015 } else { 223 };
    if point.column >= max_coordinate || point.row >= max_coordinate {
        return None;
    }
    let mut bytes = vec![0x1b, b'[', b'M', 32 + button];
    encode_mouse_coordinate(&mut bytes, point.column, mode.utf8_mouse);
    encode_mouse_coordinate(&mut bytes, point.row, mode.utf8_mouse);
    Some(bytes)
}

fn encode_mouse_coordinate(bytes: &mut Vec<u8>, coordinate: usize, utf8: bool) {
    let encoded = 33 + coordinate;
    if utf8 && coordinate >= 95 {
        bytes.push((0xc0 + encoded / 64) as u8);
        bytes.push((0x80 + (encoded & 63)) as u8);
    } else {
        bytes.push(encoded as u8);
    }
}

#[cfg(test)]
mod tests;
