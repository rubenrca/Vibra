//! The composer at the bottom of the chat: a card with the draft, attachments,
//! `/` command and `@` file popups, model and mode pickers, and the context
//! meter, with the quota line below it. Enter sends, Shift+Enter adds a line,
//! and Escape closes a popup or stops a running turn.

mod files;
mod meter;
mod picker;
mod popups;

use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    AnyElement, Context, KeyDownEvent, Modifiers, MouseButton, SharedString, Window, div,
    prelude::*, px, svg,
};

use super::AgentChatView;
use super::tooltip::tooltip;
use crate::domain::agent_chat::ChatConnectionState;
use crate::ui::text_edit::{TextKeyOutcome, apply_text_key};
use crate::ui::theme::{colors, surface_tint};
use meter::{context_meter, quota_line, working_spinner};
use picker::{PickerMenu, mode_label, model_label};

/// Shown in the draft field while it is empty.
const PLACEHOLDER: &str = "Ask, build, / for commands, @ for references…";

/// Popup, menu and file-index state for the composer.
#[derive(Debug, Default)]
pub(super) struct ComposerState {
    /// Highlighted row of the open `/` or `@` popup.
    selection: usize,
    /// Set by Escape or a press outside the popup; the next edit reopens it.
    popup_dismissed: bool,
    /// The model or mode menu that is open.
    menu: Option<PickerMenu>,
    /// Project files relative to `files_root`, from the last completed scan.
    files: Vec<String>,
    files_root: Option<PathBuf>,
    files_scanned_at: Option<Instant>,
    /// A scan is running; a new one starts only after it finishes.
    files_scanning: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComposerKey {
    /// The draft changed.
    Edited,
    /// Enter: send the draft.
    Send,
    /// Escape: stop the running turn, if any.
    Cancel,
    /// Not a composer key; let it propagate.
    Ignored,
}

/// Maps a key press onto the draft. Plain Enter sends; every other editing key
/// goes through the shared text editing rules (multiline, so Shift+Enter adds
/// a newline and paste keeps line breaks).
pub(super) fn handle_key(
    draft: &mut String,
    key: &str,
    key_char: Option<&str>,
    modifiers: &Modifiers,
    paste: impl FnOnce() -> Option<String>,
) -> ComposerKey {
    if key == "escape" || key == "esc" {
        return ComposerKey::Cancel;
    }
    if matches!(key, "enter" | "return") && !modifiers.shift {
        return ComposerKey::Send;
    }
    match apply_text_key(draft, key, key_char, modifiers, true, paste) {
        TextKeyOutcome::Edited => ComposerKey::Edited,
        TextKeyOutcome::Submit => ComposerKey::Send,
        TextKeyOutcome::Cancel => ComposerKey::Cancel,
        TextKeyOutcome::NextField | TextKeyOutcome::PreviousField | TextKeyOutcome::Unhandled => {
            ComposerKey::Ignored
        }
    }
}

impl AgentChatView {
    pub(super) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if !self.accepts_input() {
            // Escape still stops a turn while the composer is locked.
            if matches!(key, "escape" | "esc") && self.is_working() {
                self.cancel_turn(cx);
                cx.stop_propagation();
            }
            return;
        }
        // Escape closes the innermost open thing first: a menu, then a popup.
        if self.composer.menu.is_some() && matches!(key, "escape" | "esc") {
            self.composer.menu = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.handle_popup_key(key, &event.keystroke.modifiers, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let outcome = handle_key(
            &mut self.draft,
            key,
            event.keystroke.key_char.as_deref(),
            &event.keystroke.modifiers,
            || cx.read_from_clipboard().and_then(|item| item.text()),
        );
        match outcome {
            ComposerKey::Edited => self.after_edit(cx),
            ComposerKey::Send => self.submit_draft(cx),
            ComposerKey::Cancel => {
                if !self.is_working() {
                    return;
                }
                self.cancel_turn(cx);
            }
            ComposerKey::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Runs after a key edits the draft: a dismissed popup reopens and starts
    /// at its first row, and an `@` reference refreshes the file list.
    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.composer.popup_dismissed = false;
        self.composer.selection = 0;
        self.refresh_file_index_if_mentioning(cx);
    }

    pub(super) fn composer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.accepts_input();
        let focused = self.focus_handle.is_focused(window);
        let popup = self.active_popup();
        let menu_open = enabled && self.composer.menu.is_some();

        let mut card = div()
            .relative()
            .flex()
            .flex_col()
            .rounded(px(12.0))
            .border_1()
            .border_color(if focused && enabled {
                colors().accent
            } else {
                colors().border_subtle
            })
            .bg(surface_tint(colors().elevated, colors().terminal));
        if let Some(header) = self.composer_header() {
            card = card.child(header);
        }
        if let Some(chips) = self.attachment_chips(cx) {
            card = card.child(chips);
        }
        card = card.child(self.composer_input(enabled, focused));
        if let Some(meter) = context_meter(self.timeline.usage) {
            card = card.child(meter);
        }
        card = card.child(self.composer_toolbar(enabled, cx));
        if let Some(popup) = &popup {
            card = card.child(self.popup_panel(popup, cx));
        }

        let mut outer = div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(12.0))
            .pt(px(8.0))
            .pb(px(12.0))
            .child(card);
        if let Some(line) = quota_line(&self.context.quota) {
            outer = outer.child(line);
        }
        if menu_open {
            // A press outside the open menu closes it. The menu is deferred above this layer.
            outer = outer.child(div().absolute().inset_0().occlude().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.composer.menu = None;
                    cx.notify();
                }),
            ));
        }
        outer.into_any_element()
    }

    /// Worktree or branch on the left and the working spinner on the right.
    /// `None` when there is nothing to show.
    fn composer_header(&self) -> Option<AnyElement> {
        let context = &self.context;
        let working = self.is_working();
        if !context.worktree && context.branch.is_none() && !working {
            return None;
        }
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .pt(px(8.0))
                .min_w(px(0.0))
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .when(context.worktree, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .h(px(18.0))
                            .px(px(6.0))
                            .flex()
                            .items_center()
                            .rounded(px(5.0))
                            .bg(colors().hover)
                            .child("⎇ Worktree"),
                    )
                })
                .when_some(context.branch.clone(), |row, branch| {
                    row.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .min_w(px(0.0))
                            .child(
                                svg()
                                    .path("chrome-icons/git-branch.svg")
                                    .size(px(11.0))
                                    .flex_none()
                                    .text_color(colors().subtle),
                            )
                            .child(div().min_w(px(0.0)).truncate().child(branch)),
                    )
                })
                .child(div().flex_1())
                .when(working, |row| row.child(working_spinner()))
                .into_any_element(),
        )
    }

    /// Attached files, each with a button to remove it before sending.
    fn attachment_chips(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.attachments.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .px(px(12.0))
                .pt(px(8.0))
                .children(self.attachments.iter().enumerate().map(|(index, file)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .max_w(px(240.0))
                        .h(px(22.0))
                        .pl(px(6.0))
                        .pr(px(2.0))
                        .rounded(px(6.0))
                        .bg(colors().hover)
                        .text_size(px(11.5))
                        .text_color(colors().foreground)
                        .child(
                            svg()
                                .path("chrome-icons/files.svg")
                                .size(px(12.0))
                                .flex_none()
                                .text_color(colors().muted),
                        )
                        .child(div().min_w(px(0.0)).truncate().child(file.label.clone()))
                        .child(
                            div()
                                .id(SharedString::from(format!("agent-chat-attachment-{index}")))
                                .flex_none()
                                .size(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|button| button.bg(colors().border_subtle))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if index < this.attachments.len() {
                                        this.attachments.remove(index);
                                    }
                                    cx.notify();
                                }))
                                .child(
                                    svg()
                                        .path("chrome-icons/close.svg")
                                        .size(px(10.0))
                                        .text_color(colors().muted),
                                ),
                        )
                        .into_any_element()
                }))
                .into_any_element(),
        )
    }

    fn composer_input(&self, enabled: bool, focused: bool) -> AnyElement {
        let lines: Vec<String> = self.draft.split('\n').map(str::to_owned).collect();
        let line_count = lines.len();
        div()
            .flex_none()
            .min_h(px(40.0))
            .max_h(px(160.0))
            .overflow_hidden()
            .px(px(12.0))
            .py(px(8.0))
            .text_size(px(13.0))
            .line_height(px(20.0))
            .text_color(if enabled {
                colors().foreground
            } else {
                colors().subtle
            })
            .when(self.draft.is_empty(), |text| {
                text.text_color(colors().subtle).child(PLACEHOLDER)
            })
            .when(!self.draft.is_empty(), |text| {
                text.children(lines.into_iter().enumerate().map(|(index, line)| {
                    let caret = focused && enabled && index + 1 == line_count;
                    let line = if caret { format!("{line}▏") } else { line };
                    div().min_h(px(20.0)).child(if line.is_empty() {
                        " ".to_owned()
                    } else {
                        line
                    })
                }))
            })
            .into_any_element()
    }

    /// The row under the draft: "+", the pickers, and the send or stop button.
    fn composer_toolbar(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let mut toolbar = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(8.0))
            .pb(px(8.0))
            .child(self.mention_button(enabled, cx));
        if let Some(label) = model_label(&self.timeline.config_options) {
            toolbar =
                toolbar.child(self.picker_button(PickerMenu::Model, label, None, enabled, cx));
        }
        if let Some(label) = mode_label(self.timeline.modes.as_ref(), &self.timeline.config_options)
        {
            toolbar = toolbar.child(self.picker_button(
                PickerMenu::Mode,
                label,
                Some("chrome-icons/sparkles.svg"),
                enabled,
                cx,
            ));
        }
        toolbar
            .child(div().flex_1())
            .child(self.action_button(enabled, cx))
            .into_any_element()
    }

    /// "+" inserts `@`, which opens the file popup.
    fn mention_button(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("agent-chat-add-reference")
            .flex_none()
            .size(px(26.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.0))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|button| button.bg(colors().hover))
                    .on_click(cx.listener(|this, _, _, cx| this.insert_mention(cx)))
            })
            .when(!enabled, |button| button.opacity(0.4))
            .tooltip(|_, cx| tooltip("Reference a file · @", cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                svg()
                    .path("chrome-icons/plus.svg")
                    .size(px(14.0))
                    .flex_none()
                    .text_color(colors().muted),
            )
            .into_any_element()
    }

    /// Stop while a turn runs; otherwise the send button.
    fn action_button(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        if self.is_working() {
            return div()
                .id("agent-chat-stop")
                .flex_none()
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.0))
                .bg(colors().foreground)
                .cursor_pointer()
                .hover(|button| button.opacity(0.85))
                .tooltip(|_, cx| tooltip("Stop · esc", cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| this.cancel_turn(cx)))
                .child(
                    div()
                        .size(px(10.0))
                        .rounded(px(2.0))
                        .bg(colors().background),
                )
                .into_any_element();
        }
        let can_send = enabled && !self.draft.trim().is_empty();
        div()
            .id("agent-chat-send")
            .flex_none()
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.0))
            .bg(if can_send {
                colors().foreground
            } else {
                colors().hover
            })
            .when(can_send, |button| {
                button
                    .cursor_pointer()
                    .hover(|button| button.opacity(0.85))
                    .on_click(cx.listener(|this, _, _, cx| this.submit_draft(cx)))
            })
            .when(!can_send, |button| button.opacity(0.6))
            .tooltip(|_, cx| tooltip("Send · ↵", cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                svg()
                    .path("chrome-icons/send.svg")
                    .size(px(14.0))
                    .flex_none()
                    .text_color(if can_send {
                        colors().background
                    } else {
                        colors().muted
                    }),
            )
            .into_any_element()
    }

    /// Whether the composer accepts input: the session is open and idle.
    pub(super) fn accepts_input(&self) -> bool {
        matches!(self.timeline.state, Some(ChatConnectionState::Ready))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::*;
    use crate::domain::agent_chat::{ChatAgent, PromptFile, SlashCommand};
    use crate::ports::files::{FileEntry, FileSystemPort};
    use crate::ui::agent_chat::FileSearch;
    use gpui::{Keystroke, TestAppContext};

    fn press(
        draft: &mut String,
        key: &str,
        key_char: Option<&str>,
        modifiers: Modifiers,
    ) -> ComposerKey {
        handle_key(draft, key, key_char, &modifiers, || {
            Some("pegado\r\nlínea".into())
        })
    }

    fn key_event(key: &str, key_char: Option<&str>) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers::none(),
                key: key.into(),
                key_char: key_char.map(Into::into),
            },
            is_held: false,
        }
    }

    /// Presses each character of `text` as a key.
    fn type_text(
        view: &mut AgentChatView,
        window: &mut Window,
        cx: &mut Context<AgentChatView>,
        text: &str,
    ) {
        for character in text.chars() {
            let key = character.to_string();
            view.on_key_down(&key_event(&key, Some(&key)), window, cx);
        }
    }

    /// A file source with a fixed list, for the `@` popup.
    struct FakeFiles(Vec<PathBuf>);

    impl FileSystemPort for FakeFiles {
        fn search_files(&self, _root: &Path, output: &mut Vec<PathBuf>) -> anyhow::Result<()> {
            output.extend(self.0.iter().cloned());
            Ok(())
        }

        fn read_text_file(&self, _project_root: &Path, _path: &Path) -> anyhow::Result<String> {
            anyhow::bail!("not used by composer tests")
        }

        fn create_entry(
            &self,
            _root: &Path,
            _directory: &Path,
            _name: &str,
            _folder: bool,
        ) -> anyhow::Result<PathBuf> {
            anyhow::bail!("not used by composer tests")
        }

        fn list_directory(
            &self,
            _project_root: &Path,
            _directory: &Path,
            _show_hidden: bool,
        ) -> anyhow::Result<Vec<FileEntry>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn enter_sends_and_shift_enter_adds_a_line() {
        let mut draft = "hola".to_owned();
        assert_eq!(
            press(&mut draft, "enter", None, Modifiers::none()),
            ComposerKey::Send
        );
        assert_eq!(draft, "hola");
        assert_eq!(
            press(&mut draft, "enter", None, Modifiers::shift()),
            ComposerKey::Edited
        );
        assert_eq!(draft, "hola\n");
    }

    #[test]
    fn typing_paste_and_deletion_edit_the_draft() {
        let mut draft = String::new();
        assert_eq!(
            press(&mut draft, "h", Some("h"), Modifiers::none()),
            ComposerKey::Edited
        );
        assert_eq!(
            press(&mut draft, "v", None, Modifiers::command()),
            ComposerKey::Edited
        );
        assert_eq!(draft, "hpegado\nlínea");
        assert_eq!(
            press(&mut draft, "backspace", None, Modifiers::alt()),
            ComposerKey::Edited
        );
        assert_eq!(draft, "hpegado\n");
    }

    #[test]
    fn escape_cancels_and_app_shortcuts_pass_through() {
        let mut draft = "borrador".to_owned();
        assert_eq!(
            press(&mut draft, "escape", None, Modifiers::none()),
            ComposerKey::Cancel
        );
        assert_eq!(
            press(&mut draft, "w", Some("w"), Modifiers::command()),
            ComposerKey::Ignored
        );
        assert_eq!(draft, "borrador");
    }

    #[gpui::test]
    fn slash_popup_completes_on_enter_instead_of_sending(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Ready);
            view.timeline.commands = vec![
                SlashCommand {
                    name: "compact".into(),
                    description: "Summarize the conversation".into(),
                    input_hint: None,
                },
                SlashCommand {
                    name: "clear".into(),
                    description: "Clear the context".into(),
                    input_hint: None,
                },
            ];
            view.commands = Some(commands_tx);
            view
        });
        view.update_in(cx, |view, window, cx| {
            type_text(view, window, cx, "/co");
            view.on_key_down(&key_event("enter", None), window, cx);
        });
        assert!(commands_rx.try_recv().is_err());
        view.read_with(cx, |view, _| {
            assert_eq!(view.draft, "/compact ");
            assert!(view.active_popup().is_none());
        });
    }

    #[gpui::test]
    fn at_popup_attaches_the_chosen_file_and_drops_the_mention(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Ready);
            view.file_search = Some(FileSearch {
                port: Arc::new(FakeFiles(vec![
                    PathBuf::from("/repo/src/main.rs"),
                    PathBuf::from("/repo/README.md"),
                ])),
                root: PathBuf::from("/repo"),
            });
            view
        });
        view.update_in(cx, |view, window, cx| {
            type_text(view, window, cx, "see @ma");
        });
        // The first scan runs in the background; the popup fills once it finishes.
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            view.on_key_down(&key_event("enter", None), window, cx);
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.draft, "see ");
            assert_eq!(
                view.attachments,
                vec![PromptFile {
                    path: "/repo/src/main.rs".into(),
                    label: "src/main.rs".into(),
                }]
            );
        });
    }
}
