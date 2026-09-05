use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    term::{Config, Term},
    vte::ansi::Processor,
};
use std::{ffi::c_void, hint::black_box, time::Instant};

unsafe extern "C" {
    fn eval_new(cols: u16, rows: u16) -> *mut c_void;
    fn eval_free(ptr: *mut c_void);
    fn eval_feed(ptr: *mut c_void, bytes: *const u8, len: usize);
    fn eval_resize(ptr: *mut c_void, cols: u16, rows: u16);
    fn eval_capture(ptr: *mut c_void) -> u64;
    fn eval_format(ptr: *mut c_void, len: *mut usize, styled: i32) -> *mut u8;
    fn eval_buffer_free(ptr: *mut u8, len: usize);
}

struct Ghostty(*mut c_void);
impl Ghostty {
    fn new(cols: u16, rows: u16) -> Self {
        Self(unsafe { eval_new(cols, rows) })
    }
    fn feed(&mut self, bytes: &[u8]) {
        unsafe { eval_feed(self.0, bytes.as_ptr(), bytes.len()) }
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        unsafe { eval_resize(self.0, cols, rows) }
    }
    fn capture(&self) -> u64 {
        unsafe { eval_capture(self.0) }
    }
    fn format(&self, styled: bool) -> Vec<u8> {
        let mut len = 0;
        let ptr = unsafe { eval_format(self.0, &mut len, styled.into()) };
        let out = if len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
        };
        unsafe { eval_buffer_free(ptr, len) };
        out
    }
}
impl Drop for Ghostty {
    fn drop(&mut self) {
        unsafe { eval_free(self.0) }
    }
}

struct Size(usize, usize);
impl Dimensions for Size {
    fn columns(&self) -> usize {
        self.0
    }
    fn screen_lines(&self) -> usize {
        self.1
    }
    fn total_lines(&self) -> usize {
        self.1
    }
}
struct Alacritty {
    term: Term<VoidListener>,
    parser: Processor,
}
impl Alacritty {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            term: Term::new(
                Config {
                    scrolling_history: 1000,
                    kitty_keyboard: true,
                    ..Config::default()
                },
                &Size(cols, rows),
                VoidListener,
            ),
            parser: Processor::new(),
        }
    }
    fn feed(&mut self, data: &[u8]) {
        self.parser.advance(&mut self.term, data);
    }
    fn text(&self) -> String {
        let mut lines = vec![String::new(); self.term.screen_lines()];
        for cell in self.term.renderable_content().display_iter {
            let row = cell.point.line.0;
            if row < 0 || row as usize >= lines.len() {
                continue;
            }
            lines[row as usize].push(cell.c);
            if let Some(extra) = cell.zerowidth() {
                lines[row as usize].extend(extra);
            }
        }
        lines
            .iter()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_owned()
    }
    fn capture(&self) -> u64 {
        let mut h = 14695981039346656037u64;
        for c in self.term.renderable_content().display_iter {
            h = (h ^ u64::from(c.c as u32)).wrapping_mul(1099511628211);
            if let Some(extra) = c.zerowidth() {
                for ch in extra {
                    h = (h ^ u64::from(*ch as u32)).wrapping_mul(1099511628211);
                }
            }
            h = (h ^ u64::from(c.flags.bits())).wrapping_mul(1099511628211);
        }
        h
    }
}

fn fixtures() {
    let cases: &[(&str, &[u8], &str)] = &[
        ("cursor-erase", b"hello\r\nold\x1b[2;1H\x1b[2Knew", "new"),
        ("truecolor", b"\x1b[38;2;17;101;221mCOLOR\x1b[0m", "COLOR"),
        (
            "unicode",
            "Español 日本語 e\u{301} 🦀".as_bytes(),
            "Español",
        ),
        (
            "alternate-screen",
            b"normal\x1b[?1049halt-screen",
            "alt-screen",
        ),
        (
            "alternate-return",
            b"normal\x1b[?1049halt\x1b[?1049l",
            "normal",
        ),
        (
            "scrolling-region",
            b"\x1b[2;5r\x1b[2;1Ha\r\nb\r\nc\r\nd\r\nLAST",
            "LAST",
        ),
        (
            "keyboard-modes",
            b"\x1b[?2004h\x1b[?1h\x1b[>1uREADY",
            "READY",
        ),
    ];
    for (name, bytes, expected) in cases {
        let mut ghost = Ghostty::new(80, 24);
        // Exercise arbitrary fragmentation of both UTF-8 and escape sequences.
        for b in *bytes {
            ghost.feed(std::slice::from_ref(b));
        }
        let plain = String::from_utf8_lossy(&ghost.format(false)).into_owned();
        assert!(plain.contains(expected), "{name}: {plain:?}");
        black_box(ghost.capture());
        let ansi = ghost.format(true);
        let mut restored = Ghostty::new(80, 24);
        restored.feed(&ansi);
        assert_eq!(
            plain.trim_end(),
            String::from_utf8_lossy(&restored.format(false)).trim_end(),
            "Ghostty ANSI roundtrip: {name}"
        );
        let mut alacritty = Alacritty::new(80, 24);
        alacritty.feed(&ansi);
        assert!(
            alacritty.text().contains(expected),
            "ANSI cross-engine replay: {name}"
        );
        if *name == "truecolor" {
            assert!(ansi.windows(5).any(|w| w == b"38;2;"));
        }
        if let Some(dir) = std::env::var_os("EVAL_FIXTURE_DIR") {
            std::fs::write(
                std::path::Path::new(&dir).join(format!("{name}.ansi")),
                &ansi,
            )
            .unwrap();
        }
        println!(
            "PASS {name}: {} ANSI bytes, Ghostty roundtrip + Alacritty replay",
            ansi.len()
        );
    }
    let mut ghost = Ghostty::new(80, 24);
    ghost.feed(b"resize keeps this content");
    ghost.resize(40, 12);
    ghost.resize(120, 40);
    assert!(String::from_utf8_lossy(&ghost.format(false)).contains("resize keeps this content"));
    println!("PASS resize 80x24 -> 40x12 -> 120x40");
}

fn benchmark() {
    let workloads = [
        ("ascii", "build: checking compilation unit 12345 OK\r\n".repeat(100_000)),
        ("ansi", "\x1b[32mPASS\x1b[0m test 12345\r\n\x1b[38;2;17;101;221mINFO\x1b[0m build completed\r\n".repeat(60_000)),
        ("tui", "\x1b[H\x1b[2Kagent running\r\n\x1b[32mprocessing source files\x1b[0m\x1b[5;1H\x1b[2Kstatus ready".repeat(60_000)),
    ];
    println!("BENCH engine,workload,bytes,median_ms (7 runs, parser only, 4096-byte chunks)");
    for (name, input) in workloads {
        let mut gt = Vec::new();
        let mut at = Vec::new();
        for run in 0..8 {
            // Alternate first engine to reduce ordering bias; first run is warmup.
            for kind in if run % 2 == 0 { [0, 1] } else { [1, 0] } {
                if kind == 0 {
                    let mut term = Ghostty::new(120, 40);
                    let start = Instant::now();
                    for chunk in input.as_bytes().chunks(4096) {
                        term.feed(chunk);
                    }
                    let elapsed = start.elapsed().as_secs_f64() * 1000.;
                    black_box(term.capture());
                    if run > 0 {
                        gt.push(elapsed);
                    }
                } else {
                    let mut term = Alacritty::new(120, 40);
                    let start = Instant::now();
                    for chunk in input.as_bytes().chunks(4096) {
                        term.feed(chunk);
                    }
                    let elapsed = start.elapsed().as_secs_f64() * 1000.;
                    black_box(term.capture());
                    if run > 0 {
                        at.push(elapsed);
                    }
                }
            }
        }
        gt.sort_by(f64::total_cmp);
        at.sort_by(f64::total_cmp);
        println!("ghostty,{name},{},{:.3}", input.len(), gt[3]);
        println!("alacritty,{name},{},{:.3}", input.len(), at[3]);
    }
}

fn main() {
    fixtures();
    if !std::env::args().any(|a| a == "--fixtures-only") {
        benchmark();
    }
}
