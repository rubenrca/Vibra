use super::*;
use gpui::{
    Context, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Window, px,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseReportState {
    Pressed,
    Released,
}

impl TerminalView {
    pub(crate) fn on_scroll(
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

    pub(crate) fn on_mouse_down(
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

    pub(crate) fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(crate) fn on_mouse_move(
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

    pub(crate) fn terminal_point(
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
}

pub(crate) fn mouse_button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

pub(crate) fn mouse_report_bytes(
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

pub(crate) fn encode_mouse_coordinate(bytes: &mut Vec<u8>, coordinate: usize, utf8: bool) {
    let encoded = 33 + coordinate;
    if utf8 && coordinate >= 95 {
        bytes.push((0xc0 + encoded / 64) as u8);
        bytes.push((0x80 + (encoded & 63)) as u8);
    } else {
        bytes.push(encoded as u8);
    }
}
