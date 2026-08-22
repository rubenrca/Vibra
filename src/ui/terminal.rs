use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, ElementInputHandler, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, Hsla, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels,
    Render, ShapedLine, SharedString, StrikethroughStyle, Subscription, Task, TextRun, Timer,
    UTF16Selection, UnderlineStyle, Window, canvas, div, fill, outline, point, prelude::*, px,
    rgba, size,
};
use uuid::Uuid;

use crate::ports::terminal::{
    TerminalAgentKindSource, TerminalAgentPresence, TerminalAgentState, TerminalCell,
    TerminalCellSide, TerminalCursor, TerminalCursorShape, TerminalEvent, TerminalHandle,
    TerminalInputMode, TerminalPoint, TerminalPort, TerminalRgb, TerminalSearchDirection,
    TerminalSelectionType, TerminalSize, TerminalSnapshot, TerminalUnderline,
};
use crate::ui::theme::colors;
use crate::{
    ClearTerminalScrollback, CopyTerminal, DecreaseTerminalFontSize, IncreaseTerminalFontSize,
    PasteTerminal, ResetTerminalFontSize, SearchTerminal, SearchTerminalNext,
    SearchTerminalPrevious,
};

const TERMINAL_FONT_SIZE: f32 = 12.0;
const TERMINAL_LINE_HEIGHT: f32 = 16.0;
const MIN_TERMINAL_FONT_SIZE: f32 = 8.0;
const MAX_TERMINAL_FONT_SIZE: f32 = 32.0;
const CURSOR_COLOR: TerminalRgb = TerminalRgb::new(0xe8, 0xe8, 0xe8);
const SELECTION_FOREGROUND: TerminalRgb = TerminalRgb::new(0xf4, 0xf4, 0xf5);
const SELECTION_BACKGROUND: TerminalRgb = TerminalRgb::new(0x3b, 0x3b, 0x43);
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);
/// Poll shell/foreground cwd often enough that `cd` feels live in the chrome.
const WORKING_DIRECTORY_POLL_INTERVAL: Duration = Duration::from_millis(500);
const BELL_FLASH_DURATION: Duration = Duration::from_millis(140);

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
    ContextMenuRequested {
        session_id: Uuid,
        x: f32,
        y: f32,
    },
}

#[derive(Clone)]
enum TerminalConfirmation {
    Paste(String),
    ClipboardRead {
        contents: String,
        formatter: Arc<dyn Fn(&str) -> String + Send + Sync>,
    },
}

impl TerminalConfirmation {
    fn title(&self) -> String {
        match self {
            Self::Paste(text) => {
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
            Self::Paste(text) => text,
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
            Self::Paste(_) => "↵ pegar    esc cancelar",
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
    scrollbar_dragging: bool,
    scrollbar_drag_offset: f32,
    hovered_hyperlink: Option<String>,
    search_active: bool,
    search_query: String,
    search_match_found: bool,
    pending_confirmations: VecDeque<TerminalConfirmation>,
    cursor_visible: bool,
    cursor_blinking: bool,
    terminal_focused: bool,
    bell_active: bool,
    agent_presence: Option<TerminalAgentPresence>,
    render_cache: Arc<Mutex<TerminalRenderCache>>,
    surface_visible: bool,
    _focus_subscriptions: Vec<Subscription>,
    _event_task: Option<Task<()>>,
    _cursor_task: Task<()>,
    _working_directory_task: Task<()>,
    _bell_task: Option<Task<()>>,
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
            scrollbar_dragging: false,
            scrollbar_drag_offset: 0.0,
            hovered_hyperlink: None,
            search_active: false,
            search_query: String::new(),
            search_match_found: false,
            pending_confirmations: VecDeque::new(),
            cursor_visible: true,
            cursor_blinking: false,
            terminal_focused: false,
            bell_active: false,
            agent_presence: None,
            render_cache: Arc::new(Mutex::new(TerminalRenderCache::default())),
            surface_visible: true,
            _focus_subscriptions: Vec::new(),
            _event_task: event_task,
            _cursor_task: cursor_task,
            _working_directory_task: working_directory_task,
            _bell_task: None,
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

    pub fn send_automation_input(&self, text: &str, newline: bool) -> Result<(), String> {
        let mut input = text.as_bytes().to_vec();
        if newline {
            input.push(b'\r');
        }
        self.send_automation_bytes(input)
    }

    /// Submit a prompt, honoring bracketed paste when the agent terminal enables it.
    pub fn send_automation_prompt(&self, text: &str) -> Result<(), String> {
        let bracketed = self
            .handle
            .as_ref()
            .map(|handle| handle.input_mode().bracketed_paste)
            .unwrap_or(false);
        let mut input = paste_bytes(text, bracketed);
        input.push(b'\r');
        self.send_automation_bytes(input)
    }

    fn send_automation_bytes(&self, input: Vec<u8>) -> Result<(), String> {
        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| "el terminal no está disponible".to_owned())?;
        handle.clear_selection();
        handle.scroll(i32::MIN);
        handle
            .send_input(input)
            .map_err(|error| format!("no se pudo escribir al terminal: {error:#}"))
    }

    pub fn foreground_process_name(&self) -> Option<String> {
        self.handle
            .as_ref()
            .and_then(|handle| handle.foreground_process_name())
    }

    pub fn foreground_process_id(&self) -> Option<u32> {
        self.handle
            .as_ref()
            .and_then(|handle| handle.foreground_process_id())
    }

    pub fn session_process_id(&self) -> Option<u32> {
        self.handle
            .as_ref()
            .and_then(|handle| handle.session_process_id())
    }

    pub fn is_interactive_shell(&self) -> bool {
        let Some(name) = self.foreground_process_name() else {
            // No foreground process reported yet — treat as not ready.
            return false;
        };
        let base = std::path::Path::new(&name)
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|| name.to_ascii_lowercase());
        let base = base.strip_prefix('-').unwrap_or(&base);
        is_interactive_shell_process_name(base)
    }

    pub fn automation_read_text(&self, lines: usize) -> String {
        let Some(handle) = self.handle.as_ref() else {
            return String::new();
        };
        if let Some(text) = handle.recent_text(lines) {
            return text;
        }
        let snapshot = handle.snapshot();
        let take = lines.max(1).min(snapshot.lines.len().max(1));
        let start = snapshot.lines.len().saturating_sub(take);
        let mut text = String::new();
        for line in &snapshot.lines[start..] {
            for cell in line.iter().filter(|cell| !cell.wide_spacer) {
                text.push_str(cell.text());
            }
            while text.ends_with(' ') {
                text.pop();
            }
            text.push('\n');
        }
        text
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

    fn handle_terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::Wakeup => {
                self.refresh_working_directory(cx);
                self.refresh_agent_presence(cx);
                if let Some(handle) = &self.handle {
                    handle.acknowledge_wakeup();
                }
                self.cursor_visible = true;
                cx.notify();
            }
            TerminalEvent::Title(title) => {
                if self.title != title {
                    self.title.clone_from(&title);
                    self.refresh_agent_presence(cx);
                    cx.emit(TerminalViewEvent::TitleChanged {
                        session_id: self.session_id,
                        title,
                    });
                    cx.notify();
                }
            }
            TerminalEvent::ResetTitle => {
                let title = "Terminal".to_owned();
                if self.title != title {
                    self.title.clone_from(&title);
                    self.refresh_agent_presence(cx);
                    cx.emit(TerminalViewEvent::TitleChanged {
                        session_id: self.session_id,
                        title,
                    });
                    cx.notify();
                }
            }
            TerminalEvent::ClipboardStore(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            TerminalEvent::ClipboardLoad(formatter) => {
                let contents = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
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
                    cx.emit(TerminalViewEvent::Exited {
                        session_id: self.session_id,
                        code,
                    });
                    cx.notify();
                }
            }
        }
    }

    fn send(&self, input: Vec<u8>) {
        if let Some(handle) = &self.handle {
            handle.clear_selection();
            handle.scroll(i32::MIN);
            let _ = handle.send_input(input);
        }
    }

    fn send_protocol(&self, input: Vec<u8>) {
        if let Some(handle) = &self.handle {
            let _ = handle.send_input(input);
        }
    }

    fn paste(&self, text: &str) {
        let mode = self
            .handle
            .as_ref()
            .map(|handle| handle.input_mode())
            .unwrap_or_default();
        self.send(paste_bytes(text, mode.bracketed_paste));
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
                .push_back(TerminalConfirmation::Paste(text));
            cx.notify();
        } else {
            self.paste(&text);
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
            self.send_protocol(vec![0x16]);
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
            .and_then(agent_kind_from_process_name)
            .is_some()
    }

    fn confirm_pending_action(&mut self, cx: &mut Context<Self>) {
        match self.pending_confirmations.pop_front() {
            Some(TerminalConfirmation::Paste(text)) => {
                self.paste(&text);
                self.reset_cursor_blink();
            }
            Some(TerminalConfirmation::ClipboardRead {
                contents,
                formatter,
            }) => self.send_protocol(formatter(&contents).into_bytes()),
            None => {}
        }
        cx.notify();
    }

    fn cancel_pending_action(&mut self, cx: &mut Context<Self>) {
        self.pending_confirmations.pop_front();
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
            self.refresh_search();
        }
        cx.notify();
    }

    fn close_search(&mut self, clear_selection: bool, cx: &mut Context<Self>) {
        self.search_active = false;
        self.marked_text.clear();
        if clear_selection && let Some(handle) = &self.handle {
            handle.clear_selection();
        }
        cx.notify();
    }

    fn search(&mut self, direction: TerminalSearchDirection) {
        self.search_match_found = self.handle.as_ref().is_some_and(|handle| {
            handle
                .search(&self.search_query, direction)
                .unwrap_or(false)
        });
    }

    fn refresh_search(&mut self) {
        if let Some(handle) = &self.handle {
            handle.clear_selection();
        }
        self.search(TerminalSearchDirection::Next);
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
        self.search(TerminalSearchDirection::Next);
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
        self.search(TerminalSearchDirection::Previous);
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
        self.send_protocol(vec![0x0c]);
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
                    self.search(direction);
                    cx.notify();
                }
                "backspace" => {
                    self.search_query.pop();
                    self.refresh_search();
                    cx.notify();
                }
                _ if keystroke.modifiers.platform && key == "g" && keystroke.modifiers.shift => {
                    self.search(TerminalSearchDirection::Previous);
                    cx.notify();
                }
                _ if keystroke.modifiers.platform && key == "g" => {
                    self.search(TerminalSearchDirection::Next);
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
                    self.send_protocol(vec![0x0c]);
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
        if let Some(input) = key_event_bytes(keystroke, mode, event_type) {
            self.send(input);
            self.reset_cursor_blink();
            cx.stop_propagation();
        }
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.search_active || event.keystroke.modifiers.platform {
            return;
        }
        let mode = self
            .handle
            .as_ref()
            .map(|handle| handle.input_mode())
            .unwrap_or_default();
        if let Some(input) = key_event_bytes(&event.keystroke, mode, TerminalKeyEventType::Release)
        {
            self.send(input);
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
                    self.send_protocol(bytes);
                }
            }
        } else if mode.alternate_screen && mode.alternate_scroll && !event.modifiers.shift {
            let sequence = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
            self.send_protocol(sequence.repeat(lines.unsigned_abs() as usize));
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
        self.reset_cursor_blink();
        if event.button == MouseButton::Left
            && let Some((track, thumb, snapshot)) = self.scrollbar_at(event.position)
        {
            let pointer_y: f32 = event.position.y.into();
            let thumb_top: f32 = thumb.top().into();
            let thumb_bottom: f32 = thumb.bottom().into();
            let thumb_height: f32 = thumb.size.height.into();
            self.scrollbar_dragging = true;
            self.scrollbar_drag_offset = if (thumb_top..=thumb_bottom).contains(&pointer_y) {
                pointer_y - thumb_top
            } else {
                thumb_height / 2.0
            };
            self.update_scrollbar_drag(event.position.y, track, thumb, &snapshot);
            cx.notify();
            cx.stop_propagation();
            return;
        }
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
                self.send_protocol(bytes);
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
        if event.button == MouseButton::Left && self.scrollbar_dragging {
            self.scrollbar_dragging = false;
            cx.notify();
            cx.stop_propagation();
            return;
        }
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
                self.send_protocol(bytes);
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
        if self.scrollbar_dragging {
            if let Some(handle) = &self.handle {
                let snapshot = handle.snapshot();
                if let Some((track, thumb)) =
                    scrollbar_metrics(self.last_terminal_bounds.unwrap_or_default(), &snapshot)
                {
                    self.update_scrollbar_drag(event.position.y, track, thumb, &snapshot);
                    cx.notify();
                    cx.stop_propagation();
                }
            }
            return;
        }
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
                    self.send_protocol(bytes);
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

    fn scrollbar_at(
        &self,
        position: gpui::Point<Pixels>,
    ) -> Option<(Bounds<Pixels>, Bounds<Pixels>, Arc<TerminalSnapshot>)> {
        let bounds = self.last_terminal_bounds?;
        let snapshot = self.handle.as_ref()?.snapshot();
        let (track, thumb) = scrollbar_metrics(bounds, &snapshot)?;
        track
            .contains(&position)
            .then_some((track, thumb, snapshot))
    }

    fn update_scrollbar_drag(
        &self,
        pointer_y: Pixels,
        track: Bounds<Pixels>,
        thumb: Bounds<Pixels>,
        snapshot: &TerminalSnapshot,
    ) {
        let available: f32 = (track.size.height - thumb.size.height).into();
        if available <= 0.0 || snapshot.history_size == 0 {
            return;
        }
        let pointer_y: f32 = pointer_y.into();
        let track_top: f32 = track.top().into();
        let thumb_top = (pointer_y - track_top - self.scrollbar_drag_offset).clamp(0.0, available);
        let progress = thumb_top / available;
        let target_offset = ((1.0 - progress) * snapshot.history_size as f32).round() as usize;
        let delta = target_offset as i64 - snapshot.display_offset as i64;
        if delta != 0
            && let Some(handle) = &self.handle
        {
            handle.scroll(delta.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
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
            let mode = this
                .handle
                .as_ref()
                .map(|handle| handle.input_mode())
                .unwrap_or_default();
            if mode.focus_reporting {
                this.send_protocol(b"\x1b[I".to_vec());
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
                this.send_protocol(b"\x1b[O".to_vec());
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
                self.refresh_search();
            } else if new_text.chars().count() > 1 {
                self.paste(new_text);
                self.reset_cursor_blink();
            } else {
                self.send(new_text.as_bytes().to_vec());
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
    /// Per-cell backgrounds in row-major order (`rows * columns`).
    /// Painted as quads so the grid always fills the pane (ShapedLine
    /// backgrounds can leave a gap; Warp-style emulators paint cells directly).
    cell_backgrounds: Vec<Hsla>,
    /// Full-pane underlay from the live surface color (not a forced theme black).
    surface: Hsla,
    cursor: Option<PaintQuad>,
    cursor_bounds: Option<Bounds<Pixels>>,
    composition: Option<ShapedLine>,
    scrollbar: Option<PaintQuad>,
    cursor_blinking: bool,
    cell_width: Pixels,
    line_height: Pixels,
    grid_size: (usize, usize),
}

#[derive(Default)]
struct TerminalRenderCache {
    snapshot: Option<Arc<TerminalSnapshot>>,
    lines: Vec<ShapedLine>,
    font_size: f32,
    cell_width: f32,
    focused: bool,
    cursor_visible: bool,
}

struct TerminalShapeContext<'a> {
    base_font: &'a gpui::Font,
    font_size: Pixels,
    cell_width: Pixels,
    focused: bool,
    cursor_visible: bool,
    window: &'a Window,
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.install_focus_observers(window, cx);
        let entity = cx.entity();
        let handle = self.handle.clone();
        let marked_text: SharedString = self.marked_text.clone().into();
        let canvas_marked_text = marked_text.clone();
        let error = self.error.clone();
        let exited = self.exited;
        let search_active = self.search_active;
        let search_query = self.search_query.clone();
        let search_match_found = self.search_match_found;
        let pending_confirmation = self.pending_confirmations.front().cloned();
        let cursor_visible = self.cursor_visible;
        let bell_active = self.bell_active;
        let hyperlink_hovered = self.hovered_hyperlink.is_some();
        let render_cache = self.render_cache.clone();
        let font_size = self.font_size;
        let line_height = font_size + (TERMINAL_LINE_HEIGHT - TERMINAL_FONT_SIZE);

        div()
            .id(SharedString::from(format!("terminal-{}", self.session_id)))
            .size_full()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
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
            .font_family("JetBrains Mono")
            .font_weight(gpui::FontWeight::LIGHT)
            .text_size(px(font_size))
            .line_height(px(line_height))
            .bg(colors().terminal)
            .border_1()
            .border_color(if bell_active {
                colors().danger
            } else {
                colors().terminal
            })
            .cursor(gpui::CursorStyle::IBeam)
            .when(hyperlink_hovered, |terminal| terminal.cursor_pointer())
            .child(
                canvas(
                    {
                        let entity = entity.clone();
                        move |bounds, window, cx| {
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
                                let _ = handle.resize(size);
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
                            let cell_backgrounds = collect_cell_backgrounds(&snapshot);
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
                                            color: to_hsla(CURSOR_COLOR),
                                            background_color: Some(to_hsla(TerminalRgb::new(
                                                0x1d, 0x1d, 0x1f,
                                            ))),
                                            underline: Some(UnderlineStyle {
                                                thickness: px(1.0),
                                                color: Some(to_hsla(CURSOR_COLOR)),
                                                wavy: false,
                                            }),
                                            strikethrough: None,
                                        }],
                                        Some(cell_width),
                                    )
                                });
                            let scrollbar = scrollbar_quad(bounds, &snapshot);
                            TerminalPaintState {
                                lines,
                                cell_backgrounds,
                                surface,
                                cursor,
                                cursor_bounds,
                                composition,
                                scrollbar,
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
                        window.handle_input(
                            &entity.read(cx).focus_handle,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        // 1) Full-pane underlay from live surface color (matches TUI canvas,
                        //    not a forced theme black — normal shells keep their bg).
                        window.paint_quad(fill(bounds, state.surface));
                        // 2) Exact per-cell backgrounds so the grid covers every pixel.
                        let columns = state.grid_size.0.max(1);
                        for (index, background) in state.cell_backgrounds.iter().enumerate() {
                            let row = index / columns;
                            let column = index % columns;
                            let cell_bounds = Bounds::new(
                                point(
                                    bounds.left() + state.cell_width * column,
                                    bounds.top() + state.line_height * row,
                                ),
                                size(state.cell_width, state.line_height),
                            );
                            if *background != state.surface {
                                window.paint_quad(fill(cell_bounds, *background));
                            }
                        }
                        if let Some(cursor) = state.cursor {
                            window.paint_quad(cursor);
                        }
                        for (row, line) in state.lines.iter().enumerate() {
                            let origin =
                                point(bounds.left(), bounds.top() + state.line_height * row);
                            let _ = line.paint(origin, state.line_height, window, cx);
                        }
                        if let Some(scrollbar) = state.scrollbar {
                            window.paint_quad(scrollbar);
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
                let status = if search_query.is_empty() {
                    "Escribe para buscar"
                } else if search_match_found {
                    "↵ siguiente  ⇧↵ anterior"
                } else {
                    "Sin resultados"
                };
                let composition = if marked_text.is_empty() {
                    String::new()
                } else {
                    format!("{marked_text}")
                };
                terminal.child(
                    div()
                        .absolute()
                        .top_3()
                        .right_3()
                        .min_w(px(260.0))
                        .max_w(px(420.0))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(colors().elevated)
                        .shadow_sm()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(colors().foreground)
                                .child(format!("Buscar  {search_query}{composition}")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(if search_match_found {
                                    colors().muted
                                } else {
                                    colors().danger
                                })
                                .child(status),
                        ),
                )
            })
            .when_some(pending_confirmation, |terminal, confirmation| {
                let title = confirmation.title();
                let preview = confirmation.preview();
                let hint = confirmation.hint();
                let warning = confirmation.warning();
                terminal.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x08080acc))
                        .child(
                            div()
                                .w(px(480.0))
                                .max_w_full()
                                .mx_4()
                                .p_4()
                                .rounded_lg()
                                .border_1()
                                .border_color(colors().border_subtle)
                                .bg(colors().elevated)
                                .shadow_lg()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(div().text_sm().text_color(colors().foreground).child(title))
                                .when_some(warning, |dialog, warning| {
                                    dialog.child(
                                        div().text_xs().text_color(colors().danger).child(warning),
                                    )
                                })
                                .child(
                                    div()
                                        .px_2()
                                        .py_2()
                                        .rounded_sm()
                                        .bg(colors().terminal)
                                        .text_xs()
                                        .text_color(colors().muted)
                                        .overflow_hidden()
                                        .child(preview),
                                )
                                .child(div().text_xs().text_color(colors().muted).child(hint)),
                        ),
                )
            })
            .when_some(error, |terminal, error| {
                terminal.child(
                    div()
                        .absolute()
                        .inset_0()
                        .p_5()
                        .font_family("JetBrains Mono")
                        .text_sm()
                        .text_color(colors().danger)
                        .child(error),
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
                        .bg(colors().elevated)
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
            .font_family("JetBrains Mono")
            .font_weight(gpui::FontWeight::LIGHT)
            .text_size(px(font_size))
            .line_height(px(line_height))
            .bg(colors().terminal)
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
                        let cell_backgrounds = collect_cell_backgrounds(&snapshot);
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
                            cell_backgrounds,
                            surface,
                            cursor,
                            cursor_bounds,
                            composition: None,
                            scrollbar: scrollbar_quad(bounds, &snapshot),
                            cursor_blinking: snapshot.cursor.is_some_and(|cursor| cursor.blinking),
                            cell_width,
                            line_height,
                            grid_size: (snapshot.columns, snapshot.rows),
                        }
                    },
                    move |bounds, state, window, cx| {
                        // Match the terminal canvas paint order, but keep this copy
                        // read-only so dragging never affects the live pane.
                        window.paint_quad(fill(bounds, state.surface));
                        let columns = state.grid_size.0.max(1);
                        for (index, background) in state.cell_backgrounds.iter().enumerate() {
                            let row = index / columns;
                            let column = index % columns;
                            let cell_bounds = Bounds::new(
                                point(
                                    bounds.left() + state.cell_width * column,
                                    bounds.top() + state.line_height * row,
                                ),
                                size(state.cell_width, state.line_height),
                            );
                            if *background != state.surface {
                                window.paint_quad(fill(cell_bounds, *background));
                            }
                        }
                        if let Some(cursor) = state.cursor {
                            window.paint_quad(cursor);
                        }
                        for (row, line) in state.lines.iter().enumerate() {
                            let origin =
                                point(bounds.left(), bounds.top() + state.line_height * row);
                            let _ = line.paint(origin, state.line_height, window, cx);
                        }
                        if let Some(scrollbar) = state.scrollbar {
                            window.paint_quad(scrollbar);
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
    let full_rebuild = cache.lines.len() != snapshot.rows
        || cache.font_size != font_size_f32
        || cache.cell_width != cell_width_f32;
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
            SELECTION_FOREGROUND
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

fn collect_cell_backgrounds(snapshot: &TerminalSnapshot) -> Vec<Hsla> {
    let mut backgrounds = Vec::with_capacity(snapshot.rows.saturating_mul(snapshot.columns));
    for line in &snapshot.lines {
        for cell in line.iter() {
            let background = if cell.selected {
                SELECTION_BACKGROUND
            } else {
                cell.background
            };
            backgrounds.push(to_hsla(background));
        }
    }
    backgrounds
}

/// Live canvas color for full-pane underlay. Taken from the TUI itself so Grok’s
/// black fills the pane without forcing the shell theme to pure black.
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

fn scrollbar_quad(bounds: Bounds<Pixels>, snapshot: &TerminalSnapshot) -> Option<PaintQuad> {
    let (_, thumb) = scrollbar_metrics(bounds, snapshot)?;
    Some(fill(thumb, rgba(0x77777f88)))
}

fn scrollbar_metrics(
    bounds: Bounds<Pixels>,
    snapshot: &TerminalSnapshot,
) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
    if snapshot.history_size == 0 || bounds.size.height <= px(8.0) {
        return None;
    }

    let track = Bounds::new(
        point(bounds.right() - px(12.0), bounds.top() + px(4.0)),
        size(px(12.0), bounds.size.height - px(8.0)),
    );
    let track_height: f32 = track.size.height.into();
    let total_lines = snapshot.history_size + snapshot.rows;
    let thumb_height = (track_height * snapshot.rows as f32 / total_lines as f32)
        .max(24.0)
        .min(track_height);
    let progress = 1.0 - snapshot.display_offset as f32 / snapshot.history_size as f32;
    let top = track.top() + px((track_height - thumb_height) * progress);
    let thumb = Bounds::new(
        point(bounds.right() - px(4.0), top),
        size(px(3.0), px(thumb_height)),
    );
    Some((track, thumb))
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
    let color = if focused {
        to_hsla(CURSOR_COLOR)
    } else {
        to_hsla(TerminalRgb::new(0x6f, 0x6f, 0x75))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalKeyEventType {
    Press,
    Repeat,
    Release,
}

#[cfg(test)]
fn key_bytes(keystroke: &Keystroke, mode: TerminalInputMode) -> Option<Vec<u8>> {
    key_event_bytes(keystroke, mode, TerminalKeyEventType::Press)
}

fn key_event_bytes(
    keystroke: &Keystroke,
    mode: TerminalInputMode,
    event_type: TerminalKeyEventType,
) -> Option<Vec<u8>> {
    if event_type == TerminalKeyEventType::Release && !mode.report_event_types {
        return None;
    }

    let key = keystroke.key.to_ascii_lowercase();
    let kitty_control_code = match key.as_str() {
        "tab" => Some(9),
        "enter" | "return" => Some(13),
        "escape" | "esc" => Some(27),
        "space" => Some(32),
        "backspace" => Some(127),
        _ => None,
    };
    let modifiers = keystroke.modifiers;
    if let Some(codepoint) = kitty_control_code
        && (mode.report_all_keys_as_escape_codes
            || (mode.disambiguate_escape_codes
                && (modifiers.modified()
                    || matches!(
                        key.as_str(),
                        "tab" | "enter" | "return" | "escape" | "esc" | "backspace"
                    ))))
    {
        return Some(kitty_unicode_sequence(
            codepoint, None, modifiers, event_type, mode, None,
        ));
    }

    if let Some((base, terminator, application_sequence)) = named_key_sequence(&key) {
        let has_modifiers = modifiers.shift || modifiers.alt || modifiers.control;
        let kitty_event = mode.report_event_types && event_type != TerminalKeyEventType::Press;
        if has_modifiers || kitty_event {
            let base = if base.is_empty() { "1" } else { base };
            let mut sequence = format!("\x1b[{base};{}", modifier_parameter(modifiers));
            if kitty_event {
                sequence.push(':');
                sequence.push(key_event_code(event_type));
            }
            sequence.push(terminator);
            return Some(sequence.into_bytes());
        }

        if event_type == TerminalKeyEventType::Release {
            return None;
        }
        if application_sequence && mode.application_cursor {
            return Some(format!("\x1bO{terminator}").into_bytes());
        }
        if matches!(key.as_str(), "f1" | "f2" | "f3" | "f4") {
            return Some(format!("\x1bO{terminator}").into_bytes());
        }
        return Some(format!("\x1b[{base}{terminator}").into_bytes());
    }

    if event_type == TerminalKeyEventType::Release {
        return kitty_text_sequence(keystroke, mode, event_type);
    }

    match key.as_str() {
        "enter" | "return" => {
            return Some(prefixed_control_byte(b'\r', modifiers.alt));
        }
        "tab" if modifiers.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return Some(prefixed_control_byte(b'\t', modifiers.alt)),
        "backspace" => return Some(prefixed_control_byte(0x7f, modifiers.alt)),
        "escape" | "esc" => return Some(prefixed_control_byte(0x1b, modifiers.alt)),
        _ => {}
    }

    if mode.kitty_keyboard()
        && (mode.report_all_keys_as_escape_codes
            || (mode.disambiguate_escape_codes && (modifiers.control || modifiers.alt)))
    {
        return kitty_text_sequence(keystroke, mode, event_type);
    }

    if modifiers.control {
        let character = key.chars().next()?;
        let control = match character {
            'a'..='z' => character as u8 - b'a' + 1,
            '@' | ' ' | '2' => 0,
            '[' | '3' => 27,
            '\\' | '4' => 28,
            ']' | '5' => 29,
            '^' | '6' => 30,
            '_' | '7' | '/' => 31,
            '8' | '?' => 127,
            _ => return None,
        };
        let mut bytes = Vec::with_capacity(2);
        if modifiers.alt {
            bytes.push(0x1b);
        }
        bytes.push(control);
        return Some(bytes);
    }

    if modifiers.alt {
        let text = keystroke.key_char.as_deref().unwrap_or(&keystroke.key);
        let mut bytes = Vec::with_capacity(text.len() + 1);
        bytes.push(0x1b);
        bytes.extend_from_slice(text.as_bytes());
        return Some(bytes);
    }

    None
}

fn named_key_sequence(key: &str) -> Option<(&'static str, char, bool)> {
    let sequence = match key {
        "up" => ("", 'A', true),
        "down" => ("", 'B', true),
        "right" => ("", 'C', true),
        "left" => ("", 'D', true),
        "home" => ("", 'H', false),
        "end" => ("", 'F', false),
        "insert" => ("2", '~', false),
        "delete" => ("3", '~', false),
        "pageup" | "page-up" => ("5", '~', false),
        "pagedown" | "page-down" => ("6", '~', false),
        "f1" => ("", 'P', false),
        "f2" => ("", 'Q', false),
        "f3" => ("", 'R', false),
        "f4" => ("", 'S', false),
        "f5" => ("15", '~', false),
        "f6" => ("17", '~', false),
        "f7" => ("18", '~', false),
        "f8" => ("19", '~', false),
        "f9" => ("20", '~', false),
        "f10" => ("21", '~', false),
        "f11" => ("23", '~', false),
        "f12" => ("24", '~', false),
        "f13" => ("25", '~', false),
        "f14" => ("26", '~', false),
        "f15" => ("28", '~', false),
        "f16" => ("29", '~', false),
        "f17" => ("31", '~', false),
        "f18" => ("32", '~', false),
        "f19" => ("33", '~', false),
        "f20" => ("34", '~', false),
        _ => return None,
    };
    Some(sequence)
}

fn kitty_text_sequence(
    keystroke: &Keystroke,
    mode: TerminalInputMode,
    event_type: TerminalKeyEventType,
) -> Option<Vec<u8>> {
    let base_character = keystroke.key.chars().next()?;
    let alternate_character = keystroke.key_char.as_deref().and_then(|text| {
        (text.chars().count() == 1)
            .then(|| text.chars().next())
            .flatten()
    });
    let alternate = mode
        .report_alternate_keys
        .then_some(alternate_character)
        .flatten()
        .filter(|alternate| *alternate != base_character)
        .map(u32::from);
    let associated_text = mode
        .report_associated_text
        .then_some(keystroke.key_char.as_deref())
        .flatten()
        .filter(|text| !text.is_empty());
    Some(kitty_unicode_sequence(
        u32::from(base_character),
        alternate,
        keystroke.modifiers,
        event_type,
        mode,
        associated_text,
    ))
}

fn kitty_unicode_sequence(
    codepoint: u32,
    alternate: Option<u32>,
    modifiers: Modifiers,
    event_type: TerminalKeyEventType,
    mode: TerminalInputMode,
    associated_text: Option<&str>,
) -> Vec<u8> {
    let mut sequence = format!("\x1b[{codepoint}");
    if let Some(alternate) = alternate {
        sequence.push(':');
        sequence.push_str(&alternate.to_string());
    }
    let include_event = mode.report_event_types && event_type != TerminalKeyEventType::Press;
    if modifiers.modified() || include_event || associated_text.is_some() {
        sequence.push(';');
        sequence.push_str(&modifier_parameter(modifiers).to_string());
    }
    if include_event {
        sequence.push(':');
        sequence.push(key_event_code(event_type));
    }
    if let Some(text) = associated_text {
        sequence.push(';');
        let mut codepoints = text.chars().map(u32::from);
        if let Some(codepoint) = codepoints.next() {
            sequence.push_str(&codepoint.to_string());
            for codepoint in codepoints {
                sequence.push(':');
                sequence.push_str(&codepoint.to_string());
            }
        }
    }
    sequence.push('u');
    sequence.into_bytes()
}

fn modifier_parameter(modifiers: Modifiers) -> u8 {
    1 + modifiers.shift as u8
        + (modifiers.alt as u8 * 2)
        + (modifiers.control as u8 * 4)
        + (modifiers.platform as u8 * 8)
}

fn key_event_code(event_type: TerminalKeyEventType) -> char {
    match event_type {
        TerminalKeyEventType::Press => '1',
        TerminalKeyEventType::Repeat => '2',
        TerminalKeyEventType::Release => '3',
    }
}

fn prefixed_control_byte(byte: u8, alt: bool) -> Vec<u8> {
    if alt { vec![0x1b, byte] } else { vec![byte] }
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

fn is_safe_hyperlink(uri: &str) -> bool {
    ["https://", "http://", "mailto:", "file://"]
        .iter()
        .any(|scheme| uri.starts_with(scheme))
}

fn detect_agent_presence(
    title: &str,
    snapshot: &TerminalSnapshot,
    recent_text: Option<&str>,
    process_name: Option<&str>,
    process_id: Option<u32>,
) -> Option<TerminalAgentPresence> {
    let screen = recent_text
        .map(str::to_lowercase)
        .unwrap_or_else(|| visible_screen_text(snapshot));
    // Identity and activity deliberately use different evidence. Process names
    // are reliable at startup, titles are the next-best structured signal, and
    // screen text remains a fallback for wrappers and unsupported terminals.
    // Once a known shell owns the TTY again, old agent titles and scrollback
    // must not keep the pane looking live or make aliases target the shell.
    if process_name.is_some_and(is_interactive_shell_process_name) {
        return None;
    }
    let (kind, kind_source) = process_name
        .and_then(agent_kind_from_process_name)
        .map(|kind| (kind, TerminalAgentKindSource::Process))
        .or_else(|| agent_kind_from_text(title).map(|kind| (kind, TerminalAgentKindSource::Title)))
        .or_else(|| {
            agent_kind_from_text(&screen).map(|kind| (kind, TerminalAgentKindSource::Screen))
        })?;
    let state = agent_state_from_text(title, &screen);
    Some(TerminalAgentPresence {
        kind: kind.to_owned(),
        kind_source,
        state,
        process_id: (kind_source == TerminalAgentKindSource::Process)
            .then_some(process_id)
            .flatten(),
    })
}

fn is_interactive_shell_process_name(process_name: &str) -> bool {
    let base = std::path::Path::new(process_name)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| process_name.to_ascii_lowercase());
    let base = base.strip_prefix('-').unwrap_or(&base);
    matches!(
        base,
        "sh" | "bash" | "zsh" | "fish" | "nu" | "pwsh" | "powershell" | "cmd" | "dash" | "ksh"
    )
}

fn visible_screen_text(snapshot: &TerminalSnapshot) -> String {
    let start = snapshot.lines.len().saturating_sub(14);
    let mut text = String::new();
    for line in &snapshot.lines[start..] {
        for cell in line.iter().filter(|cell| !cell.wide_spacer) {
            text.push_str(&cell.text().to_lowercase());
        }
        text.push('\n');
    }
    text
}

fn agent_kind_from_process_name(process_name: &str) -> Option<&'static str> {
    let base = std::path::Path::new(process_name)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| process_name.to_ascii_lowercase());
    const AGENT_PROCESSES: [(&str, &str); 10] = [
        ("opencode", "OpenCode"),
        ("claude", "Claude"),
        ("codex", "Codex"),
        ("gemini", "Gemini"),
        ("goose", "Goose"),
        ("grok", "Grok"),
        ("aider", "Aider"),
        ("amp", "Amp"),
        ("pi", "Pi"),
        ("cursor-agent", "Cursor"),
    ];
    AGENT_PROCESSES
        .iter()
        .find(|(name, _)| process_name_matches(&base, name))
        .map(|(_, kind)| *kind)
}

fn process_name_matches(process_name: &str, agent_name: &str) -> bool {
    process_name == agent_name
        || process_name
            .strip_prefix(agent_name)
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(|separator| matches!(separator, '-' | '_' | '.'))
}

fn agent_kind_from_text(text: &str) -> Option<&'static str> {
    let text = text.to_lowercase();
    let agents: [(&str, &[&str]); 10] = [
        ("OpenCode", &["opencode"]),
        ("Claude", &["claude code", "claude"]),
        ("Codex", &["openai codex", "codex"]),
        ("Gemini", &["gemini cli", "gemini"]),
        ("Goose", &["goose session", "block goose", "goose"]),
        ("Grok", &["grok cli", "grok"]),
        ("Cursor", &["cursor agent"]),
        ("Aider", &["aider"]),
        ("Amp", &["sourcegraph amp", "amp thread"]),
        ("Pi", &["pi coding agent", "pi agent"]),
    ];
    agents
        .iter()
        .find(|(_, markers)| markers.iter().any(|marker| text.contains(marker)))
        .map(|(kind, _)| *kind)
}

fn agent_state_from_text(title: &str, screen: &str) -> TerminalAgentState {
    let mut visible = title.to_lowercase();
    visible.push('\n');
    visible.push_str(screen);
    let waiting_markers = [
        "allow?",
        "approve",
        "do you want to continue",
        "press enter to confirm",
        "waiting for input",
        "(y/n)",
        "[y/n]",
        "permission required",
        "permission requested",
    ];
    let working_markers = [
        "esc to interrupt",
        "ctrl+c to interrupt",
        "thinking",
        "generating response",
        "running tool",
        "running command",
    ];
    if waiting_markers
        .iter()
        .any(|marker| visible.contains(marker))
    {
        TerminalAgentState::Waiting
    } else if working_markers
        .iter()
        .any(|marker| visible.contains(marker))
    {
        TerminalAgentState::Working
    } else {
        TerminalAgentState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_channel::{Receiver, Sender};
    use gpui::{KeyBinding, Modifiers, TestAppContext};

    struct MockTerminalPort {
        handle: Arc<MockTerminalHandle>,
    }

    struct MockTerminalHandle {
        events: Receiver<TerminalEvent>,
        _events_tx: Sender<TerminalEvent>,
        inputs: Mutex<Vec<Vec<u8>>>,
    }

    impl MockTerminalPort {
        fn new() -> Self {
            let (events_tx, events) = async_channel::unbounded();
            Self {
                handle: Arc::new(MockTerminalHandle {
                    events,
                    _events_tx: events_tx,
                    inputs: Mutex::new(Vec::new()),
                }),
            }
        }
    }

    impl TerminalPort for MockTerminalPort {
        fn backend_name(&self) -> &'static str {
            "mock"
        }

        fn spawn(
            &self,
            _: Uuid,
            _: &Path,
            _: &std::collections::HashMap<String, String>,
        ) -> Result<Arc<dyn TerminalHandle>> {
            Ok(self.handle.clone())
        }
    }

    impl TerminalHandle for MockTerminalHandle {
        fn events(&self) -> Receiver<TerminalEvent> {
            self.events.clone()
        }

        fn send_input(&self, input: Vec<u8>) -> Result<()> {
            self.inputs.lock().unwrap().push(input);
            Ok(())
        }

        fn resize(&self, _: TerminalSize) -> Result<()> {
            Ok(())
        }

        fn scroll(&self, _: i32) {}

        fn clear_scrollback(&self) {}

        fn snapshot(&self) -> Arc<TerminalSnapshot> {
            Arc::new(TerminalSnapshot {
                columns: 80,
                rows: 24,
                lines: (0..24)
                    .map(|row| {
                        Arc::from(
                            (0..80)
                                .map(|column| {
                                    let mut cell = TerminalCell::blank(row, column);
                                    cell.foreground = TerminalRgb::new(0xe5, 0xe5, 0xe6);
                                    cell.background = TerminalRgb::new(0x10, 0x10, 0x11);
                                    cell.underline_color = TerminalRgb::new(0xe5, 0xe5, 0xe6);
                                    cell
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect(),
                cursor: None,
                display_offset: 0,
                history_size: 0,
            })
        }

        fn input_mode(&self) -> TerminalInputMode {
            TerminalInputMode::default()
        }

        fn clear_selection(&self) {}

        fn start_selection(&self, _: TerminalSelectionType, _: TerminalPoint, _: TerminalCellSide) {
        }

        fn update_selection(&self, _: TerminalPoint, _: TerminalCellSide) {}

        fn selection_text(&self) -> Option<String> {
            None
        }

        fn search(&self, _: &str, _: TerminalSearchDirection) -> Result<bool> {
            Ok(true)
        }

        fn hyperlink_at(&self, _: TerminalPoint) -> Option<String> {
            None
        }

        fn acknowledge_wakeup(&self) {}

        fn shutdown(&self) {}
    }

    fn key(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            key: key.into(),
            key_char: Some(key.into()),
            modifiers,
        }
    }

    #[gpui::test]
    fn terminal_key_context_dispatches_product_shortcuts(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([
                KeyBinding::new("cmd-f", SearchTerminal, Some("Terminal")),
                KeyBinding::new("cmd-=", IncreaseTerminalFontSize, Some("Terminal")),
            ]);
        });
        let port = Arc::new(MockTerminalPort::new());
        let mock_handle = port.handle.clone();
        let window = cx.update(|cx| {
            let port = port.clone();
            cx.open_window(Default::default(), |window, cx| {
                let terminal = cx.new(|cx| {
                    TerminalView::new_with_environment(
                        Uuid::new_v4(),
                        "Terminal".into(),
                        Path::new("/"),
                        port,
                        std::collections::HashMap::new(),
                        cx,
                    )
                });
                terminal.read(cx).focus_handle(cx).focus(window);
                terminal
            })
            .unwrap()
        });

        cx.dispatch_keystroke(*window, Keystroke::parse("cmd-f").unwrap());
        window
            .update(cx, |terminal, _, _| assert!(terminal.search_active))
            .unwrap();

        cx.dispatch_keystroke(*window, Keystroke::parse("escape").unwrap());
        cx.dispatch_keystroke(*window, Keystroke::parse("cmd-=").unwrap());
        window
            .update(cx, |terminal, _, _| {
                assert!(!terminal.search_active);
                assert_eq!(terminal.font_size, TERMINAL_FONT_SIZE + 1.0);
            })
            .unwrap();

        cx.simulate_input(*window, "x");
        assert_eq!(
            mock_handle.inputs.lock().unwrap().as_slice(),
            [b"x".to_vec()]
        );
    }

    #[gpui::test]
    fn osc52_clipboard_reads_wait_for_explicit_consent(cx: &mut TestAppContext) {
        cx.write_to_clipboard(ClipboardItem::new_string("token-super-secreto".into()));
        let port = Arc::new(MockTerminalPort::new());
        let mock_handle = port.handle.clone();
        let window = cx.update(|cx| {
            let port = port.clone();
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    TerminalView::new_with_environment(
                        Uuid::new_v4(),
                        "Terminal".into(),
                        Path::new("/"),
                        port,
                        std::collections::HashMap::new(),
                        cx,
                    )
                })
            })
            .unwrap()
        });

        window
            .update(cx, |terminal, _, cx| {
                terminal.handle_terminal_event(
                    TerminalEvent::ClipboardLoad(Arc::new(|text| format!("OSC52:{text}"))),
                    cx,
                );
                assert_eq!(terminal.pending_confirmations.len(), 1);
            })
            .unwrap();
        assert!(mock_handle.inputs.lock().unwrap().is_empty());

        window
            .update(cx, |terminal, _, cx| {
                terminal.confirm_pending_action(cx);
            })
            .unwrap();
        assert_eq!(
            mock_handle.inputs.lock().unwrap().as_slice(),
            [b"OSC52:token-super-secreto".to_vec()]
        );
    }

    #[gpui::test]
    fn denying_an_osc52_clipboard_read_sends_nothing(cx: &mut TestAppContext) {
        cx.write_to_clipboard(ClipboardItem::new_string("no-compartir".into()));
        let port = Arc::new(MockTerminalPort::new());
        let mock_handle = port.handle.clone();
        let window = cx.update(|cx| {
            let port = port.clone();
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    TerminalView::new_with_environment(
                        Uuid::new_v4(),
                        "Terminal".into(),
                        Path::new("/"),
                        port,
                        std::collections::HashMap::new(),
                        cx,
                    )
                })
            })
            .unwrap()
        });

        window
            .update(cx, |terminal, _, cx| {
                terminal.handle_terminal_event(
                    TerminalEvent::ClipboardLoad(Arc::new(|text| text.to_owned())),
                    cx,
                );
                terminal.cancel_pending_action(cx);
            })
            .unwrap();

        assert!(mock_handle.inputs.lock().unwrap().is_empty());
    }

    #[test]
    fn application_cursor_uses_ss3_sequences() {
        let bytes = key_bytes(
            &key("up", Modifiers::default()),
            TerminalInputMode {
                application_cursor: true,
                ..TerminalInputMode::default()
            },
        );
        assert_eq!(bytes.as_deref(), Some(b"\x1bOA".as_slice()));
    }

    #[test]
    fn control_letters_map_to_ascii_control_codes() {
        let bytes = key_bytes(
            &key(
                "c",
                Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
            ),
            TerminalInputMode::default(),
        );
        assert_eq!(bytes, Some(vec![3]));
    }

    #[test]
    fn screen_fallback_detects_supported_agents_and_waiting_state() {
        let snapshot = TerminalSnapshot {
            columns: 80,
            rows: 1,
            lines: vec![Arc::from([TerminalCell::with_text(
                0,
                0,
                "Claude Code — allow? [y/n]",
                TerminalRgb::new(255, 255, 255),
                TerminalRgb::new(0, 0, 0),
            )])],
            cursor: None,
            display_offset: 0,
            history_size: 0,
        };

        let presence = detect_agent_presence("Terminal", &snapshot, None, None, None).unwrap();

        assert_eq!(presence.kind, "Claude");
        assert_eq!(presence.state, TerminalAgentState::Waiting);

        let title_presence =
            detect_agent_presence("OpenAI Codex", &snapshot, None, None, None).unwrap();
        assert_eq!(title_presence.kind, "Codex");
        assert_eq!(title_presence.state, TerminalAgentState::Waiting);
    }

    #[test]
    fn process_name_detects_codex_before_screen_banner() {
        let snapshot = TerminalSnapshot {
            columns: 80,
            rows: 1,
            lines: vec![Arc::from([TerminalCell::with_text(
                0,
                0,
                "ready",
                TerminalRgb::new(255, 255, 255),
                TerminalRgb::new(0, 0, 0),
            )])],
            cursor: None,
            display_offset: 0,
            history_size: 0,
        };

        let presence = detect_agent_presence(
            "Claude Code",
            &snapshot,
            None,
            Some("codex-code-mode-host"),
            Some(42),
        )
        .unwrap();
        assert_eq!(presence.kind, "Codex");
        assert_eq!(presence.kind_source, TerminalAgentKindSource::Process);
        assert_eq!(presence.state, TerminalAgentState::Idle);
        assert_eq!(presence.process_id, Some(42));

        let bare = detect_agent_presence(
            "Terminal",
            &snapshot,
            None,
            Some("/usr/local/bin/codex"),
            Some(43),
        )
        .unwrap();
        assert_eq!(bare.kind, "Codex");
        assert_eq!(bare.kind_source, TerminalAgentKindSource::Process);
        assert_eq!(
            detect_agent_presence("Terminal", &snapshot, None, Some("goose"), Some(46))
                .unwrap()
                .kind,
            "Goose"
        );

        let process_with_state_word =
            detect_agent_presence("Terminal", &snapshot, None, Some("codex-working"), Some(44))
                .unwrap();
        assert_eq!(process_with_state_word.state, TerminalAgentState::Idle);
        assert!(agent_kind_from_process_name("codexical").is_none());
        assert!(
            detect_agent_presence(
                "Claude Code",
                &snapshot,
                Some("old Claude Code output"),
                Some("/bin/zsh"),
                Some(45),
            )
            .is_none(),
            "a shell must win over stale title and scrollback evidence"
        );
    }

    #[test]
    fn bracketed_paste_is_wrapped_and_cannot_close_early() {
        assert_eq!(
            paste_bytes("uno\x1b[201~dos", true),
            b"\x1b[200~unodos\x1b[201~"
        );
        assert_eq!(
            paste_bytes("uno\x03dos\x1b[31m", true),
            b"\x1b[200~unodos[31m\x1b[201~"
        );
    }

    #[test]
    fn plain_paste_normalizes_line_endings_to_enter() {
        assert_eq!(paste_bytes("uno\r\ndos\ntres", false), b"uno\rdos\rtres");
    }

    #[test]
    fn paste_filters_non_text_control_characters() {
        assert_eq!(
            paste_bytes("uno\0\x04\x07\tdos", true),
            b"\x1b[200~uno\tdos\x1b[201~"
        );
    }

    #[test]
    fn multiline_and_control_pastes_require_confirmation() {
        assert!(!paste_requires_confirmation("cargo test"));
        assert!(!paste_requires_confirmation("uno\tdos"));
        assert!(paste_requires_confirmation("cargo test\nrm -rf build"));
        assert!(paste_requires_confirmation("echo\x1b[31m"));
    }

    #[test]
    fn clipboard_image_detection_ignores_text_only_items() {
        let text = ClipboardItem::new_string("hola".into());
        assert!(!clipboard_has_image(&text));
    }

    #[test]
    fn modified_arrows_use_xterm_modifier_parameters() {
        let bytes = key_event_bytes(
            &key(
                "up",
                Modifiers {
                    shift: true,
                    control: true,
                    ..Modifiers::default()
                },
            ),
            TerminalInputMode::default(),
            TerminalKeyEventType::Press,
        );
        assert_eq!(bytes.as_deref(), Some(b"\x1b[1;6A".as_slice()));
    }

    #[test]
    fn kitty_keyboard_reports_press_repeat_and_release() {
        let mode = TerminalInputMode {
            disambiguate_escape_codes: true,
            report_event_types: true,
            ..TerminalInputMode::default()
        };
        let keystroke = key(
            "c",
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(
            key_event_bytes(&keystroke, mode, TerminalKeyEventType::Press).as_deref(),
            Some(b"\x1b[99;5u".as_slice())
        );
        assert_eq!(
            key_event_bytes(&keystroke, mode, TerminalKeyEventType::Repeat).as_deref(),
            Some(b"\x1b[99;5:2u".as_slice())
        );
        assert_eq!(
            key_event_bytes(&keystroke, mode, TerminalKeyEventType::Release).as_deref(),
            Some(b"\x1b[99;5:3u".as_slice())
        );
    }

    #[test]
    fn sgr_mouse_reports_coordinates_modifiers_and_release() {
        let mode = TerminalInputMode {
            sgr_mouse: true,
            mouse_report_click: true,
            ..TerminalInputMode::default()
        };
        let modifiers = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let point = TerminalPoint { row: 4, column: 9 };
        assert_eq!(
            mouse_report_bytes(point, 0, MouseReportState::Pressed, modifiers, mode).as_deref(),
            Some(b"\x1b[<16;10;5M".as_slice())
        );
        assert_eq!(
            mouse_report_bytes(point, 0, MouseReportState::Released, modifiers, mode).as_deref(),
            Some(b"\x1b[<16;10;5m".as_slice())
        );
    }

    #[test]
    fn legacy_mouse_uses_x10_packet_encoding() {
        let point = TerminalPoint { row: 1, column: 2 };
        assert_eq!(
            mouse_report_bytes(
                point,
                0,
                MouseReportState::Pressed,
                Modifiers::default(),
                TerminalInputMode {
                    mouse_report_click: true,
                    ..TerminalInputMode::default()
                },
            ),
            Some(vec![0x1b, b'[', b'M', 32, 35, 34])
        );
    }

    #[test]
    fn hyperlink_opening_is_limited_to_expected_schemes() {
        assert!(is_safe_hyperlink("https://example.com"));
        assert!(is_safe_hyperlink("mailto:hello@example.com"));
        assert!(!is_safe_hyperlink("javascript:alert(1)"));
    }
}
