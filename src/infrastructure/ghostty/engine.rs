use super::super::terminal_support::{current_terminal_palette, indexed_color_with};
use crate::ports::terminal::*;
use anyhow::{Context, Result, bail};
use async_channel::Sender;
use std::{collections::VecDeque, ffi::c_void, ptr::NonNull, sync::Arc};

use super::{MAX_CLIPBOARD_STORE_BYTES, MAX_TERMINAL_TITLE_BYTES};

#[repr(C)]
#[derive(Default)]
pub(super) struct Info {
    columns: u16,
    rows: u16,
    cursor_x: u16,
    cursor_y: u16,
    cursor_visible: u8,
    cursor_blinking: u8,
    cursor_style: u8,
    kitty: u8,
    history: u64,
    offset: u64,
    modes: u32,
    painted_cells: u32,
}
#[repr(C)]
pub(super) struct Cell {
    foreground: [u8; 3],
    background: [u8; 3],
    underline_color: [u8; 3],
    bold: u8,
    italic: u8,
    underline: u8,
    strikeout: u8,
    hidden: u8,
    wide_spacer: u8,
    selected: u8,
}
type EventFn = unsafe extern "C" fn(*mut c_void, i32, *const u8, usize);
type PaintFn =
    unsafe extern "C" fn(*mut c_void, u16, u16, *const Cell, *const u8, usize, *const u8, usize);
unsafe extern "C" {
    pub(super) fn vg_new(c: u16, r: u16, event: EventFn, data: *mut c_void) -> *mut c_void;
    pub(super) fn vg_free(p: *mut c_void);
    pub(super) fn vg_feed(p: *mut c_void, s: *const u8, n: usize);
    pub(super) fn vg_resize(p: *mut c_void, c: u16, r: u16, w: u32, h: u32) -> i32;
    pub(super) fn vg_palette(p: *mut c_void, rgb: *const u8) -> i32;
    pub(super) fn vg_snapshot(
        p: *mut c_void,
        info: *mut Info,
        paint: Option<PaintFn>,
        data: *mut c_void,
        force: i32,
    ) -> i32;
    pub(super) fn vg_scroll(p: *mut c_void, delta: i64);
    pub(super) fn vg_clear_history(p: *mut c_void) -> i32;
    pub(super) fn vg_select(
        p: *mut c_void,
        action: i32,
        kind: i32,
        x: u16,
        y: u16,
        right: i32,
    ) -> i32;
    pub(super) fn vg_search(p: *mut c_void, s: *const u8, n: usize, previous: i32) -> i32;
    pub(super) fn vg_search_step(
        p: *mut c_void,
        s: *const u8,
        n: usize,
        previous: i32,
        continuation: i32,
    ) -> i32;
    pub(super) fn vg_text(p: *mut c_void, n: *mut usize, selection: i32) -> *mut u8;
    pub(super) fn vg_buffer_free(p: *mut u8, n: usize);
    pub(super) fn vg_recent_text(p: *mut c_void, n: *mut usize, lines: usize) -> *mut u8;
}
pub(super) fn checked(code: i32) -> Result<()> {
    if code != 0 {
        bail!("libghostty-vt returned {code}")
    }
    Ok(())
}
pub(super) struct Callbacks {
    pub(super) events: Sender<TerminalEvent>,
    pub(super) replies: VecDeque<u8>,
}
pub(super) unsafe fn bytes<'a>(p: *const u8, n: usize) -> &'a [u8] {
    if n == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(p, n) }
    }
}
unsafe extern "C" fn event(data: *mut c_void, kind: i32, p: *const u8, n: usize) {
    // SAFETY: userdata points to a stable Box owned by Engine; terminal calls
    // and destruction are serialized by the engine mutex.
    let context = unsafe { &mut *data.cast::<Callbacks>() };
    let data = unsafe { bytes(p, n) };
    match kind {
        0 => context.replies.extend(data),
        1 => {
            let _ = context.events.try_send(TerminalEvent::Bell);
        }
        2 => {
            if data.len() > MAX_TERMINAL_TITLE_BYTES {
                return;
            }
            let title = String::from_utf8_lossy(data).into_owned();
            let _ = context.events.try_send(if title.is_empty() {
                TerminalEvent::ResetTitle
            } else {
                TerminalEvent::Title(title)
            });
        }
        3 => {
            if data.len() > MAX_CLIPBOARD_STORE_BYTES {
                return;
            }
            let _ = context.events.try_send(TerminalEvent::ClipboardStore(
                String::from_utf8_lossy(data).into_owned(),
            ));
        }
        4 => {
            if let Some((prefix, suffix)) = clipboard_template(data) {
                let empty_response = format!("{prefix}{suffix}");
                if context
                    .events
                    .try_send(TerminalEvent::ClipboardLoad(Arc::new(move |text| {
                        use base64::Engine as _;
                        format!(
                            "{}{}{}",
                            prefix,
                            base64::engine::general_purpose::STANDARD.encode(text),
                            suffix
                        )
                    })))
                    .is_err()
                {
                    // The parser already denied the synchronous read. If the
                    // UI queue is full, still complete the OSC 52 query.
                    context.replies.extend(empty_response.bytes());
                }
            }
        }
        _ => {}
    }
}

// Only accept an empty OSC 52 response. Destination is limited to the protocol's
// clipboard selectors; arbitrary terminal output cannot become a reply template.
pub(super) fn clipboard_template(data: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(data).ok()?;
    let body = text.strip_prefix("\x1b]52;")?;
    let (destination, suffix) = body.split_once(';')?;
    if !destination.bytes().all(|c| b"cps01234567".contains(&c))
        || !matches!(suffix, "\x07" | "\x1b\\")
    {
        return None;
    }
    Some((format!("\x1b]52;{destination};"), suffix.to_owned()))
}
unsafe extern "C" fn paint(
    data: *mut c_void,
    x: u16,
    y: u16,
    cell: *const Cell,
    p: *const u8,
    n: usize,
    uri: *const u8,
    uri_len: usize,
) {
    let lines = unsafe { &mut *data.cast::<Vec<Arc<[TerminalCell]>>>() };
    let c = unsafe { &*cell };
    let Some(row) = lines.get_mut(y as usize) else {
        return;
    };
    let Some(slot) = Arc::make_mut(row).get_mut(x as usize) else {
        return;
    };
    let rgb = |a: [u8; 3]| TerminalRgb::new(a[0], a[1], a[2]);
    let text = String::from_utf8_lossy(unsafe { bytes(p, n) });
    let mut out = TerminalCell::with_text(
        y as usize,
        x as usize,
        if text.is_empty() { " " } else { &text },
        rgb(c.foreground),
        rgb(c.background),
    );
    out.underline_color = rgb(c.underline_color);
    out.bold = c.bold != 0;
    out.italic = c.italic != 0;
    out.underline = match c.underline {
        1 => TerminalUnderline::Single,
        2 => TerminalUnderline::Double,
        3 => TerminalUnderline::Curly,
        4 => TerminalUnderline::Dotted,
        5 => TerminalUnderline::Dashed,
        _ => TerminalUnderline::None,
    };
    out.strikeout = c.strikeout != 0;
    out.hidden = c.hidden != 0;
    out.wide_spacer = c.wide_spacer != 0;
    out.selected = c.selected != 0;
    if uri_len != 0 {
        out.set_hyperlink(Some(&String::from_utf8_lossy(unsafe {
            bytes(uri, uri_len)
        })));
    }
    *slot = out;
}
pub(super) struct Engine {
    pub(super) ptr: NonNull<c_void>,
    pub(super) callbacks: Box<Callbacks>,
    pub(super) cache: Option<Arc<TerminalSnapshot>>,
    pub(super) dirty: bool,
    pub(super) theme_generation: u64,
    pub(super) size: TerminalSize,
    #[cfg(test)]
    pub(super) last_painted_cells: u32,
}
// SAFETY: no upstream state is accessed concurrently. Engine is only exposed
// through Mutex, and borrowed callback/string pointers never escape a call.
unsafe impl Send for Engine {}
impl Drop for Engine {
    fn drop(&mut self) {
        unsafe { vg_free(self.ptr.as_ptr()) }
    }
}
impl Engine {
    pub(super) fn new(size: TerminalSize, events: Sender<TerminalEvent>) -> Result<Self> {
        let mut callbacks = Box::new(Callbacks {
            events,
            replies: VecDeque::new(),
        });
        let ptr = NonNull::new(unsafe {
            vg_new(
                size.columns,
                size.rows,
                event,
                (&mut *callbacks as *mut Callbacks).cast(),
            )
        })
        .context("could not create libghostty-vt")?;
        let mut engine = Self {
            ptr,
            callbacks,
            cache: None,
            dirty: true,
            theme_generation: u64::MAX,
            size,
            #[cfg(test)]
            last_painted_cells: 0,
        };
        engine.resize(size)?;
        engine.update_palette()?;
        Ok(engine)
    }
    pub(super) fn p(&self) -> *mut c_void {
        self.ptr.as_ptr()
    }
    fn update_palette(&mut self) -> Result<()> {
        let (generation, palette) = current_terminal_palette();
        if self.theme_generation == generation {
            return Ok(());
        }
        let mut rgb = Vec::with_capacity(259 * 3);
        for i in 0..259 {
            let c = indexed_color_with(i, &palette);
            rgb.extend([c.red, c.green, c.blue]);
        }
        checked(unsafe { vg_palette(self.p(), rgb.as_ptr()) })?;
        self.theme_generation = generation;
        self.dirty = true;
        Ok(())
    }
    pub(super) fn feed(&mut self, data: &[u8]) {
        unsafe { vg_feed(self.p(), data.as_ptr(), data.len()) };
        self.dirty = true;
    }
    pub(super) fn resize(&mut self, size: TerminalSize) -> Result<()> {
        checked(unsafe {
            vg_resize(
                self.p(),
                size.columns.max(1),
                size.rows.max(1),
                size.cell_width.max(1.0) as u32,
                size.cell_height.max(1.0) as u32,
            )
        })?;
        self.size = size;
        self.dirty = true;
        Ok(())
    }
    pub(super) fn snapshot(&mut self) -> Result<Arc<TerminalSnapshot>> {
        self.update_palette()?;
        if !self.dirty
            && let Some(cache) = &self.cache
        {
            return Ok(cache.clone());
        }
        let mut info = Info::default();
        checked(unsafe { vg_snapshot(self.p(), &mut info, None, std::ptr::null_mut(), 0) })?;
        let force = self
            .cache
            .as_ref()
            .is_none_or(|s| s.columns != info.columns as usize || s.rows != info.rows as usize);
        let mut lines: Vec<Arc<[TerminalCell]>> = if force {
            (0..info.rows as usize)
                .map(|y| {
                    (0..info.columns as usize)
                        .map(|x| TerminalCell::blank(y, x))
                        .collect::<Vec<_>>()
                        .into()
                })
                .collect()
        } else {
            self.cache
                .as_ref()
                .map(|snapshot| snapshot.lines.clone())
                .unwrap_or_default()
        };
        checked(unsafe {
            vg_snapshot(
                self.p(),
                &mut info,
                Some(paint),
                (&mut lines as *mut Vec<Arc<[TerminalCell]>>).cast(),
                force.into(),
            )
        })?;
        if let Some(old) = &self.cache {
            for (line, previous) in lines.iter_mut().zip(&old.lines) {
                if !Arc::ptr_eq(line, previous) && line.as_ref() == previous.as_ref() {
                    *line = previous.clone();
                }
            }
        }
        #[cfg(test)]
        {
            self.last_painted_cells = info.painted_cells;
        }
        let cursor = (info.cursor_visible != 0).then_some(TerminalCursor {
            row: info.cursor_y as usize,
            column: info.cursor_x as usize,
            shape: match info.cursor_style {
                0 => TerminalCursorShape::Beam,
                2 => TerminalCursorShape::Underline,
                3 => TerminalCursorShape::HollowBlock,
                _ => TerminalCursorShape::Block,
            },
            blinking: info.cursor_blinking != 0,
        });
        let snapshot = Arc::new(TerminalSnapshot {
            columns: info.columns as usize,
            rows: info.rows as usize,
            lines,
            cursor,
            display_offset: info.offset as usize,
            history_size: info.history as usize,
        });
        self.cache = Some(snapshot.clone());
        self.dirty = false;
        Ok(snapshot)
    }
    #[cfg(test)]
    pub(super) fn mode(&self) -> TerminalInputMode {
        self.try_mode().unwrap()
    }
    pub(super) fn try_mode(&self) -> Result<TerminalInputMode> {
        let mut i = Info::default();
        checked(unsafe { vg_snapshot(self.p(), &mut i, None, std::ptr::null_mut(), 0) })?;
        let m = |b: u32| i.modes & (1u32 << b) != 0u32;
        let k = |b: u32| i.kitty & (1u8 << b) != 0u8;
        Ok(TerminalInputMode {
            application_cursor: m(0),
            bracketed_paste: m(1),
            alternate_screen: m(2),
            alternate_scroll: m(3),
            focus_reporting: m(4),
            mouse_report_click: m(5),
            mouse_drag: m(6),
            mouse_motion: m(7),
            sgr_mouse: m(8),
            utf8_mouse: m(9),
            disambiguate_escape_codes: k(0),
            report_event_types: k(1),
            report_alternate_keys: k(2),
            report_all_keys_as_escape_codes: k(3),
            report_associated_text: k(4),
        })
    }
    pub(super) fn text(&self, selection: bool) -> Option<String> {
        let mut n = 0;
        let p = unsafe { vg_text(self.p(), &mut n, selection.into()) };
        if p.is_null() {
            return None;
        }
        let s = String::from_utf8_lossy(unsafe { bytes(p, n) }).into_owned();
        unsafe { vg_buffer_free(p, n) };
        Some(s)
    }
    #[cfg(test)]
    pub(super) fn select(
        &mut self,
        action: i32,
        kind: TerminalSelectionType,
        point: TerminalPoint,
        side: TerminalCellSide,
    ) {
        self.try_select(action, kind, point, side).unwrap();
    }
    pub(super) fn try_select(
        &mut self,
        action: i32,
        kind: TerminalSelectionType,
        point: TerminalPoint,
        side: TerminalCellSide,
    ) -> Result<()> {
        let kind = match kind {
            TerminalSelectionType::Simple => 0,
            TerminalSelectionType::Block => 1,
            TerminalSelectionType::Semantic => 2,
            TerminalSelectionType::Lines => 3,
        };
        let x = point
            .column
            .min(self.size.columns.saturating_sub(1) as usize) as u16;
        let y = point.row.min(self.size.rows.saturating_sub(1) as usize) as u16;
        checked(unsafe {
            vg_select(
                self.p(),
                action,
                kind,
                x,
                y,
                (side == TerminalCellSide::Right).into(),
            )
        })?;
        self.dirty = true;
        Ok(())
    }
}
