//! The draft field: a multi-line text editor with a blinking caret, a
//! selection, mouse placement (click, double-click for a word, triple-click
//! for a line, drag to select) and IME input (dead keys, the emoji picker,
//! CJK) through GPUI's input handler.
//!
//! The text lives in `AgentChatView::draft`; this module keeps the caret and
//! selection over it as UTF-8 byte offsets. Printable keys are not handled in
//! `on_key_down`: they go on to the platform input handler, which inserts them
//! through `EntityInputHandler`, so composed characters work. Editing and
//! motion keys are handled here and stop propagation.

use std::ops::Range;
use std::time::Duration;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, GlobalElementId, Hsla, InspectorElementId, IntoElement, Keystroke,
    LayoutId, MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Point, Rgba, SharedString, Style,
    Task, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, fill, point, px,
    relative, size,
};

use super::super::AgentChatView;
use crate::ui::theme::colors;

/// The field grows up to this many lines, then scrolls to keep the caret in view.
const MAX_VISIBLE_LINES: f32 = 8.0;
/// How long the caret stays on, then off.
const BLINK_INTERVAL: Duration = Duration::from_millis(530);
/// Width of the caret bar.
const CARET_WIDTH: Pixels = px(2.0);

/// The caret and selection, as UTF-8 byte offsets into the draft. The head is
/// the end that moves; it is `range.start` when `reversed`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Selection {
    pub range: Range<usize>,
    pub reversed: bool,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self {
            range: at..at,
            reversed: false,
        }
    }

    /// The end of the selection that moves with the keys and the mouse.
    pub fn head(&self) -> usize {
        if self.reversed {
            self.range.start
        } else {
            self.range.end
        }
    }

    fn tail(&self) -> usize {
        if self.reversed {
            self.range.end
        } else {
            self.range.start
        }
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    /// Moves the head to `head`, keeping the tail.
    pub fn select_to(&mut self, head: usize) {
        let tail = self.tail();
        if head < tail {
            self.range = head..tail;
            self.reversed = true;
        } else {
            self.range = tail..head;
            self.reversed = false;
        }
    }
}

/// Where a motion key moves the head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Motion {
    CharLeft,
    CharRight,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    TextStart,
    TextEnd,
}

/// The previous char boundary before `at`.
fn prev_char(text: &str, at: usize) -> usize {
    text[..at]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

/// The next char boundary after `at`.
fn next_char(text: &str, at: usize) -> usize {
    text[at..]
        .chars()
        .next()
        .map_or(text.len(), |character| at + character.len_utf8())
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The start of the word before `at`, skipping the spaces and punctuation in between.
fn prev_word(text: &str, at: usize) -> usize {
    let mut index = at;
    let mut in_word = false;
    for (start, character) in text[..at].char_indices().rev() {
        if is_word_char(character) {
            in_word = true;
        } else if in_word {
            break;
        }
        index = start;
    }
    index
}

/// The end of the word after `at`, skipping the spaces and punctuation in between.
fn next_word(text: &str, at: usize) -> usize {
    let mut in_word = false;
    for (offset, character) in text[at..].char_indices() {
        if is_word_char(character) {
            in_word = true;
        } else if in_word {
            return at + offset;
        }
    }
    text.len()
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |index| at + index)
}

/// The word around `at` for a double-click, or the single character there
/// when it is not part of a word.
pub(super) fn word_range(text: &str, at: usize) -> Range<usize> {
    let at = at.min(text.len());
    let touches_word = text[at..].chars().next().is_some_and(is_word_char)
        || text[..at].chars().next_back().is_some_and(is_word_char);
    if !touches_word {
        return at..next_char(text, at);
    }
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_word_char(*character))
        .last()
        .map_or(at, |(index, _)| index);
    let end = text[at..]
        .char_indices()
        .find(|(_, character)| !is_word_char(*character))
        .map_or(text.len(), |(offset, _)| at + offset);
    start..end
}

/// The line around `at` for a triple-click, with its line break.
pub(super) fn line_range(text: &str, at: usize) -> Range<usize> {
    let end = line_end(text, at);
    line_start(text, at)..next_char(text, end).max(end)
}

/// Where `motion` takes the head. A plain left or right move with a selection
/// collapses it to that side, as macOS does.
pub(super) fn motion_target(
    text: &str,
    selection: &Selection,
    motion: Motion,
    extend: bool,
) -> usize {
    let head = selection.head();
    match motion {
        Motion::CharLeft if !extend && !selection.is_empty() => selection.range.start,
        Motion::CharRight if !extend && !selection.is_empty() => selection.range.end,
        Motion::CharLeft => prev_char(text, head),
        Motion::CharRight => next_char(text, head),
        Motion::WordLeft => prev_word(text, head),
        Motion::WordRight => next_word(text, head),
        Motion::LineStart => line_start(text, head),
        Motion::LineEnd => line_end(text, head),
        Motion::TextStart => 0,
        Motion::TextEnd => text.len(),
    }
}

/// Moves the head by `motion`; `extend` keeps the tail, so the selection grows.
pub(super) fn apply_motion(text: &str, selection: &mut Selection, motion: Motion, extend: bool) {
    let target = motion_target(text, selection, motion, extend);
    move_head(selection, target, extend);
}

fn move_head(selection: &mut Selection, target: usize, extend: bool) {
    if extend {
        selection.select_to(target);
    } else {
        *selection = Selection::caret(target);
    }
}

/// Replaces the selection with `new_text` and leaves the caret after it.
pub(super) fn replace_selection(text: &mut String, selection: &mut Selection, new_text: &str) {
    let range = selection.range.clone();
    text.replace_range(range.clone(), new_text);
    *selection = Selection::caret(range.start + new_text.len());
}

/// Deletes the selection, or, when it is empty, from the caret to where `motion` goes.
pub(super) fn delete(text: &mut String, selection: &mut Selection, motion: Motion) {
    if selection.is_empty() {
        let target = motion_target(text, selection, motion, true);
        selection.select_to(target);
    }
    replace_selection(text, selection, "");
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

fn offset_from_utf16(text: &str, utf16: usize) -> usize {
    let mut seen = 0;
    for (index, character) in text.char_indices() {
        if seen >= utf16 {
            return index;
        }
        seen += character.len_utf16();
    }
    text.len()
}

fn offset_to_utf16(text: &str, offset: usize) -> usize {
    utf16_len(&text[..offset.min(text.len())])
}

fn range_from_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    offset_from_utf16(text, range.start)..offset_from_utf16(text, range.end)
}

fn range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    offset_to_utf16(text, range.start)..offset_to_utf16(text, range.end)
}

/// The shaped draft from the last paint, for mouse hit-testing, vertical
/// caret moves and the IME candidate window. Positions are window coordinates.
#[derive(Clone)]
pub(super) struct DraftLayout {
    lines: Vec<WrappedLine>,
    /// Byte offset where each hard line starts.
    starts: Vec<usize>,
    /// Top of each hard line, relative to the text origin.
    tops: Vec<Pixels>,
    /// Where the first line is drawn: the field's origin, scrolled.
    origin: Point<Pixels>,
    line_height: Pixels,
    /// The draft was empty; the lines are the placeholder's.
    empty: bool,
}

impl DraftLayout {
    fn hard_line_of(&self, offset: usize) -> usize {
        self.starts
            .iter()
            .rposition(|start| *start <= offset)
            .unwrap_or(0)
    }

    /// Top-left of the caret at `offset`, in window coordinates.
    pub fn position_for_offset(&self, offset: usize) -> Point<Pixels> {
        if self.empty || self.lines.is_empty() {
            return self.origin;
        }
        let line = self.hard_line_of(offset);
        let local = self.lines[line]
            .position_for_index(offset - self.starts[line], self.line_height)
            .unwrap_or_default();
        point(
            self.origin.x + local.x,
            self.origin.y + self.tops[line] + local.y,
        )
    }

    /// The offset closest to `position`, in window coordinates.
    pub fn offset_for_position(&self, position: Point<Pixels>) -> usize {
        if self.empty || self.lines.is_empty() {
            return 0;
        }
        let y = position.y - self.origin.y;
        if y < px(0.0) {
            return 0;
        }
        let line = self.tops.iter().rposition(|top| *top <= y).unwrap_or(0);
        let local = point(
            (position.x - self.origin.x).max(px(0.0)),
            y - self.tops[line],
        );
        let index = match self.lines[line].closest_index_for_position(local, self.line_height) {
            Ok(index) | Err(index) => index,
        };
        // Below the last row of a line, the hit lands at that line's end.
        let line_bottom = self.tops[line] + self.lines[line].size(self.line_height).height;
        let index = if y >= line_bottom {
            self.lines[line].len()
        } else {
            index
        };
        self.starts[line] + index.min(self.lines[line].len())
    }
}

/// Caret, selection, IME and blink state of the draft field.
#[derive(Default)]
pub(super) struct EditorState {
    pub selection: Selection,
    /// Text the IME is composing (for example a dead key's accent), underlined.
    marked: Option<Range<usize>>,
    /// The draft as the editor last saw it. A different draft means code
    /// outside the editor replaced it, so the caret moves to its end.
    known: String,
    /// The mouse is down in the field, so moves extend the selection.
    selecting: bool,
    /// The x the caret keeps while moving up and down through shorter lines.
    goal_x: Option<Pixels>,
    scroll_y: Pixels,
    layout: Option<DraftLayout>,
    /// The field has focus and takes input, so the caret blinks.
    focused: bool,
    caret_on: bool,
    blink_epoch: usize,
    blink_task: Option<Task<()>>,
}

impl std::fmt::Debug for EditorState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EditorState")
            .field("selection", &self.selection)
            .field("marked", &self.marked)
            .finish_non_exhaustive()
    }
}

impl EditorState {
    /// Moves the caret to the end when the draft changed outside the editor.
    pub fn sync(&mut self, draft: &str) {
        if self.known != draft {
            self.known = draft.to_owned();
            self.selection = Selection::caret(draft.len());
            self.marked = None;
            self.goal_x = None;
        }
    }

    /// Whether the caret sits at the end of the draft with nothing selected.
    pub fn caret_at_end(&self, draft: &str) -> bool {
        self.known != draft || (self.selection.is_empty() && self.selection.head() == draft.len())
    }
}

/// What a key does to the draft field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorKey {
    /// The key edited the draft or moved the caret.
    Handled,
    /// Enter: send the draft, or queue it while a turn runs.
    Send,
    /// Escape.
    Cancel,
    /// Not an editing key. Printable keys land here and go on to the input handler.
    Ignored,
}

/// How a key maps onto the draft, without touching it. Pure, for the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyAction {
    Send,
    Cancel,
    Newline,
    Move {
        motion: Motion,
        extend: bool,
    },
    /// Up or down one visual line.
    Vertical {
        down: bool,
        extend: bool,
    },
    Delete(Motion),
    SelectAll,
    Copy,
    Cut,
    Paste,
}

/// The action for a keystroke, or `None` for keys the field does not handle
/// (printable characters, which the input handler inserts, and app shortcuts).
pub(super) fn key_action(keystroke: &Keystroke) -> Option<KeyAction> {
    let modifiers = &keystroke.modifiers;
    let extend = modifiers.shift;
    let command = modifiers.platform;
    let horizontal = |left: bool| {
        let motion = match (command, modifiers.alt, left) {
            (true, _, true) => Motion::LineStart,
            (true, _, false) => Motion::LineEnd,
            (false, true, true) => Motion::WordLeft,
            (false, true, false) => Motion::WordRight,
            (false, false, true) => Motion::CharLeft,
            (false, false, false) => Motion::CharRight,
        };
        KeyAction::Move { motion, extend }
    };
    Some(match keystroke.key.as_str() {
        "escape" | "esc" => KeyAction::Cancel,
        "enter" | "return" if modifiers.shift || modifiers.alt => KeyAction::Newline,
        "enter" | "return" if !command && !modifiers.control => KeyAction::Send,
        "backspace" if command => KeyAction::Delete(Motion::LineStart),
        "backspace" if modifiers.alt || modifiers.control => KeyAction::Delete(Motion::WordLeft),
        "backspace" => KeyAction::Delete(Motion::CharLeft),
        "delete" if command => KeyAction::Delete(Motion::LineEnd),
        "delete" if modifiers.alt => KeyAction::Delete(Motion::WordRight),
        "delete" => KeyAction::Delete(Motion::CharRight),
        "left" => horizontal(true),
        "right" => horizontal(false),
        "up" if command => KeyAction::Move {
            motion: Motion::TextStart,
            extend,
        },
        "down" if command => KeyAction::Move {
            motion: Motion::TextEnd,
            extend,
        },
        "up" => KeyAction::Vertical {
            down: false,
            extend,
        },
        "down" => KeyAction::Vertical { down: true, extend },
        "home" => KeyAction::Move {
            motion: Motion::LineStart,
            extend,
        },
        "end" => KeyAction::Move {
            motion: Motion::LineEnd,
            extend,
        },
        "a" if command => KeyAction::SelectAll,
        "c" if command => KeyAction::Copy,
        "x" if command => KeyAction::Cut,
        "v" if command => KeyAction::Paste,
        // Emacs keys that macOS text fields honor.
        "a" if modifiers.control => KeyAction::Move {
            motion: Motion::LineStart,
            extend,
        },
        "e" if modifiers.control => KeyAction::Move {
            motion: Motion::LineEnd,
            extend,
        },
        "k" if modifiers.control => KeyAction::Delete(Motion::LineEnd),
        _ => return None,
    })
}

impl AgentChatView {
    /// Applies an editing or motion key to the draft. Printable keys return
    /// `Ignored` so they reach the input handler.
    pub(super) fn editor_key(
        &mut self,
        keystroke: &Keystroke,
        cx: &mut Context<Self>,
    ) -> EditorKey {
        let Some(action) = key_action(keystroke) else {
            return EditorKey::Ignored;
        };
        self.composer.editor.sync(&self.draft);
        let editor = &mut self.composer.editor;
        if !matches!(action, KeyAction::Vertical { .. }) {
            editor.goal_x = None;
        }
        let mut edited = false;
        match action {
            KeyAction::Send => return EditorKey::Send,
            KeyAction::Cancel => return EditorKey::Cancel,
            KeyAction::Newline => {
                replace_selection(&mut self.draft, &mut editor.selection, "\n");
                edited = true;
            }
            KeyAction::Move { motion, extend } => {
                apply_motion(&self.draft, &mut editor.selection, motion, extend);
            }
            KeyAction::Vertical { down, extend } => {
                let target = vertical_target(editor, &self.draft, down);
                move_head(&mut editor.selection, target, extend);
            }
            KeyAction::Delete(motion) => {
                delete(&mut self.draft, &mut editor.selection, motion);
                edited = true;
            }
            KeyAction::SelectAll => {
                editor.selection = Selection::caret(0);
                editor.selection.select_to(self.draft.len());
            }
            KeyAction::Copy | KeyAction::Cut => {
                let range = editor.selection.range.clone();
                if range.is_empty() {
                    return EditorKey::Handled;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(self.draft[range].to_owned()));
                if action == KeyAction::Cut {
                    replace_selection(&mut self.draft, &mut editor.selection, "");
                    edited = true;
                }
            }
            KeyAction::Paste => {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return EditorKey::Handled;
                };
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                replace_selection(&mut self.draft, &mut editor.selection, &text);
                edited = true;
            }
        }
        editor.marked = None;
        self.finish_edit(edited, cx);
        EditorKey::Handled
    }

    /// Bookkeeping after the editor changed the draft or moved the caret: the
    /// caret shows solid again, and an edit reopens popups.
    fn finish_edit(&mut self, edited: bool, cx: &mut Context<Self>) {
        self.composer.editor.known.clone_from(&self.draft);
        self.reset_caret_blink(cx);
        if edited {
            self.after_edit(cx);
        }
        cx.notify();
    }

    /// Shows the caret solid and restarts its blinking, as text fields do after
    /// each key. The blink stops once the field loses focus.
    pub(super) fn reset_caret_blink(&mut self, cx: &mut Context<Self>) {
        let editor = &mut self.composer.editor;
        editor.caret_on = true;
        editor.blink_epoch += 1;
        if !editor.focused {
            editor.blink_task = None;
            return;
        }
        let epoch = editor.blink_epoch;
        editor.blink_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                let blinking = this
                    .update(cx, |view, cx| {
                        let editor = &mut view.composer.editor;
                        if editor.blink_epoch != epoch || !editor.focused {
                            return false;
                        }
                        editor.caret_on = !editor.caret_on;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !blinking {
                    return;
                }
            }
        }));
    }

    /// Called on each render: follows outside changes to the draft and starts
    /// or stops the blink as the field gains or loses focus.
    pub(in crate::ui::agent_chat) fn sync_editor(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.composer.editor.sync(&self.draft);
        if self.composer.editor.focused != focused {
            self.composer.editor.focused = focused;
            self.reset_caret_blink(cx);
        }
    }

    /// Click: place the caret (Shift extends); double-click selects a word and
    /// triple-click a line. Dragging afterwards extends the selection.
    pub(super) fn editor_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if !self.accepts_input() {
            return;
        }
        self.composer.editor.sync(&self.draft);
        let editor = &mut self.composer.editor;
        let Some(layout) = editor.layout.as_ref() else {
            return;
        };
        let offset = layout.offset_for_position(event.position);
        editor.goal_x = None;
        editor.selecting = true;
        match event.click_count {
            0 | 1 if event.modifiers.shift => editor.selection.select_to(offset),
            0 | 1 => editor.selection = Selection::caret(offset),
            2 => {
                let range = word_range(&self.draft, offset);
                editor.selection = Selection {
                    range,
                    reversed: false,
                };
            }
            _ => {
                let range = line_range(&self.draft, offset);
                editor.selection = Selection {
                    range,
                    reversed: false,
                };
            }
        }
        self.finish_edit(false, cx);
    }

    pub(super) fn editor_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let editor = &mut self.composer.editor;
        if !editor.selecting || !event.dragging() {
            return;
        }
        let Some(layout) = editor.layout.as_ref() else {
            return;
        };
        let offset = layout.offset_for_position(event.position);
        if offset != editor.selection.head() {
            editor.selection.select_to(offset);
            self.finish_edit(false, cx);
        }
    }

    pub(super) fn editor_mouse_up(&mut self) {
        self.composer.editor.selecting = false;
    }

    /// The draft field element, which draws the text, selection and caret and
    /// takes IME input while the composer accepts it.
    pub(super) fn draft_field(
        &self,
        enabled: bool,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> DraftField {
        let editor = &self.composer.editor;
        let in_sync = editor.known == self.draft;
        let selection = if in_sync {
            editor.selection.clone()
        } else {
            Selection::caret(self.draft.len())
        };
        DraftField {
            view: cx.entity(),
            text: SharedString::from(self.draft.clone()),
            placeholder: SharedString::from(self.composer_placeholder()),
            selection,
            marked: editor.marked.clone().filter(|_| in_sync),
            show_caret: enabled && focused && editor.caret_on,
            enabled,
            scroll_y: editor.scroll_y,
        }
    }
}

/// The head's target one visual line up or down. Past the first or last line
/// it goes to the start or end of the draft, as macOS does.
fn vertical_target(editor: &mut EditorState, text: &str, down: bool) -> usize {
    let head = editor.selection.head();
    let Some(layout) = editor.layout.as_ref() else {
        return if down { text.len() } else { 0 };
    };
    let caret = layout.position_for_offset(head);
    let x = *editor.goal_x.get_or_insert(caret.x);
    let line_height = layout.line_height;
    let y = if down {
        caret.y + line_height * 1.5
    } else {
        caret.y - line_height * 0.5
    };
    let content_bottom = layout.origin.y
        + layout.tops.last().copied().unwrap_or_default()
        + layout
            .lines
            .last()
            .map_or(line_height, |line| line.size(line_height).height);
    if y < layout.origin.y {
        0
    } else if y >= content_bottom {
        text.len()
    } else {
        layout.offset_for_position(point(x, y)).min(text.len())
    }
}

impl EntityInputHandler for AgentChatView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        self.composer.editor.sync(&self.draft);
        let range = range_from_utf16(&self.draft, &range_utf16);
        actual_range.replace(range_to_utf16(&self.draft, &range));
        Some(self.draft[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.composer.editor.sync(&self.draft);
        let selection = &self.composer.editor.selection;
        Some(UTF16Selection {
            range: range_to_utf16(&self.draft, &selection.range),
            reversed: selection.reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let editor = &self.composer.editor;
        if editor.known != self.draft {
            return None;
        }
        editor
            .marked
            .as_ref()
            .map(|range| range_to_utf16(&self.draft, range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.composer.editor.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_input() {
            return;
        }
        self.composer.editor.sync(&self.draft);
        let editor = &mut self.composer.editor;
        let range = range_utf16
            .map(|range| range_from_utf16(&self.draft, &range))
            .or(editor.marked.take())
            .unwrap_or(editor.selection.range.clone());
        editor.selection = Selection {
            range,
            reversed: false,
        };
        editor.goal_x = None;
        replace_selection(&mut self.draft, &mut editor.selection, new_text);
        self.finish_edit(true, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_input() {
            return;
        }
        self.composer.editor.sync(&self.draft);
        let editor = &mut self.composer.editor;
        let range = range_utf16
            .map(|range| range_from_utf16(&self.draft, &range))
            .or(editor.marked.clone())
            .unwrap_or(editor.selection.range.clone());
        self.draft.replace_range(range.clone(), new_text);
        editor.marked = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        editor.selection = match new_selected_range_utf16 {
            Some(selected) => {
                let selected = range_from_utf16(new_text, &selected);
                Selection {
                    range: range.start + selected.start..range.start + selected.end,
                    reversed: false,
                }
            }
            None => Selection::caret(range.start + new_text.len()),
        };
        editor.goal_x = None;
        self.finish_edit(true, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.composer.editor.layout.as_ref()?;
        let range = range_from_utf16(&self.draft, &range_utf16);
        let start = layout.position_for_offset(range.start);
        let end = layout.position_for_offset(range.end);
        Some(Bounds::from_corners(
            start,
            point(end.x.max(start.x), end.y + layout.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.composer.editor.layout.as_ref()?;
        let offset = layout.offset_for_position(point);
        Some(offset_to_utf16(&self.draft, offset))
    }
}

/// The element that draws the draft (or its placeholder), the selection and
/// the caret, and registers the input handler.
pub(super) struct DraftField {
    view: Entity<AgentChatView>,
    text: SharedString,
    placeholder: SharedString,
    selection: Selection,
    marked: Option<Range<usize>>,
    show_caret: bool,
    enabled: bool,
    scroll_y: Pixels,
}

pub(super) struct DraftPrepaint {
    layout: DraftLayout,
    selections: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    scroll_y: Pixels,
}

impl DraftField {
    fn display_text(&self) -> SharedString {
        if self.text.is_empty() {
            self.placeholder.clone()
        } else {
            self.text.clone()
        }
    }

    /// Text runs for the shown text: the placeholder is subtle, and IME text
    /// being composed is underlined.
    fn runs(&self, window: &Window) -> Vec<TextRun> {
        let style = window.text_style();
        let empty = self.text.is_empty();
        let color: Hsla = if empty {
            colors().subtle.into()
        } else {
            style.color
        };
        let shown = self.display_text();
        let run = TextRun {
            len: shown.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let Some(marked) = self.marked.clone().filter(|_| !empty) else {
            return vec![run];
        };
        [
            TextRun {
                len: marked.start,
                ..run.clone()
            },
            TextRun {
                len: marked.end - marked.start,
                underline: Some(UnderlineStyle {
                    color: Some(color),
                    thickness: px(1.0),
                    wavy: false,
                }),
                ..run.clone()
            },
            TextRun {
                len: shown.len() - marked.end,
                ..run
            },
        ]
        .into_iter()
        .filter(|run| run.len > 0)
        .collect()
    }
}

impl IntoElement for DraftField {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Lays out `text` at `wrap_width` and returns its lines and the top of each.
fn shape(
    text: &SharedString,
    runs: &[TextRun],
    font_size: Pixels,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    window: &Window,
) -> (Vec<WrappedLine>, Vec<Pixels>, Pixels) {
    let lines: Vec<WrappedLine> = window
        .text_system()
        .shape_text(text.clone(), font_size, runs, wrap_width, None)
        .map(|lines| lines.into_vec())
        .unwrap_or_default();
    let mut tops = Vec::with_capacity(lines.len());
    let mut height = px(0.0);
    for line in &lines {
        tops.push(height);
        height += line.size(line_height).height;
    }
    (lines, tops, height.max(line_height))
}

impl Element for DraftField {
    type RequestLayoutState = ();
    type PrepaintState = DraftPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let text = self.display_text();
        let runs = self.runs(window);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let max_height = line_height * MAX_VISIBLE_LINES;
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, _cx| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let (_, _, height) = shape(&text, &runs, font_size, line_height, width, window);
                size(width.unwrap_or_default(), height.min(max_height))
            });
        let _ = cx;
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let text = self.display_text();
        let runs = self.runs(window);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let (lines, tops, content_height) = shape(
            &text,
            &runs,
            font_size,
            line_height,
            Some(bounds.size.width),
            window,
        );
        let mut starts = Vec::with_capacity(lines.len());
        let mut start = 0;
        for line in &lines {
            starts.push(start);
            start += line.len() + 1;
        }
        let empty = self.text.is_empty();

        // Scroll so the caret's line stays in view.
        let mut layout = DraftLayout {
            lines,
            starts,
            tops,
            origin: bounds.origin,
            line_height,
            empty,
        };
        let head = layout.position_for_offset(self.selection.head());
        let caret_top = head.y - bounds.origin.y;
        let max_scroll = (content_height - bounds.size.height).max(px(0.0));
        let mut scroll_y = self.scroll_y.min(max_scroll);
        if caret_top < scroll_y {
            scroll_y = caret_top;
        } else if caret_top + line_height > scroll_y + bounds.size.height {
            scroll_y = caret_top + line_height - bounds.size.height;
        }
        scroll_y = scroll_y.clamp(px(0.0), max_scroll);
        layout.origin = point(bounds.origin.x, bounds.origin.y - scroll_y);

        let selections = if empty || self.selection.is_empty() {
            Vec::new()
        } else {
            selection_quads(&layout, &self.selection.range, bounds.size.width)
        };
        let caret = (self.show_caret && (self.selection.is_empty() || empty)).then(|| {
            let at = layout.position_for_offset(if empty { 0 } else { self.selection.head() });
            // A little shorter than the line, centered, like native carets.
            let inset = (line_height * 0.12).round();
            fill(
                Bounds::new(
                    point(at.x, at.y + inset),
                    size(CARET_WIDTH, line_height - inset * 2.0),
                ),
                Hsla::from(colors().accent),
            )
        });
        DraftPrepaint {
            layout,
            selections,
            caret,
            scroll_y,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.enabled {
            let focus_handle = self.view.read(cx).focus_handle.clone();
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.view.clone()),
                cx,
            );
        }
        let layout = &prepaint.layout;
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in prepaint.selections.drain(..) {
                window.paint_quad(quad);
            }
            for (line, top) in layout.lines.iter().zip(&layout.tops) {
                let origin = point(layout.origin.x, layout.origin.y + *top);
                let _ = line.paint(
                    origin,
                    layout.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if let Some(caret) = prepaint.caret.take() {
                window.paint_quad(caret);
            }
        });
        let layout = prepaint.layout.clone();
        let scroll_y = prepaint.scroll_y;
        self.view.update(cx, |view, _| {
            view.composer.editor.layout = Some(layout);
            view.composer.editor.scroll_y = scroll_y;
        });
    }
}

/// One quad per visual row the selection covers. An empty line inside the
/// selection still shows a sliver, so selected line breaks are visible.
fn selection_quads(layout: &DraftLayout, range: &Range<usize>, width: Pixels) -> Vec<PaintQuad> {
    let color = Rgba {
        a: 0.3,
        ..colors().accent
    };
    let line_height = layout.line_height;
    let mut quads = Vec::new();
    let mut push = |left: Pixels, top: Pixels, right: Pixels| {
        let right = right.max(left + px(4.0));
        quads.push(fill(
            Bounds::from_corners(point(left, top), point(right, top + line_height)),
            Hsla::from(color),
        ));
    };
    for (line, start) in layout.starts.iter().enumerate() {
        let end = start + layout.lines[line].len();
        if range.end < *start || range.start > end {
            continue;
        }
        let from = layout.position_for_offset(range.start.max(*start));
        let to = layout.position_for_offset(range.end.min(end));
        let left_edge = layout.origin.x;
        let right_edge = layout.origin.x + width;
        if from.y == to.y {
            push(from.x, from.y, to.x);
        } else {
            push(from.x, from.y, right_edge);
            let mut top = from.y + line_height;
            while top < to.y {
                push(left_edge, top, right_edge);
                top += line_height;
            }
            push(left_edge, to.y, to.x);
        }
    }
    quads
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    fn keystroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: None,
        }
    }

    fn selected(range: Range<usize>, reversed: bool) -> Selection {
        Selection { range, reversed }
    }

    #[test]
    fn typing_replaces_the_selection_and_leaves_the_caret_after_it() {
        let mut text = "hello world".to_owned();
        let mut selection = selected(6..11, false);
        replace_selection(&mut text, &mut selection, "there");
        assert_eq!(text, "hello there");
        assert_eq!(selection, Selection::caret(11));

        let mut selection = Selection::caret(0);
        replace_selection(&mut text, &mut selection, "¡");
        assert_eq!(text, "¡hello there");
        assert_eq!(selection, Selection::caret("¡".len()));
    }

    #[test]
    fn arrows_move_by_character_word_and_line() {
        let text = "uno dos\ntres";
        let mut selection = Selection::caret(5);
        apply_motion(text, &mut selection, Motion::CharLeft, false);
        assert_eq!(selection.head(), 4);
        apply_motion(text, &mut selection, Motion::WordLeft, false);
        assert_eq!(selection.head(), 0);
        apply_motion(text, &mut selection, Motion::WordRight, false);
        assert_eq!(selection.head(), 3);
        apply_motion(text, &mut selection, Motion::LineEnd, false);
        assert_eq!(selection.head(), 7);
        apply_motion(text, &mut selection, Motion::CharRight, false);
        assert_eq!(selection.head(), 8);
        apply_motion(text, &mut selection, Motion::LineEnd, false);
        assert_eq!(selection.head(), text.len());
        apply_motion(text, &mut selection, Motion::LineStart, false);
        assert_eq!(selection.head(), 8);
    }

    #[test]
    fn moves_step_over_whole_characters() {
        let text = "año";
        let mut selection = Selection::caret(1);
        apply_motion(text, &mut selection, Motion::CharRight, false);
        assert_eq!(selection.head(), 3);
        apply_motion(text, &mut selection, Motion::CharLeft, false);
        assert_eq!(selection.head(), 1);
    }

    #[test]
    fn shift_extends_and_a_plain_arrow_collapses_to_that_side() {
        let text = "abcdef";
        let mut selection = Selection::caret(3);
        apply_motion(text, &mut selection, Motion::CharLeft, true);
        apply_motion(text, &mut selection, Motion::CharLeft, true);
        assert_eq!(selection, selected(1..3, true));
        apply_motion(text, &mut selection, Motion::CharRight, true);
        apply_motion(text, &mut selection, Motion::CharRight, true);
        apply_motion(text, &mut selection, Motion::CharRight, true);
        assert_eq!(selection, selected(3..4, false));
        let mut wide = selected(1..4, false);
        apply_motion(text, &mut wide, Motion::CharLeft, false);
        assert_eq!(wide, Selection::caret(1));
        let mut wide = selected(1..4, true);
        apply_motion(text, &mut wide, Motion::CharRight, false);
        assert_eq!(wide, Selection::caret(4));
    }

    #[test]
    fn deleting_removes_the_selection_or_up_to_the_motion() {
        let mut text = "one two three".to_owned();
        let mut selection = Selection::caret(text.len());
        delete(&mut text, &mut selection, Motion::WordLeft);
        assert_eq!(text, "one two ");
        delete(&mut text, &mut selection, Motion::CharLeft);
        assert_eq!(text, "one two");
        let mut selection = selected(0..4, false);
        delete(&mut text, &mut selection, Motion::CharLeft);
        assert_eq!(text, "two");
        assert_eq!(selection, Selection::caret(0));
        delete(&mut text, &mut selection, Motion::CharRight);
        assert_eq!(text, "wo");
        let mut selection = Selection::caret(1);
        delete(&mut text, &mut selection, Motion::LineStart);
        assert_eq!(text, "o");
    }

    #[test]
    fn double_and_triple_clicks_pick_a_word_and_a_line() {
        let text = "fix the_parser now\nnext";
        assert_eq!(word_range(text, 9), 4..14);
        assert_eq!(word_range(text, 4), 4..14);
        assert_eq!(word_range(text, 0), 0..3);
        assert_eq!(word_range(text, 3), 0..3);
        assert_eq!(line_range(text, 5), 0..19);
        assert_eq!(line_range(text, 21), 19..text.len());
        assert_eq!(word_range("a  b", 2), 2..3);
    }

    #[test]
    fn utf16_offsets_round_trip_through_wide_characters() {
        let text = "a😀b";
        assert_eq!(offset_to_utf16(text, 5), 3);
        assert_eq!(offset_from_utf16(text, 3), 5);
        assert_eq!(range_from_utf16(text, &(1..3)), 1..5);
        assert_eq!(offset_from_utf16(text, 99), text.len());
    }

    #[test]
    fn keys_map_to_macos_text_editing() {
        let plain = Modifiers::none();
        assert_eq!(
            key_action(&keystroke("enter", plain)),
            Some(KeyAction::Send)
        );
        assert_eq!(
            key_action(&keystroke("enter", Modifiers::shift())),
            Some(KeyAction::Newline)
        );
        assert_eq!(
            key_action(&keystroke("left", Modifiers::alt())),
            Some(KeyAction::Move {
                motion: Motion::WordLeft,
                extend: false
            })
        );
        assert_eq!(
            key_action(&keystroke("right", Modifiers::command_shift())),
            Some(KeyAction::Move {
                motion: Motion::LineEnd,
                extend: true
            })
        );
        assert_eq!(
            key_action(&keystroke("backspace", Modifiers::command())),
            Some(KeyAction::Delete(Motion::LineStart))
        );
        assert_eq!(
            key_action(&keystroke("a", Modifiers::command())),
            Some(KeyAction::SelectAll)
        );
        assert_eq!(
            key_action(&keystroke("up", plain)),
            Some(KeyAction::Vertical {
                down: false,
                extend: false
            })
        );
        // Printable keys go to the input handler; app shortcuts pass through.
        assert_eq!(key_action(&keystroke("h", plain)), None);
        assert_eq!(key_action(&keystroke("w", Modifiers::command())), None);
    }

    #[test]
    fn an_outside_change_moves_the_caret_to_the_end() {
        let mut editor = EditorState::default();
        editor.sync("hola");
        editor.selection = Selection::caret(1);
        editor.sync("hola");
        assert_eq!(editor.selection, Selection::caret(1));
        assert!(!editor.caret_at_end("hola"));
        editor.sync("hola mundo");
        assert_eq!(editor.selection, Selection::caret(10));
        assert!(editor.caret_at_end("hola mundo"));
    }
}
