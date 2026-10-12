//! The composer at the bottom of the chat: a quiet card with the draft,
//! attachments, the `/` command and `@` file popups, the model and mode menus,
//! and the context meter, with a footer line below it (see `footer.rs`).
//! Enter sends, or queues the draft while a turn runs. Shift+Enter adds a line,
//! and Escape closes a menu or popup, or stops a running turn when the draft is empty.
//! The draft field itself (caret, selection, IME) is `editor.rs`.

mod approval;
mod editor;
mod files;
mod footer;
mod meter;
mod picker;
mod picker_view;
mod popups;
mod queue;

pub(super) use queue::QueuedPrompt;

use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    AnyElement, Context, CursorStyle, KeyDownEvent, MouseButton, Rgba, ScrollHandle, SharedString,
    Window, div, prelude::*, px, svg,
};

use super::AgentChatView;
use super::tooltip::tooltip;
use crate::ui::theme::{colors, surface_tint};
use editor::EditorKey;
use meter::context_meter;
use picker::{MenuKey, OpenMenu, mode_label};

/// Shown in the draft field while it is empty and no turn runs.
const PLACEHOLDER: &str = "Ask for changes, / for commands, @ for files…";

/// Shown in the draft field while a turn runs; Enter queues what is typed.
const QUEUE_PLACEHOLDER: &str = "Queue a follow-up…";

/// Popup, menu and file-index state for the composer.
#[derive(Debug, Default)]
pub(super) struct ComposerState {
    /// Highlighted row of the open `/` or `@` popup.
    selection: usize,
    /// Set by Escape or a press outside the popup; the next edit reopens it.
    popup_dismissed: bool,
    /// The model or mode menu that is open.
    menu: Option<OpenMenu>,
    /// Scroll position of the open menu's list, so its highlight stays in view.
    menu_scroll: ScrollHandle,
    /// Project files relative to `files_root`, from the last completed scan.
    files: Vec<String>,
    files_root: Option<PathBuf>,
    files_scanned_at: Option<Instant>,
    /// A scan is running; a new one starts only after it finishes.
    files_scanning: bool,
    /// Caret, selection and IME state of the draft field.
    editor: editor::EditorState,
}

/// The keys an open model or mode menu uses. Other keys keep editing the draft.
fn menu_key(key: &str) -> Option<MenuKey> {
    Some(match key {
        "up" => MenuKey::Up,
        "down" => MenuKey::Down,
        "left" => MenuKey::Left,
        "right" => MenuKey::Right,
        "enter" | "return" => MenuKey::Accept,
        "escape" | "esc" => MenuKey::Close,
        _ => return None,
    })
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
            return;
        }
        // An open menu takes its keys first, so Escape closes it before a popup.
        if let Some(menu_key) = menu_key(key).filter(|_| self.composer.menu.is_some()) {
            self.handle_menu_key(menu_key, cx);
            cx.stop_propagation();
            return;
        }
        if self.handle_popup_key(key, &event.keystroke.modifiers, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        match self.editor_key(&event.keystroke, cx) {
            EditorKey::Handled => {}
            EditorKey::Send => self.submit_draft(cx),
            EditorKey::Cancel => {
                // With text in the draft, Escape leaves the turn running so the text can still be queued.
                if !self.is_working() || !self.draft.trim().is_empty() {
                    return;
                }
                self.cancel_turn(cx);
            }
            // Printable keys go on to the input handler, which inserts them.
            EditorKey::Ignored => return,
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
            .rounded(px(20.0))
            .border_1()
            .border_color(card_border(focused && enabled))
            .bg(surface_tint(colors().elevated, colors().terminal));
        if let Some(header) = self.composer_header() {
            card = card.child(header);
        }
        if let Some(chips) = self.attachment_chips(cx) {
            card = card.child(chips);
        }
        card = card
            .child(self.composer_input(enabled, focused, cx))
            .child(self.composer_toolbar(enabled, cx));
        if let Some(popup) = &popup {
            card = card.child(self.popup_panel(popup, cx));
        }

        let mut outer = div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(16.0))
            .pt(px(8.0))
            .pb(px(10.0))
            // Attached above the card: prompts waiting for the turn to end, then
            // the permission request that needs an answer now.
            .children(self.queue_strip(cx))
            .children(self.approval_dock(cx))
            .child(card)
            .child(self.composer_footer(cx));
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

    /// The branch the pane's directory is on, quiet, above the draft. Hidden
    /// when there is no branch.
    fn composer_header(&self) -> Option<AnyElement> {
        let branch = self.context.branch.clone()?;
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .min_w(px(0.0))
                .px(px(14.0))
                .pt(px(10.0))
                .text_size(px(11.0))
                .text_color(colors().subtle)
                .child(
                    svg()
                        .path("chrome-icons/git-branch.svg")
                        .size(px(11.0))
                        .flex_none()
                        .text_color(colors().subtle),
                )
                .child(div().min_w(px(0.0)).truncate().child(branch))
                .when(self.context.worktree, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .h(px(16.0))
                            .px(px(5.0))
                            .flex()
                            .items_center()
                            .rounded(px(4.0))
                            .bg(colors().hover)
                            .child("Worktree"),
                    )
                })
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
                .px(px(14.0))
                .pt(px(10.0))
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

    /// The draft field. A press places the caret and a drag selects; the text
    /// itself, the selection and the caret are drawn by `editor::DraftField`.
    fn composer_input(&self, enabled: bool, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("agent-chat-draft")
            .flex_none()
            .min_h(px(44.0))
            .px(px(14.0))
            .pt(px(10.0))
            .pb(px(4.0))
            .text_size(px(13.0))
            .line_height(px(20.0))
            .text_color(if enabled {
                colors().foreground
            } else {
                colors().subtle
            })
            .when(enabled, |field| field.cursor(CursorStyle::IBeam))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| this.editor_mouse_down(event, cx)),
            )
            .on_mouse_move(cx.listener(|this, event, _, cx| this.editor_mouse_move(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.editor_mouse_up()),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.editor_mouse_up()),
            )
            .child(self.draft_field(enabled, focused, cx))
            .into_any_element()
    }

    /// What the empty draft field says: what to ask, or that typing queues.
    fn composer_placeholder(&self) -> &'static str {
        if self.is_working() {
            QUEUE_PLACEHOLDER
        } else {
            PLACEHOLDER
        }
    }

    /// The row under the draft: "+", the model and mode triggers, the context
    /// meter, and the send or stop button.
    fn composer_toolbar(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        // Switching model or mode would change the running turn, so the menus wait for it to end.
        let pickers = self.is_idle();
        let mut toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(8.0))
            .pb(px(8.0))
            .child(self.mention_button(enabled, cx))
            .child(self.model_trigger(pickers, cx));
        if let Some(label) = mode_label(self.timeline.modes.as_ref(), &self.timeline.config_options)
        {
            toolbar = toolbar.child(self.mode_trigger(label, pickers, cx));
        }
        toolbar
            .child(div().flex_1())
            .when_some(context_meter(self.timeline.usage), |row, meter| {
                row.child(meter)
            })
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
            .rounded(px(13.0))
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

    /// The round button at the end of the toolbar. Idle it sends the draft; while
    /// a turn runs it queues a typed draft, and otherwise it stops the turn.
    fn action_button(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let has_text = !self.draft.trim().is_empty();
        if self.is_working() && !has_text {
            return div()
                .id("agent-chat-stop")
                .flex_none()
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(14.0))
                .bg(colors().danger)
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
        let queueing = self.is_working();
        let can_send = enabled && has_text;
        div()
            .id(if queueing {
                "agent-chat-queue"
            } else {
                "agent-chat-send"
            })
            .flex_none()
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(14.0))
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
            .tooltip(move |_, cx| {
                tooltip(
                    if queueing {
                        "Queue message · ↵"
                    } else {
                        "Send · ↵"
                    },
                    cx,
                )
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                svg()
                    .path(if queueing {
                        "chrome-icons/list-plus.svg"
                    } else {
                        "chrome-icons/arrow-up.svg"
                    })
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

    /// Whether the draft accepts typing: the session is open and either idle or
    /// running a turn. Typed text is queued or sent, so it can be edited while a turn runs.
    pub(super) fn accepts_input(&self) -> bool {
        self.is_idle() || self.is_working()
    }
}

/// The card's edge is very soft; it turns to the accent while the draft has focus.
fn card_border(focused: bool) -> Rgba {
    if focused {
        Rgba {
            a: 0.45,
            ..colors().accent
        }
    } else {
        let border = colors().border_subtle;
        Rgba {
            a: border.a * 0.6,
            ..border
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::picker::PickerMenu;
    use super::*;
    use crate::domain::agent_chat::{
        ChatAgent, ChatCommand, ChatConnectionState, ConfigCategory, ConfigOption, ConfigValue,
        PromptFile, SlashCommand,
    };
    use crate::ports::files::{FileEntry, FileSystemPort};
    use crate::ui::agent_chat::FileSearch;
    use gpui::{
        Entity, EntityInputHandler, Keystroke, Modifiers, TestAppContext, VisualTestContext,
    };

    fn key_event(key: &str, key_char: Option<&str>) -> KeyDownEvent {
        key_with(key, key_char, Modifiers::none())
    }

    fn key_with(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers,
                key: key.into(),
                key_char: key_char.map(Into::into),
            },
            is_held: false,
        }
    }

    /// Types `text` one character at a time, as macOS does: the key goes
    /// unhandled through `on_key_down`, then the input handler inserts it.
    fn type_text(
        view: &mut AgentChatView,
        window: &mut Window,
        cx: &mut Context<AgentChatView>,
        text: &str,
    ) {
        for character in text.chars() {
            let key = character.to_string();
            view.on_key_down(&key_event(&key, Some(&key)), window, cx);
            view.replace_text_in_range(None, &key, window, cx);
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

    /// A running Claude with a Model option of two values.
    fn model_option(current: &str) -> ConfigOption {
        ConfigOption {
            id: "model".into(),
            name: "Model".into(),
            category: ConfigCategory::Model,
            current: current.into(),
            values: vec![
                ConfigValue {
                    value: "opus".into(),
                    name: "Opus 5.5".into(),
                    description: None,
                },
                ConfigValue {
                    value: "sonnet".into(),
                    name: "Sonnet".into(),
                    description: None,
                },
            ],
        }
    }

    /// A chat that is ready for input.
    fn ready_view(cx: &mut TestAppContext) -> (Entity<AgentChatView>, &mut VisualTestContext) {
        cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Ready);
            view
        })
    }

    #[gpui::test]
    fn keys_edit_the_draft_at_the_caret(cx: &mut TestAppContext) {
        let (view, cx) = ready_view(cx);
        view.update_in(cx, |view, window, cx| {
            type_text(view, window, cx, "hola");
            view.on_key_down(&key_event("left", None), window, cx);
            view.on_key_down(&key_event("left", None), window, cx);
            type_text(view, window, cx, "X");
            assert_eq!(view.draft, "hoXla");
            view.on_key_down(&key_with("enter", None, Modifiers::shift()), window, cx);
            assert_eq!(view.draft, "hoX\nla");
            view.on_key_down(&key_event("backspace", None), window, cx);
            assert_eq!(view.draft, "hoXla");
            // Option-Backspace deletes back to the start of the word.
            view.on_key_down(&key_with("backspace", None, Modifiers::alt()), window, cx);
            assert_eq!(view.draft, "la");
        });
    }

    #[gpui::test]
    fn paste_replaces_the_selection_and_keeps_line_breaks(cx: &mut TestAppContext) {
        let (view, cx) = ready_view(cx);
        view.update_in(cx, |view, window, cx| {
            type_text(view, window, cx, "old text");
            view.on_key_down(&key_with("a", None, Modifiers::command()), window, cx);
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("pegado\r\nlínea".into()));
            view.on_key_down(&key_with("v", None, Modifiers::command()), window, cx);
            assert_eq!(view.draft, "pegado\nlínea");
        });
    }

    #[gpui::test]
    fn enter_sends_and_shortcuts_pass_through(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (view, cx) = ready_view(cx);
        view.update_in(cx, |view, window, cx| {
            view.commands = Some(commands_tx);
            type_text(view, window, cx, "borrador");
            // ⌘W is the app's; the composer leaves it alone.
            view.on_key_down(&key_with("w", Some("w"), Modifiers::command()), window, cx);
            assert_eq!(view.draft, "borrador");
            view.on_key_down(&key_event("enter", None), window, cx);
            assert!(view.draft.is_empty());
        });
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(ChatCommand::Prompt { text, .. }) if text == "borrador"
        ));
    }

    #[test]
    fn menu_keys_are_the_arrows_enter_and_escape() {
        assert_eq!(menu_key("up"), Some(MenuKey::Up));
        assert_eq!(menu_key("down"), Some(MenuKey::Down));
        assert_eq!(menu_key("left"), Some(MenuKey::Left));
        assert_eq!(menu_key("right"), Some(MenuKey::Right));
        assert_eq!(menu_key("return"), Some(MenuKey::Accept));
        assert_eq!(menu_key("esc"), Some(MenuKey::Close));
        assert_eq!(menu_key("h"), None);
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

    #[gpui::test]
    fn model_menu_keys_pick_a_model_of_the_running_agent(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Ready);
            view.timeline.config_options = vec![model_option("opus")];
            view.commands = Some(commands_tx);
            view
        });
        view.update_in(cx, |view, window, cx| {
            view.toggle_menu(PickerMenu::Model, cx);
            view.on_key_down(&key_event("down", None), window, cx);
            view.on_key_down(&key_event("enter", None), window, cx);
        });
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(ChatCommand::SetConfigOption { option_id, value })
                if option_id == "model" && value == "sonnet"
        ));
        view.read_with(cx, |view, _| assert!(view.composer.menu.is_none()));
    }

    #[gpui::test]
    fn locked_chat_keeps_the_arrow_keys_on_its_own_provider(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Ready);
            view.timeline.config_options = vec![model_option("opus")];
            view.available_agents = vec![ChatAgent::Claude, ChatAgent::Codex];
            // A sent message locks the provider; the turn it starts is not what this test checks.
            view.timeline.push_user_prompt("hola".into(), Vec::new());
            view.timeline.state = Some(ChatConnectionState::Ready);
            view.commands = Some(commands_tx);
            view
        });
        view.update_in(cx, |view, window, cx| {
            view.toggle_menu(PickerMenu::Model, cx);
            view.on_key_down(&key_event("right", None), window, cx);
            view.on_key_down(&key_event("enter", None), window, cx);
        });
        // Codex is disabled once the chat has a message, so Enter picks Claude's own row.
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(ChatCommand::SetConfigOption { value, .. }) if value == "opus"
        ));
    }

    #[gpui::test]
    fn typing_is_accepted_while_a_turn_runs(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Working);
            view.commands = Some(commands_tx);
            view
        });
        view.update_in(cx, |view, window, cx| type_text(view, window, cx, "next"));
        view.read_with(cx, |view, _| assert_eq!(view.draft, "next"));
        assert!(commands_rx.try_recv().is_err());
    }

    #[gpui::test]
    fn enter_while_a_turn_runs_queues_the_draft_with_its_files(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Working);
            view.commands = Some(commands_tx);
            view
        });
        view.update_in(cx, |view, window, cx| {
            type_text(view, window, cx, "later");
            view.attachments.push(PromptFile {
                path: "/repo/README.md".into(),
                label: "README.md".into(),
            });
            view.on_key_down(&key_event("enter", None), window, cx);
        });
        assert!(commands_rx.try_recv().is_err());
        view.read_with(cx, |view, _| {
            assert!(view.draft.is_empty());
            assert!(view.attachments.is_empty());
            assert_eq!(
                view.queued,
                vec![QueuedPrompt {
                    text: "later".into(),
                    files: vec![PromptFile {
                        path: "/repo/README.md".into(),
                        label: "README.md".into(),
                    }],
                }]
            );
        });
    }

    #[gpui::test]
    fn escape_stops_a_running_turn_only_when_the_draft_is_empty(cx: &mut TestAppContext) {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Working);
            view.commands = Some(commands_tx);
            view
        });
        view.update_in(cx, |view, window, cx| {
            type_text(view, window, cx, "x");
            view.on_key_down(&key_event("escape", None), window, cx);
        });
        assert!(commands_rx.try_recv().is_err());
        view.update_in(cx, |view, window, cx| {
            view.draft.clear();
            view.on_key_down(&key_event("escape", None), window, cx);
        });
        assert!(matches!(commands_rx.try_recv(), Ok(ChatCommand::Cancel)));
    }
}
