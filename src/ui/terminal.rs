use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, MouseButton, Pixels,
    SharedString, StrikethroughStyle, Subscription, Task, Timer, Window, prelude::*, px,
};
use uuid::Uuid;

use crate::domain::agents::AgentKind;
use crate::infrastructure::settings::{MAX_TERMINAL_FONT_SIZE, MIN_TERMINAL_FONT_SIZE};
use crate::ports::terminal::{
    TerminalAgentPresence, TerminalCell, TerminalCellSide, TerminalCursor, TerminalCursorShape,
    TerminalEvent, TerminalHandle, TerminalInputMode, TerminalPoint, TerminalPort, TerminalRgb,
    TerminalSearchDirection, TerminalSelectionType, TerminalSize, TerminalSnapshot,
    TerminalUnderline, is_safe_hyperlink,
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
mod input;
mod mouse;
mod render;
use agent_presence::{detect_agent_presence, is_interactive_shell_process_name};
#[cfg(test)]
pub(crate) use input::{
    clipboard_has_image, key_bytes, key_event_bytes, paste_bytes, paste_requires_confirmation,
};
#[cfg(test)]
pub(crate) use mouse::{MouseReportState, mouse_report_bytes};
pub(crate) use render::TerminalRenderCache;

const TERMINAL_FONT_SIZE: f32 = 12.0;
const TERMINAL_LINE_HEIGHT: f32 = 16.0;
/// Keeps the terminal grid comfortably inset from the panel edges.
const TERMINAL_HORIZONTAL_PADDING: f32 = 12.0;
const TERMINAL_VERTICAL_PADDING: f32 = 4.0;
/// Match `PANEL_RADIUS` so the canvas fill doesn't square off card corners.
const SURFACE_CORNER_RADIUS: f32 = 10.0;

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
                format!("Paste {line_count} lines into the terminal?")
            }
            Self::ClipboardRead { .. } => "Allow the terminal to read the clipboard?".to_owned(),
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
            preview.push_str("(empty)");
        }
        preview
    }

    fn hint(&self) -> &'static str {
        match self {
            Self::Paste { .. } => "↵ paste    esc cancel",
            Self::ClipboardRead { .. } => "↵ allow    esc deny",
        }
    }

    fn warning(&self) -> Option<&'static str> {
        matches!(self, Self::ClipboardRead { .. }).then_some(
            "The active process will receive the content shown. Only allow this if you trust it.",
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
    failed: bool,
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
                    "Could not open {}: {error:#}",
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
            failed: false,
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
        if self.failed || self.exited {
            return;
        }
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
        if self.failed || self.exited {
            return;
        }
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
        if self.failed && !matches!(event, TerminalEvent::Exit(_)) {
            return;
        }
        match event {
            TerminalEvent::Failed(message) => {
                self.failed = true;
                self.error = Some(format!("Terminal failed: {message}").into());
                self.input_error = None;
                self._input_error_task = None;
                self.marked_text.clear();
                self.pressed_key_foregrounds.clear();
                self.search_pending = false;
                self.search_generation = self.search_generation.wrapping_add(1);
                self._search_task = None;
                self.reject_pending_external_pastes(cx);
                if self.agent_presence.take().is_some() {
                    cx.emit(TerminalViewEvent::AgentPresenceChanged {
                        session_id: self.session_id,
                        presence: None,
                    });
                }
                cx.notify();
            }
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
                    self.reject_pending_external_pastes(cx);
                    cx.emit(TerminalViewEvent::Exited {
                        session_id: self.session_id,
                        code,
                    });
                    cx.notify();
                }
            }
        }
    }

    fn reject_pending_external_pastes(&mut self, cx: &mut Context<Self>) {
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
    }

    fn install_focus_observers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self._focus_subscriptions.is_empty() {
            return;
        }
        let focus_handle = self.focus_handle.clone();
        let focus = cx.on_focus(&focus_handle, window, |this, _, cx| {
            this.terminal_focused = true;
            this.reset_cursor_blink();
            if this.current_input_mode().focus_reporting {
                this.send_protocol(b"\x1b[I".to_vec(), cx);
            }
            cx.notify();
        });
        let blur = cx.on_blur(&focus_handle, window, |this, _, cx| {
            this.terminal_focused = false;
            this.cursor_visible = true;
            if this.current_input_mode().focus_reporting {
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
    pub(crate) fn thumbnail(mut self, max_width: f32, max_height: f32) -> Self {
        let scale = (max_width / self.width.max(1.0))
            .min(max_height / self.height.max(1.0))
            .min(1.0);
        self.width *= scale;
        self.height *= scale;
        self.font_size *= scale;
        self.cursor_visible = false;
        self
    }

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

#[cfg(test)]
mod tests;
