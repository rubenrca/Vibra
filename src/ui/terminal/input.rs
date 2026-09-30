use std::ops::Range;

use super::*;
use crate::ports::terminal_keyboard::{
    TerminalKeyEventType, TerminalKeyInput, TerminalKeystroke, TerminalModifiers,
};
use gpui::{
    Bounds, ClipboardEntry, ClipboardItem, Context, EntityInputHandler, KeyDownEvent, KeyUpEvent,
    Keystroke, Pixels, UTF16Selection, Window, size,
};

impl TerminalView {
    pub(crate) fn record_input_result(
        &mut self,
        result: anyhow::Result<()>,
        cx: &mut Context<Self>,
    ) -> bool {
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
                    Some(format!("Could not send to the terminal: {error:#}").into());
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

    pub(crate) fn send(&mut self, input: Vec<u8>, cx: &mut Context<Self>) -> bool {
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

    pub(crate) fn send_protocol(&mut self, input: Vec<u8>, cx: &mut Context<Self>) -> bool {
        let Some(handle) = &self.handle else {
            return false;
        };
        self.record_input_result(handle.send_input(input), cx)
    }

    pub(crate) fn send_key(
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

    pub(crate) fn current_input_mode(&self) -> TerminalInputMode {
        self.handle
            .as_ref()
            .map(|handle| handle.input_mode())
            .unwrap_or_default()
    }

    pub(crate) fn paste(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let mode = self.current_input_mode();
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

    pub(crate) fn request_paste(&mut self, text: String, cx: &mut Context<Self>) {
        // When the app enabled bracketed paste, inject immediately (Warp/iTerm).
        // Confirm only for raw pastes that could execute as typed input.
        let bracketed = self.current_input_mode().bracketed_paste;
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
    pub(crate) fn paste_from_system_clipboard(&mut self, cx: &mut Context<Self>) {
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
    pub(crate) fn has_active_cli_agent(&self) -> bool {
        if self.agent_presence.is_some() {
            return true;
        }
        self.foreground_process_name()
            .as_deref()
            .and_then(AgentKind::from_process_name)
            .is_some()
    }

    pub(crate) fn confirm_pending_action(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn cancel_pending_action(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn copy_selection(&self, cx: &mut Context<Self>) -> bool {
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

    pub(crate) fn start_search(&mut self, cx: &mut Context<Self>) {
        self.search_active = true;
        self.marked_text.clear();
        if !self.search_query.is_empty() {
            self.refresh_search(cx);
        }
        cx.notify();
    }

    pub(crate) fn close_search(&mut self, clear_selection: bool, cx: &mut Context<Self>) {
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

    pub(crate) fn search(&mut self, direction: TerminalSearchDirection, cx: &mut Context<Self>) {
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

    pub(crate) fn refresh_search(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.handle {
            handle.clear_selection();
        }
        self.search(TerminalSearchDirection::Next, cx);
    }

    pub(crate) fn reset_cursor_blink(&mut self) {
        self.cursor_visible = true;
    }

    pub(crate) fn copy_action(&mut self, _: &CopyTerminal, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    pub(crate) fn paste_action(
        &mut self,
        _: &PasteTerminal,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_from_system_clipboard(cx);
    }

    pub(crate) fn search_action(
        &mut self,
        _: &SearchTerminal,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_search(cx);
    }

    pub(crate) fn search_next_action(
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

    pub(crate) fn search_previous_action(
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

    pub(crate) fn increase_font_size_action(
        &mut self,
        _: &IncreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_font_size(self.font_size + 1.0, cx);
    }

    pub(crate) fn decrease_font_size_action(
        &mut self,
        _: &DecreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_font_size(self.font_size - 1.0, cx);
    }

    pub(crate) fn reset_font_size_action(
        &mut self,
        _: &ResetTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_font_size(TERMINAL_FONT_SIZE, cx);
    }

    pub(crate) fn clear_scrollback_action(
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

    pub(crate) fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.update_font_size(size, true, cx);
    }

    pub(crate) fn update_font_size(&mut self, size: f32, emit: bool, cx: &mut Context<Self>) {
        let size = size.clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE);
        if self.font_size != size {
            self.font_size = size;
            if emit {
                cx.emit(TerminalViewEvent::FontSizeChanged { size });
            }
            cx.notify();
        }
    }

    pub(crate) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

        let mode = self.current_input_mode();
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

    pub(crate) fn on_key_up(
        &mut self,
        event: &KeyUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let mode = self.current_input_mode();
        if key_event_bytes(&event.keystroke, mode, TerminalKeyEventType::Release).is_some() {
            self.send_key(&event.keystroke, TerminalKeyEventType::Release, cx);
            cx.stop_propagation();
        }
    }
}

pub(crate) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
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

pub(crate) fn paste_requires_confirmation(text: &str) -> bool {
    text.contains(['\r', '\n'])
        || text
            .chars()
            .any(|character| character.is_control() && character != '\t')
}

pub(crate) fn clipboard_has_image(item: &ClipboardItem) -> bool {
    item.entries()
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::Image(_)))
}

#[cfg(test)]
pub(crate) fn key_bytes(key: &Keystroke, mode: TerminalInputMode) -> Option<Vec<u8>> {
    key_event_bytes(key, mode, TerminalKeyEventType::Press)
}
pub(crate) fn key_event_bytes(
    key: &Keystroke,
    mode: TerminalInputMode,
    event: TerminalKeyEventType,
) -> Option<Vec<u8>> {
    crate::infrastructure::terminal_keyboard::key_event_bytes(&terminal_keystroke(key), mode, event)
}

pub(crate) fn terminal_keystroke(key: &Keystroke) -> TerminalKeystroke {
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

pub(crate) fn is_terminal_special_key(key: &str) -> bool {
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
