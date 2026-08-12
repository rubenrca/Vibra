use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use async_channel::Receiver;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            columns: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 18.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalRgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl TerminalRgb {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCell {
    pub row: usize,
    pub column: usize,
    pub text: String,
    pub foreground: TerminalRgb,
    pub background: TerminalRgb,
    pub underline_color: TerminalRgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: TerminalUnderline,
    pub strikeout: bool,
    pub hidden: bool,
    pub wide_spacer: bool,
    pub selected: bool,
    pub hyperlink: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalUnderline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursor {
    pub row: usize,
    pub column: usize,
    pub shape: TerminalCursorShape,
    pub blinking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub columns: usize,
    pub rows: usize,
    pub lines: Vec<Arc<[TerminalCell]>>,
    pub cursor: Option<TerminalCursor>,
    pub display_offset: usize,
    pub history_size: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalInputMode {
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub alternate_scroll: bool,
    pub focus_reporting: bool,
    pub mouse_report_click: bool,
    pub mouse_drag: bool,
    pub mouse_motion: bool,
    pub sgr_mouse: bool,
    pub utf8_mouse: bool,
    pub disambiguate_escape_codes: bool,
    pub report_event_types: bool,
    pub report_alternate_keys: bool,
    pub report_all_keys_as_escape_codes: bool,
    pub report_associated_text: bool,
}

impl TerminalInputMode {
    pub fn mouse_mode(self) -> bool {
        self.mouse_report_click || self.mouse_drag || self.mouse_motion
    }

    pub fn kitty_keyboard(self) -> bool {
        self.disambiguate_escape_codes
            || self.report_event_types
            || self.report_alternate_keys
            || self.report_all_keys_as_escape_codes
            || self.report_associated_text
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalPoint {
    pub row: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalCellSide {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalSelectionType {
    #[default]
    Simple,
    Block,
    Semantic,
    Lines,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalSearchDirection {
    Previous,
    #[default]
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalAgentState {
    Idle,
    Working,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalAgentKindSource {
    Process,
    Title,
    Screen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalAgentPresence {
    pub kind: String,
    pub kind_source: TerminalAgentKindSource,
    pub state: TerminalAgentState,
}

#[derive(Clone)]
pub enum TerminalEvent {
    Wakeup,
    Title(String),
    ResetTitle,
    ClipboardStore(String),
    ClipboardLoad(Arc<dyn Fn(&str) -> String + Send + Sync>),
    Bell,
    Exit(Option<i32>),
}

/// Boundary between the workspace/UI and the terminal emulator.
pub trait TerminalPort: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn spawn(
        &self,
        session_id: Uuid,
        working_directory: &Path,
        environment: &HashMap<String, String>,
    ) -> Result<Arc<dyn TerminalHandle>>;
}

pub trait TerminalHandle: Send + Sync {
    fn events(&self) -> Receiver<TerminalEvent>;
    fn send_input(&self, input: Vec<u8>) -> Result<()>;
    fn resize(&self, size: TerminalSize) -> Result<()>;
    fn scroll(&self, lines: i32);
    fn clear_scrollback(&self);
    fn snapshot(&self) -> Arc<TerminalSnapshot>;
    fn input_mode(&self) -> TerminalInputMode;
    fn current_working_directory(&self) -> Option<PathBuf> {
        None
    }
    /// Basename (or full path) of the process currently owning the TTY, when known.
    /// Used to detect coding agents even before their UI prints a brand string.
    fn foreground_process_name(&self) -> Option<String> {
        None
    }
    fn clear_selection(&self);
    fn start_selection(
        &self,
        selection_type: TerminalSelectionType,
        point: TerminalPoint,
        side: TerminalCellSide,
    );
    fn update_selection(&self, point: TerminalPoint, side: TerminalCellSide);
    fn selection_text(&self) -> Option<String>;
    fn search(&self, query: &str, direction: TerminalSearchDirection) -> Result<bool>;
    fn hyperlink_at(&self, point: TerminalPoint) -> Option<String>;
    fn acknowledge_wakeup(&self);
    fn shutdown(&self);
}
