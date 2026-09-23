use super::*;
use crate::domain::agents::AgentRuntimeState;
use crate::ports::terminal::TerminalAgentKindSource;
use anyhow::Result;
use async_channel::{Receiver, Sender};
use gpui::{KeyBinding, Modifiers, TestAppContext};
use std::{cell::RefCell, rc::Rc};

struct MockTerminalPort {
    handle: Arc<MockTerminalHandle>,
}

struct MockTerminalHandle {
    events: Receiver<TerminalEvent>,
    _events_tx: Sender<TerminalEvent>,
    inputs: Mutex<Vec<Vec<u8>>>,
    reject_input: Mutex<bool>,
    resizes: Mutex<Vec<TerminalSize>>,
    mode: Mutex<TerminalInputMode>,
    foreground: Mutex<Option<u32>>,
}

impl MockTerminalPort {
    fn new() -> Self {
        let (events_tx, events) = async_channel::unbounded();
        Self {
            handle: Arc::new(MockTerminalHandle {
                events,
                _events_tx: events_tx,
                inputs: Mutex::new(Vec::new()),
                reject_input: Mutex::new(false),
                resizes: Mutex::new(Vec::new()),
                mode: Mutex::new(TerminalInputMode::default()),
                foreground: Mutex::new(None),
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
        if *self.reject_input.lock().unwrap() {
            anyhow::bail!("cola PTY llena");
        }
        self.inputs.lock().unwrap().push(input);
        Ok(())
    }

    fn send_key_input(&self, input: TerminalKeyInput) -> Result<()> {
        self.send_input(input.bytes(self.input_mode()))
    }

    fn resize(&self, size: TerminalSize) -> Result<()> {
        self.resizes.lock().unwrap().push(size);
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
        *self.mode.lock().unwrap()
    }

    fn foreground_process_id(&self) -> Option<u32> {
        *self.foreground.lock().unwrap()
    }

    fn clear_selection(&self) {}

    fn start_selection(&self, _: TerminalSelectionType, _: TerminalPoint, _: TerminalCellSide) {}

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
fn terminal_only_resizes_pty_when_grid_metrics_change(cx: &mut TestAppContext) {
    let port = Arc::new(MockTerminalPort::new());
    let handle = port.handle.clone();
    let (view, cx) = cx.add_window_view(|_, cx| {
        TerminalView::new_with_environment(
            Uuid::new_v4(),
            "Terminal".into(),
            Path::new("/"),
            port,
            HashMap::new(),
            cx,
        )
    });
    let draw = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| window.draw(cx).clear());
        cx.run_until_parked();
    };

    draw(cx);
    assert_eq!(handle.resizes.lock().unwrap().len(), 1);
    draw(cx);
    assert_eq!(handle.resizes.lock().unwrap().len(), 1);

    view.update(cx, |view, cx| {
        view.apply_font_size(TERMINAL_FONT_SIZE + 1.0, cx)
    });
    draw(cx);
    let resizes = handle.resizes.lock().unwrap();
    assert_eq!(resizes.len(), 2);
    assert_ne!(resizes[0], resizes[1]);
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
fn key_release_does_not_follow_exited_foreground_job_into_shell(cx: &mut TestAppContext) {
    let port = Arc::new(MockTerminalPort::new());
    let handle = port.handle.clone();
    *handle.mode.lock().unwrap() = TerminalInputMode {
        disambiguate_escape_codes: true,
        report_event_types: true,
        ..TerminalInputMode::default()
    };
    *handle.foreground.lock().unwrap() = Some(200);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                TerminalView::new_with_environment(
                    Uuid::new_v4(),
                    "Terminal".into(),
                    Path::new("/"),
                    port,
                    HashMap::new(),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let ctrl_c = key(
        "c",
        Modifiers {
            control: true,
            ..Modifiers::default()
        },
    );
    window
        .update(cx, |terminal, window, cx| {
            terminal.on_key_down(
                &KeyDownEvent {
                    keystroke: ctrl_c.clone(),
                    is_held: false,
                },
                window,
                cx,
            );
        })
        .unwrap();
    *handle.foreground.lock().unwrap() = Some(100);
    window
        .update(cx, |terminal, window, cx| {
            terminal.on_key_up(
                &KeyUpEvent {
                    keystroke: ctrl_c.clone(),
                },
                window,
                cx,
            );
        })
        .unwrap();
    assert_eq!(handle.inputs.lock().unwrap().as_slice(), [b"\x1b[99;5u"]);

    *handle.foreground.lock().unwrap() = Some(300);
    window
        .update(cx, |terminal, window, cx| {
            terminal.on_key_down(
                &KeyDownEvent {
                    keystroke: ctrl_c.clone(),
                    is_held: false,
                },
                window,
                cx,
            );
            terminal.on_key_up(
                &KeyUpEvent {
                    keystroke: ctrl_c.clone(),
                },
                window,
                cx,
            );
        })
        .unwrap();
    assert_eq!(
        handle.inputs.lock().unwrap().as_slice(),
        [
            b"\x1b[99;5u".to_vec(),
            b"\x1b[99;5u".to_vec(),
            b"\x1b[99;5:3u".to_vec(),
        ]
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
fn denying_an_osc52_clipboard_read_sends_empty_response(cx: &mut TestAppContext) {
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
                TerminalEvent::ClipboardLoad(Arc::new(|text| format!("\x1b]52;c;{text}\x07"))),
                cx,
            );
            terminal.cancel_pending_action(cx);
        })
        .unwrap();

    assert_eq!(
        mock_handle.inputs.lock().unwrap().as_slice(),
        [b"\x1b]52;c;\x07".to_vec()]
    );
}

#[gpui::test]
fn osc52_read_flood_caps_pending_confirmations(cx: &mut TestAppContext) {
    cx.write_to_clipboard(ClipboardItem::new_string("private".into()));
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
            for _ in 0..MAX_CLIPBOARD_CONFIRMATIONS + 1 {
                terminal.handle_terminal_event(
                    TerminalEvent::ClipboardLoad(Arc::new(|text| format!("OSC52:{text}"))),
                    cx,
                );
            }
            assert_eq!(
                terminal.pending_confirmations.len(),
                MAX_CLIPBOARD_CONFIRMATIONS
            );
        })
        .unwrap();
    assert_eq!(
        mock_handle.inputs.lock().unwrap().as_slice(),
        [b"OSC52:".to_vec()]
    );
}

#[gpui::test]
fn rejected_terminal_input_shows_error_until_a_write_succeeds(cx: &mut TestAppContext) {
    let port = Arc::new(MockTerminalPort::new());
    let handle = port.handle.clone();
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

    *handle.reject_input.lock().unwrap() = true;
    window
        .update(cx, |terminal, _, cx| {
            assert!(!terminal.send_protocol(b"\x1b[6n".to_vec(), cx));
            assert!(
                terminal
                    .input_error
                    .as_deref()
                    .is_some_and(|message| message.contains("cola PTY llena"))
            );
            assert!(!terminal.send_key(
                &key("a", Modifiers::default()),
                TerminalKeyEventType::Press,
                cx,
            ));
            assert!(terminal.input_error.is_some());
        })
        .unwrap();
    assert!(handle.inputs.lock().unwrap().is_empty());

    *handle.reject_input.lock().unwrap() = false;
    window
        .update(cx, |terminal, _, cx| {
            assert!(terminal.send(b"ok".to_vec(), cx));
            assert!(terminal.input_error.is_none());
        })
        .unwrap();
    assert_eq!(handle.inputs.lock().unwrap().as_slice(), [b"ok".to_vec()]);
}

#[gpui::test]
fn external_paste_reports_confirmation_cancellation_and_exit(cx: &mut TestAppContext) {
    let port = Arc::new(MockTerminalPort::new());
    let handle = port.handle.clone();
    let session_id = Uuid::new_v4();
    let (view, cx) = cx.add_window_view(|_, cx| {
        TerminalView::new_with_environment(
            session_id,
            "Terminal".into(),
            Path::new("/"),
            port,
            std::collections::HashMap::new(),
            cx,
        )
    });
    let resolved = Rc::new(RefCell::new(Vec::new()));
    let sink = resolved.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &TerminalViewEvent, _| {
            if let TerminalViewEvent::ExternalPasteResolved {
                session_id,
                token,
                accepted,
            } = event
            {
                sink.borrow_mut().push((*session_id, *token, *accepted));
            }
        })
        .detach();
    });

    let accepted_token = Uuid::new_v4();
    view.update(cx, |terminal, cx| {
        assert_eq!(
            terminal.insert_external_text("review\nbody", accepted_token, cx),
            TerminalInsertStatus::Pending
        );
        assert!(handle.inputs.lock().unwrap().is_empty());
        terminal.confirm_pending_action(cx);
    });
    cx.run_until_parked();
    assert_eq!(
        resolved.borrow().as_slice(),
        [(session_id, accepted_token, true)]
    );
    assert_eq!(handle.inputs.lock().unwrap().len(), 1);

    let cancelled_token = Uuid::new_v4();
    view.update(cx, |terminal, cx| {
        assert_eq!(
            terminal.insert_external_text("review\nbody", cancelled_token, cx),
            TerminalInsertStatus::Pending
        );
        terminal.cancel_pending_action(cx);
    });
    cx.run_until_parked();
    assert_eq!(resolved.borrow()[1], (session_id, cancelled_token, false));
    assert_eq!(handle.inputs.lock().unwrap().len(), 1);

    let rejected_token = Uuid::new_v4();
    *handle.reject_input.lock().unwrap() = true;
    view.update(cx, |terminal, cx| {
        assert_eq!(
            terminal.insert_external_text("review\nbody", rejected_token, cx),
            TerminalInsertStatus::Pending
        );
        terminal.confirm_pending_action(cx);
        assert!(terminal.input_error.is_some());
    });
    cx.run_until_parked();
    assert_eq!(resolved.borrow()[2], (session_id, rejected_token, false));
    assert_eq!(handle.inputs.lock().unwrap().len(), 1);
    *handle.reject_input.lock().unwrap() = false;

    let exited_token = Uuid::new_v4();
    view.update(cx, |terminal, cx| {
        assert_eq!(
            terminal.insert_external_text("review\nbody", exited_token, cx),
            TerminalInsertStatus::Pending
        );
        terminal.handle_terminal_event(TerminalEvent::Exit(Some(0)), cx);
        assert!(terminal.pending_confirmations.is_empty());
    });
    cx.run_until_parked();
    assert_eq!(resolved.borrow()[3], (session_id, exited_token, false));
    assert_eq!(handle.inputs.lock().unwrap().len(), 1);
}

#[gpui::test]
fn external_paste_immediate_status_tracks_pty_acceptance(cx: &mut TestAppContext) {
    let port = Arc::new(MockTerminalPort::new());
    let handle = port.handle.clone();
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
    handle.mode.lock().unwrap().bracketed_paste = true;
    window
        .update(cx, |terminal, _, cx| {
            assert_eq!(
                terminal.insert_external_text("review\nbody", Uuid::new_v4(), cx),
                TerminalInsertStatus::Accepted
            );
            assert!(terminal.pending_confirmations.is_empty());
        })
        .unwrap();
    assert_eq!(handle.inputs.lock().unwrap().len(), 1);

    *handle.reject_input.lock().unwrap() = true;
    window
        .update(cx, |terminal, _, cx| {
            assert_eq!(
                terminal.insert_external_text("review\nbody", Uuid::new_v4(), cx),
                TerminalInsertStatus::Rejected
            );
            assert!(terminal.input_error.is_some());
            assert!(terminal.pending_confirmations.is_empty());
        })
        .unwrap();
    assert_eq!(handle.inputs.lock().unwrap().len(), 1);
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
fn background_runs_preserve_tui_colors_and_selection_during_resize() {
    let black = TerminalRgb::new(0, 0, 0);
    let blue = TerminalRgb::new(20, 40, 80);
    let cell = |row, column, background| {
        TerminalCell::with_text(
            row,
            column,
            " ",
            TerminalRgb::new(255, 255, 255),
            background,
        )
    };
    let mut selected = cell(0, 2, blue);
    selected.selected = true;
    let snapshot = TerminalSnapshot {
        columns: 3,
        rows: 2,
        // A shorter row may arrive while the terminal is resizing.
        lines: vec![
            Arc::from([cell(0, 0, black), cell(0, 1, blue), selected]),
            Arc::from([cell(1, 0, black)]),
        ],
        cursor: None,
        display_offset: 0,
        history_size: 0,
    };
    let mut coverage = [0; 6];
    let mut backgrounds = [to_hsla(black); 6];
    for run in collect_background_runs(&snapshot) {
        assert!(run.row < 2 && run.columns.end <= 3);
        for column in run.columns {
            let index = run.row * 3 + column;
            coverage[index] += 1;
            backgrounds[index] = run.color;
        }
    }
    assert_eq!(coverage, [1; 6], "background tints must never overlap");
    assert_eq!(
        backgrounds,
        [
            to_hsla(black),
            to_hsla(blue),
            colors().selection.into(),
            to_hsla(black),
            to_hsla(black),
            to_hsla(black),
        ]
    );
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
    assert_eq!(presence.state, AgentRuntimeState::Waiting);

    let title_presence =
        detect_agent_presence("OpenAI Codex", &snapshot, None, None, None).unwrap();
    assert_eq!(title_presence.kind, "Codex");
    assert_eq!(title_presence.state, AgentRuntimeState::Waiting);
}

#[test]
fn grok_permission_mode_badge_is_not_a_waiting_prompt() {
    let snapshot = blank_agent_snapshot();
    let working = detect_agent_presence(
        ".. - Preparing read_file... - Add Sidebar",
        &snapshot,
        Some("❯\nalways-approve   42%"),
        Some("grok"),
        Some(7),
    )
    .unwrap();
    assert_eq!(working.kind, "Grok");
    assert_eq!(working.state, AgentRuntimeState::Working);

    let responding = detect_agent_presence(
        "Responding - vibra",
        &snapshot,
        Some("always-approve"),
        Some("grok"),
        Some(7),
    )
    .unwrap();
    assert_eq!(responding.state, AgentRuntimeState::Working);

    let idle = detect_agent_presence(
        "vibra",
        &snapshot,
        Some("❯\nalways-approve   12%"),
        Some("grok"),
        Some(7),
    )
    .unwrap();
    assert_eq!(idle.state, AgentRuntimeState::Idle);
    assert_eq!(
        detect_agent_presence(
            "vibra",
            &snapshot,
            Some("corresponding change\nalways-approve"),
            Some("grok"),
            Some(7),
        )
        .unwrap()
        .state,
        AgentRuntimeState::Idle
    );

    let permission = detect_agent_presence(
        "Action Required - vibra",
        &snapshot,
        Some("Yes, allow once\nNo, reject\nalways-approve"),
        Some("grok"),
        Some(7),
    )
    .unwrap();
    assert_eq!(permission.state, AgentRuntimeState::Waiting);
    assert_eq!(
        detect_agent_presence(
            "Terminal",
            &snapshot,
            Some("approve this command?"),
            Some("grok"),
            Some(7),
        )
        .unwrap()
        .state,
        AgentRuntimeState::Waiting
    );
}

fn blank_agent_snapshot() -> TerminalSnapshot {
    TerminalSnapshot {
        columns: 80,
        rows: 1,
        lines: vec![Arc::from([TerminalCell::with_text(
            0,
            0,
            " ",
            TerminalRgb::new(255, 255, 255),
            TerminalRgb::new(0, 0, 0),
        )])],
        cursor: None,
        display_offset: 0,
        history_size: 0,
    }
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
    assert_eq!(presence.state, AgentRuntimeState::Idle);
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
    assert_eq!(
        detect_agent_presence(
            "Terminal",
            &snapshot,
            None,
            Some("/usr/local/bin/cursor-agent"),
            Some(47),
        )
        .unwrap()
        .kind,
        "Cursor"
    );

    let process_with_state_word =
        detect_agent_presence("Terminal", &snapshot, None, Some("codex-working"), Some(44))
            .unwrap();
    assert_eq!(process_with_state_word.state, AgentRuntimeState::Idle);
    assert!(AgentKind::from_process_name("codexical").is_none());
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
