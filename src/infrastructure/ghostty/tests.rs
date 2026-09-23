use super::*;
use crate::ports::terminal_keyboard::{
    TerminalKeyEventType, TerminalKeyInput, TerminalKeystroke, TerminalModifiers,
};
fn engine() -> Engine {
    let (tx, _) = async_channel::unbounded();
    Engine::new(TerminalSize::default(), tx).unwrap()
}
#[test]
fn fragmented_unicode_styles_modes_and_alternate_screen() {
    let mut e = engine();
    for b in "\x1b[38;2;17;101;221mEspañol 日本語 e\u{301} 🦀\x1b[0m".as_bytes() {
        e.feed(&[*b]);
    }
    let s = e.snapshot().unwrap();
    assert_eq!(s.lines[0][0].foreground, TerminalRgb::new(17, 101, 221));
    assert!(e.text(false).unwrap().contains("日本語 e\u{301} 🦀"));
    assert!(s.lines[0].iter().any(|c| c.wide_spacer));
    e.feed(b"\x1b[?2004h\x1b[?1h\x1b[>31u");
    let m = e.mode();
    assert!(m.bracketed_paste && m.application_cursor && m.report_associated_text);
    e.feed(b"\x1b[?1049hALTERNATE");
    assert!(e.mode().alternate_screen);
    assert!(e.text(false).unwrap().contains("ALTERNATE"));
    e.feed(b"\x1b[?1049l");
    assert!(!e.mode().alternate_screen);
    assert!(e.text(false).unwrap().contains("Español"));
}
#[test]
fn snapshot_cache_selection_search_and_hyperlinks() {
    let mut e = engine();
    e.feed(b"hello world\r\n\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\");
    let a = e.snapshot().unwrap();
    assert_eq!(
        a.lines[1][0].hyperlink.as_deref(),
        Some("https://example.com")
    );
    assert!(Arc::ptr_eq(&a, &e.snapshot().unwrap()));
    e.select(
        1,
        TerminalSelectionType::Semantic,
        TerminalPoint { row: 0, column: 7 },
        TerminalCellSide::Left,
    );
    assert_eq!(e.text(true).as_deref(), Some("world"));
    assert!(e.snapshot().unwrap().lines[0][7].selected);
    e.select(
        0,
        TerminalSelectionType::Simple,
        TerminalPoint::default(),
        TerminalCellSide::Left,
    );
    assert_eq!(unsafe { vg_search(e.p(), b"hello".as_ptr(), 5, 0) }, 1);
    assert_eq!(e.text(true).as_deref(), Some("hello"));
    assert_eq!(
        unsafe { vg_search(e.p(), b"not present".as_ptr(), 11, 0) },
        0
    );
}
#[test]
fn replies_and_scrollback_resize() {
    let mut e = engine();
    e.feed(b"\x1b[6n");
    assert_eq!(
        e.callbacks.replies.drain(..).collect::<Vec<_>>(),
        b"\x1b[1;1R"
    );
    for i in 0..100 {
        e.feed(format!("line {i}\r\n").as_bytes());
    }
    assert!(e.snapshot().unwrap().history_size > 0);
    unsafe { vg_scroll(e.p(), -10) };
    e.dirty = true;
    assert!(e.snapshot().unwrap().display_offset > 0);
    e.resize(TerminalSize {
        columns: 40,
        rows: 12,
        ..TerminalSize::default()
    })
    .unwrap();
    assert_eq!(e.snapshot().unwrap().columns, 40);
    checked(unsafe { vg_clear_history(e.p()) }).unwrap();
    e.dirty = true;
    assert_eq!(e.snapshot().unwrap().history_size, 0);
}
fn wait_for(t: &GhosttyTerminal, needle: &str) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let text = t.engine.lock().unwrap().text(false).unwrap_or_default();
        if text.contains(needle) {
            return;
        }
        assert!(Instant::now() < until, "missing {needle:?}: {text:?}");
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn real_pty_input_resize_exit_and_environment() {
    let t = GhosttyTerminal::spawn(
        Uuid::new_v4(),
        Path::new("/tmp"),
        &HashMap::new(),
        Some((
            "/bin/sh",
            &[
                "-c",
                concat!(
                    "printf 'READY\\n'; read answer; ",
                    "printf 'ANSWER=%s TERM=%s\\n' \"$answer\" \"$TERM\"; ",
                    "stty size; exit 7"
                ),
            ],
        )),
    )
    .unwrap();
    wait_for(&t, "READY");
    assert!(t.foreground_process_id().is_some());
    t.resize(TerminalSize {
        columns: 100,
        rows: 30,
        ..TerminalSize::default()
    })
    .unwrap();
    t.send_input(b"from-vibra\n".to_vec()).unwrap();
    wait_for(&t, "ANSWER=from-vibra TERM=xterm-256color");
    wait_for(&t, "30 100");
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(TerminalEvent::Exit(code)) = t.events.try_recv() {
            assert_eq!(code, Some(7));
            break;
        }
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(t.send_input(vec![b'x']).is_err());
}
#[test]
fn real_pty_ctrl_c_and_query_reply() {
    let t = GhosttyTerminal::spawn(
        Uuid::new_v4(),
        Path::new("/tmp"),
        &HashMap::new(),
        Some((
            "/bin/sh",
            &[
                "-c",
                "trap 'printf INTERRUPTED; exit 0' INT; printf READY; read answer",
            ],
        )),
    )
    .unwrap();
    wait_for(&t, "READY");
    t.send_input(vec![3]).unwrap();
    wait_for(&t, "INTERRUPTED");
    let q = GhosttyTerminal::spawn(
        Uuid::new_v4(),
        Path::new("/tmp"),
        &HashMap::new(),
        Some((
            "/bin/sh",
            &[
                "-c",
                concat!(
                    "stty -echo -icanon min 1 time 0; printf '\\033[6n'; ",
                    "dd bs=1 count=6 2>/dev/null | od -An -tx1"
                ),
            ],
        )),
    )
    .unwrap();
    wait_for(&q, "52");
    let reply = q.engine.lock().unwrap().text(false).unwrap();
    assert_eq!(
        reply.split_whitespace().collect::<Vec<_>>(),
        ["1b", "5b", "31", "3b", "31", "52"]
    );
}

#[test]
fn queued_ctrl_c_events_use_restored_shell_mode() {
    let t = GhosttyTerminal::spawn(
        Uuid::new_v4(),
        Path::new("/tmp"),
        &HashMap::new(),
        Some((
            "/bin/sh",
            &[
                "-c",
                concat!(
                    "stty raw -echo; printf '\\033[>3uREADY\\r\\n'; ",
                    "dd bs=1 count=3 2>/dev/null | od -An -tx1; ",
                    "printf '\\r\\nDONE'"
                ),
            ],
        )),
    )
    .unwrap();
    wait_for(&t, "READY");
    let input = |event_type| TerminalKeyInput {
        keystroke: TerminalKeystroke {
            key: "c".into(),
            key_char: Some("c".into()),
            modifiers: TerminalModifiers {
                control: true,
                ..TerminalModifiers::default()
            },
        },
        event_type,
    };
    assert_eq!(
        input(TerminalKeyEventType::Release).bytes(t.input_mode()),
        b"\x1b[99;5:3u"
    );
    {
        // Hold the worker before it can write input. This is the same
        // ordering as a TUI's pending cleanup being read before a queued key.
        let mut e = t.engine.lock().unwrap();
        for event_type in [
            TerminalKeyEventType::Release,
            TerminalKeyEventType::Press,
            TerminalKeyEventType::Repeat,
            TerminalKeyEventType::Release,
        ] {
            t.send_key_input(input(event_type)).unwrap();
        }
        e.feed(b"\x1b[<u");
        assert!(!e.mode().kitty_keyboard());
        t.send_input(b"A".to_vec()).unwrap();
    }
    wait_for(&t, "DONE");
    let text = t.engine.lock().unwrap().text(false).unwrap();
    assert_eq!(
        text.split_whitespace().collect::<Vec<_>>(),
        ["READY", "03", "03", "41", "DONE"],
        "Kitty sequences leaked into shell input: {text:?}"
    );
}

#[test]
fn clipboard_write_and_selection_drag() {
    let (tx, rx) = async_channel::unbounded();
    let mut e = Engine::new(TerminalSize::default(), tx).unwrap();
    e.feed(b"\x1b]52;c;aGVsbG8=\x07");
    assert!(matches!(rx.try_recv(), Ok(TerminalEvent::ClipboardStore(text)) if text == "hello"));
    e.feed(b"hello world");
    e.select(
        1,
        TerminalSelectionType::Simple,
        TerminalPoint { row: 0, column: 0 },
        TerminalCellSide::Left,
    );
    e.select(
        2,
        TerminalSelectionType::Simple,
        TerminalPoint { row: 0, column: 4 },
        TerminalCellSide::Right,
    );
    assert_eq!(e.text(true).as_deref(), Some("hello"));
    e.feed(b"\r\n(https://example.com).");
    let snapshot = e.snapshot().unwrap();
    assert_eq!(
        plain_hyperlink(&snapshot.lines[1], 4).as_deref(),
        Some("https://example.com")
    );
    assert_eq!(plain_hyperlink(&snapshot.lines[1], 0), None);
}

#[test]
fn clipboard_reads_are_owned_and_wait_for_consent() {
    use base64::Engine as _;
    let (tx, rx) = async_channel::unbounded();
    let mut e = Engine::new(TerminalSize::default(), tx).unwrap();
    for byte in b"\x1b]52;c;?\x07\x1b]52;p;?\x1b\\" {
        e.feed(&[*byte]);
    }
    assert!(
        e.callbacks.replies.is_empty(),
        "must not answer before consent"
    );
    let TerminalEvent::ClipboardLoad(first) = rx.try_recv().unwrap() else {
        panic!("missing consent event")
    };
    let TerminalEvent::ClipboardLoad(second) = rx.try_recv().unwrap() else {
        panic!("missing consent event")
    };
    // Call after further parser mutations and terminal destruction: callbacks
    // must contain owned protocol strings, never a borrowed Ghostty request.
    e.feed(b"still responsive");
    drop(e);
    let secret = "Español\n\x1b]52;c;injection";
    let encoded = base64::engine::general_purpose::STANDARD.encode(secret);
    assert_eq!(first(secret), format!("\x1b]52;c;{encoded}\x07"));
    assert_eq!(second("hello"), "\x1b]52;p;aGVsbG8=\x1b\\");
    assert!(clipboard_template(b"\x1b]52;c;payload\x07").is_none());
    assert!(clipboard_template(b"\x1b]52;x;\x07").is_none());
}

#[test]
fn full_event_queue_completes_clipboard_read_with_empty_reply() {
    let (tx, rx) = async_channel::bounded(1);
    let mut e = Engine::new(TerminalSize::default(), tx).unwrap();
    e.feed(b"\x1b]52;c;?\x07\x1b]52;p;?\x07");
    assert!(matches!(rx.try_recv(), Ok(TerminalEvent::ClipboardLoad(_))));
    assert_eq!(
        e.callbacks.replies.drain(..).collect::<Vec<_>>(),
        b"\x1b]52;p;\x07"
    );
}

#[test]
fn full_event_queue_does_not_latch_wakeup() {
    let (tx, rx) = async_channel::bounded(1);
    let pending = AtomicBool::new(false);
    tx.try_send(TerminalEvent::Bell).unwrap();
    assert!(!wake(&tx, &pending));
    assert!(!pending.load(Ordering::Acquire));
    assert!(matches!(rx.try_recv(), Ok(TerminalEvent::Bell)));
    assert!(wake(&tx, &pending));
    assert!(matches!(rx.try_recv(), Ok(TerminalEvent::Wakeup)));
}

#[test]
fn exit_event_survives_a_full_event_queue() {
    let terminal = GhosttyTerminal::spawn(
        Uuid::new_v4(),
        Path::new("/tmp"),
        &HashMap::new(),
        Some((
            "/bin/sh",
            &[
                "-c",
                "i=0; while [ \"$i\" -lt 64 ]; do printf '\\a'; i=$((i+1)); done; exit 7",
            ],
        )),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match terminal.events.try_recv() {
            Ok(TerminalEvent::Exit(code)) => {
                assert_eq!(code, Some(7));
                break;
            }
            Ok(_) | Err(async_channel::TryRecvError::Empty) => {}
            Err(async_channel::TryRecvError::Closed) => {
                panic!("event stream closed before Exit")
            }
        }
        assert!(
            Instant::now() < deadline,
            "missing Exit after a full event queue"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn literal_search_preserves_case_whitespace_and_wraps() {
    let mut e = engine();
    e.feed(b"Hello hello HELLO  src/(main|lib).rs\r\nhello");
    for needle in ["hello", "Hello", "HELLO", "src/(main|lib).rs", "  "] {
        assert_eq!(
            unsafe { vg_search(e.p(), needle.as_ptr(), needle.len(), 0) },
            1,
            "{needle}"
        );
        // Selection formatter trims whitespace for copying; compare that policy.
        assert_eq!(e.text(true).unwrap().trim_end(), needle.trim_end());
    }
    assert_eq!(unsafe { vg_search(e.p(), b"hElLo".as_ptr(), 5, 0) }, 0);
    for direction in [0, 0, 0, 1, 1] {
        assert_eq!(
            unsafe { vg_search(e.p(), b"hello".as_ptr(), 5, direction) },
            1
        );
        assert_eq!(e.text(true).as_deref(), Some("hello"));
    }
}

#[test]
fn literal_search_steps_through_dense_case_mismatches() {
    let mut e = engine();
    let line = format!("{}\r\n", "A".repeat(100));
    for _ in 0..1000 {
        e.feed(line.as_bytes());
    }
    let mut steps = 0;
    let result = loop {
        let result = unsafe { vg_search_step(e.p(), b"a".as_ptr(), 1, 0, (steps > 0).into()) };
        steps += 1;
        if result != 2 {
            break result;
        }
        assert!(steps < 3000, "la búsqueda incremental no termina");
    };
    assert_eq!(result, 0);
    assert!(steps > 1, "la búsqueda densa debe ceder el control");
}

#[test]
fn search_scrolls_to_a_match_in_history() {
    let mut e = engine();
    e.feed(b"TARGET\r\n");
    for _ in 0..100 {
        e.feed(b"unrelated row\r\n");
    }
    let before = e.snapshot().unwrap().display_offset;
    assert!(e.snapshot().unwrap().history_size > 0);
    assert_eq!(unsafe { vg_search(e.p(), b"TARGET".as_ptr(), 6, 0) }, 1);
    e.dirty = true;
    let after = e.snapshot().unwrap().display_offset;
    assert!(
        after > before,
        "la coincidencia del historial debe ser visible: antes={before}, después={after}"
    );
    assert_eq!(e.text(true).as_deref(), Some("TARGET"));
}

#[test]
fn incremental_snapshot_keeps_clean_rows_and_clears_dirty_cells() {
    let mut e = engine();
    e.feed(b"row zero\r\nrow one\r\nrow two");
    let before = e.snapshot().unwrap();
    e.feed(b"\x1b[2;1H\x1b[2Kchanged");
    let _ = e.mode(); // Querying modes must not consume pending cell damage.
    let after = e.snapshot().unwrap();
    assert!(Arc::ptr_eq(&before.lines[0], &after.lines[0]));
    assert!(Arc::ptr_eq(&before.lines[2], &after.lines[2]));
    assert!(!Arc::ptr_eq(&before.lines[1], &after.lines[1]));
    assert!(
        e.last_painted_cells <= 160,
        "clean rows must not cross FFI: {}",
        e.last_painted_cells
    );
    assert_eq!(after.lines[1][0].text(), "c");
    assert_eq!(after.lines[1][7].text(), " ");
    e.feed(b"\x1b[2J");
    assert!(
        e.snapshot()
            .unwrap()
            .lines
            .iter()
            .flat_map(|r| r.iter())
            .all(|c| c.text() == " ")
    );
}

#[test]
fn real_pty_sustained_output_and_recent_text_ignore_viewport() {
    let t = GhosttyTerminal::spawn(
        Uuid::new_v4(),
        Path::new("/tmp"),
        &HashMap::new(),
        Some((
            "/bin/sh",
            &[
                "-c",
                concat!(
                    "i=0; while [ $i -lt 12000 ]; do ",
                    "printf 'line-%s abcdefghijklmnopqrstuvwxyz0123456789\\n' $i; ",
                    "i=$((i+1)); done; printf 'FINAL-TAIL'; read answer"
                ),
            ],
        )),
    )
    .unwrap();
    wait_for(&t, "FINAL-TAIL");
    let before = t.snapshot();
    assert!(before.history_size > 0);
    t.scroll(50);
    let scrolled = t.snapshot();
    assert!(scrolled.display_offset > 0);
    assert_ne!(before.lines[0], scrolled.lines[0]);
    let recent = t.recent_text(3).unwrap();
    assert!(recent.contains("FINAL-TAIL"));
    assert!(!recent.contains("line-0 "));
    t.shutdown();
}

#[test]
fn real_vim_enters_and_leaves_alternate_screen() {
    let t = GhosttyTerminal::spawn(
        Uuid::new_v4(),
        Path::new("/tmp"),
        &HashMap::new(),
        Some(("/usr/bin/vim", &["-Nu", "NONE", "-n", "-i", "NONE"])),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !t.input_mode().alternate_screen {
        assert!(
            Instant::now() < deadline,
            "vim did not enter alternate screen"
        );
        thread::sleep(Duration::from_millis(5));
    }
    t.send_input(b":q!\r".to_vec()).unwrap();
    while t.alive.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "vim did not exit");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!t.input_mode().alternate_screen);
}

#[test]
#[ignore = "manual performance measurement, not a timing-sensitive CI assertion"]
fn profile_sessions() {
    fn peak_rss() -> i64 {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        assert_eq!(
            unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
            0
        );
        unsafe { usage.assume_init().ru_maxrss }
    }
    let baseline = peak_rss();
    let mut engines = Vec::new();
    for _ in 0..8 {
        let mut e = engine();
        e.resize(TerminalSize {
            columns: 120,
            rows: 40,
            ..TerminalSize::default()
        })
        .unwrap();
        e.feed(
            "build: abcdefghijklmnopqrstuvwxyz 0123456789\r\n"
                .repeat(4000)
                .as_bytes(),
        );
        e.snapshot().unwrap();
        engines.push(e);
    }
    let mut elapsed = Vec::new();
    let mut cells = 0u64;
    for i in 0..800 {
        let e = &mut engines[i % 8];
        let start = Instant::now();
        e.feed(format!("\x1b[2;1Htick {i:08}").as_bytes());
        e.snapshot().unwrap();
        elapsed.push(start.elapsed().as_secs_f64() * 1000.);
        cells += u64::from(e.last_painted_cells);
    }
    elapsed.sort_by(f64::total_cmp);
    println!(
        concat!(
            "PROFILE 8 sessions: update median_ms={:.3} p95_ms={:.3}; ",
            "cells={}/{} full-screen baseline; peak_rss_before={} after={} bytes"
        ),
        elapsed[400],
        elapsed[760],
        cells,
        800 * 120 * 40,
        baseline,
        peak_rss()
    );
    let t = GhosttyTerminal::spawn(
        Uuid::new_v4(),
        Path::new("/tmp"),
        &HashMap::new(),
        Some((
            "/bin/sh",
            &[
                "-c",
                "printf 'READY\\n'; while read value; do printf 'ACK-%s\\n' \"$value\"; done",
            ],
        )),
    )
    .unwrap();
    wait_for(&t, "READY");
    let mut latency = Vec::new();
    for i in 0..30 {
        let token = format!("ACK-{i:04}");
        let start = Instant::now();
        t.send_input(format!("{i:04}\n").into_bytes()).unwrap();
        loop {
            if t.recent_text(40).unwrap().contains(&token) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_micros(100));
        }
        latency.push(start.elapsed().as_secs_f64() * 1000.);
    }
    latency.sort_by(f64::total_cmp);
    println!(
        "PROFILE PTY roundtrip (100us observation polling): median_ms={:.3} p95_ms={:.3}",
        latency[15], latency[28]
    );
}

#[test]
fn shutdown_reaps_child() {
    let t = GhosttyTerminal::spawn(
        Uuid::new_v4(),
        Path::new("/tmp"),
        &HashMap::new(),
        Some((
            "/bin/sh",
            &[
                "-c",
                "trap '' HUP; printf 'READY\\n'; while :; do sleep 1; done",
            ],
        )),
    )
    .unwrap();
    wait_for(&t, "READY");
    t.shutdown();
    let until = Instant::now() + Duration::from_secs(5);
    while t.alive.load(Ordering::Acquire) {
        assert!(Instant::now() < until, "child not reaped");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        unsafe { libc::waitpid(t.pid as i32, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
}
