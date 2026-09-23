//! Keyboard input values passed to a terminal backend.

#[derive(Clone, Copy, Default)]
pub struct TerminalModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
    pub platform: bool,
}

pub struct TerminalKeystroke {
    pub key: String,
    pub key_char: Option<String>,
    pub modifiers: TerminalModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKeyEventType {
    Press,
    Repeat,
    Release,
}

/// Keep key events unencoded until pending terminal output has been parsed.
/// A TUI can restore the shell's keyboard mode while this input is in flight.
pub struct TerminalKeyInput {
    pub keystroke: TerminalKeystroke,
    pub event_type: TerminalKeyEventType,
}
