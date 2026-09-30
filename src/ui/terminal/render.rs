use super::*;
use gpui::{
    Bounds, ElementInputHandler, Hsla, IntoElement, PaintQuad, Pixels, Render, ShapedLine,
    SharedString, TextRun, UnderlineStyle, Window, canvas, div, fill, outline, point, px, rgba,
    size,
};
use std::ops::Range;

#[derive(Debug)]
pub(crate) struct TerminalBackgroundRun {
    pub(crate) row: usize,
    pub(crate) columns: Range<usize>,
    pub(crate) color: Hsla,
}

struct TerminalPaintState {
    lines: Vec<ShapedLine>,
    /// Adjacent cells share a tint quad, including selections and TUI fills.
    backgrounds: Vec<TerminalBackgroundRun>,
    grid_bounds: Bounds<Pixels>,
    /// Live surface color for the canvas, including padding outside the grid.
    surface: Hsla,
    cursor: Option<PaintQuad>,
    cursor_bounds: Option<Bounds<Pixels>>,
    composition: Option<ShapedLine>,
    cursor_blinking: bool,
    cell_width: Pixels,
    line_height: Pixels,
    grid_size: (usize, usize),
}

impl TerminalPaintState {
    fn from_snapshot(
        snapshot: Arc<TerminalSnapshot>,
        bounds: Bounds<Pixels>,
        cache: &mut TerminalRenderCache,
        context: &TerminalShapeContext<'_>,
    ) -> Self {
        let cell_width = context.cell_width;
        let line_height = bounds.size.height / snapshot.rows.max(1) as f32;
        Self {
            lines: shape_snapshot_cached(cache, snapshot.clone(), context),
            backgrounds: collect_background_runs(&snapshot),
            grid_bounds: bounds,
            surface: snapshot_surface_color(&snapshot),
            cursor: snapshot.cursor.and_then(|cursor| {
                (!cursor.blinking || context.cursor_visible)
                    .then(|| cursor_quad(bounds, cursor, cell_width, line_height, context.focused))
                    .flatten()
            }),
            cursor_bounds: snapshot
                .cursor
                .map(|cursor| cursor_bounds(bounds, cursor, cell_width, line_height)),
            composition: None,
            cursor_blinking: snapshot.cursor.is_some_and(|cursor| cursor.blinking),
            cell_width,
            line_height,
            grid_size: (snapshot.columns, snapshot.rows),
        }
    }

    fn paint_cells(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(cursor) = self.cursor.take() {
            window.paint_quad(cursor);
        }
        for (row, line) in self.lines.iter().enumerate() {
            let origin = point(
                self.grid_bounds.left(),
                self.grid_bounds.top() + self.line_height * row,
            );
            let _ = line.paint(origin, self.line_height, window, cx);
        }
    }

    fn paint_backgrounds(&self, bounds: Bounds<Pixels>, floating: bool, window: &mut Window) {
        let grid = self.grid_bounds;
        let background = if floating {
            floating_surface(self.surface)
        } else {
            surface(self.surface)
        };
        window.paint_quad(fill(bounds, background).corner_radii(px(SURFACE_CORNER_RADIUS)));
        let scale = window.scale_factor();
        // Shared edges land on the same physical pixel, preventing translucent
        // seams when pane dimensions are not divisible by the terminal grid.
        let snap = |value: Pixels| (value * scale).round() / scale;
        for run in &self.backgrounds {
            if run.color == self.surface {
                continue;
            }
            let left = snap(grid.left() + self.cell_width * run.columns.start);
            let top = snap(grid.top() + self.line_height * run.row);
            let width = snap(grid.left() + self.cell_width * run.columns.end) - left;
            let height = snap(grid.top() + self.line_height * (run.row + 1)) - top;
            if width > px(0.0) && height > px(0.0) {
                window.paint_quad(fill(
                    Bounds::new(point(left, top), size(width, height)),
                    surface_tint(run.color.into(), self.surface.into()),
                ));
            }
        }
    }
}

fn terminal_grid_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let horizontal_padding = px(TERMINAL_HORIZONTAL_PADDING).min(bounds.size.width / 2.0);
    let vertical_padding = px(TERMINAL_VERTICAL_PADDING).min(bounds.size.height / 2.0);
    Bounds::new(
        point(
            bounds.left() + horizontal_padding,
            bounds.top() + vertical_padding,
        ),
        size(
            bounds.size.width - horizontal_padding * 2.0,
            bounds.size.height - vertical_padding * 2.0,
        ),
    )
}

#[derive(Default)]
pub(crate) struct TerminalRenderCache {
    snapshot: Option<Arc<TerminalSnapshot>>,
    lines: Vec<ShapedLine>,
    font_size: f32,
    cell_width: f32,
    focused: bool,
    cursor_visible: bool,
    theme_generation: u64,
}

struct TerminalShapeContext<'a> {
    base_font: &'a gpui::Font,
    font_size: Pixels,
    cell_width: Pixels,
    focused: bool,
    cursor_visible: bool,
    window: &'a Window,
}

fn search_overlay(
    query: String,
    marked_text: SharedString,
    found: bool,
    pending: bool,
) -> impl IntoElement {
    let status = if query.is_empty() {
        "Type to search"
    } else if pending {
        "Searching…"
    } else if found {
        "↵ next  ⇧↵ previous"
    } else {
        "No results"
    };
    let status_color = if found || pending || query.is_empty() {
        colors().muted
    } else {
        colors().danger
    };
    div()
        .absolute()
        .top_3()
        .left_3()
        .right_3()
        .max_w(px(420.0))
        .ml_auto()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(colors().border_subtle)
        .bg(popover_surface())
        .shadow_sm()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .truncate()
                .text_sm()
                .text_color(colors().foreground)
                .child(format!("Search  {query}{marked_text}")),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(status_color)
                .child(status),
        )
}

fn confirmation_overlay(confirmation: TerminalConfirmation) -> impl IntoElement {
    let title = confirmation.title();
    let preview = confirmation.preview();
    let hint = confirmation.hint();
    let warning = confirmation.warning();
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(colors().overlay())
        .child(
            div()
                .w(px(480.0))
                .max_w_full()
                .mx_4()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(colors().border_subtle)
                .bg(popover_surface())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().text_color(colors().foreground).child(title))
                .when_some(warning, |dialog, warning| {
                    dialog.child(div().text_xs().text_color(colors().danger).child(warning))
                })
                .child(
                    div()
                        .px_2()
                        .py_2()
                        .rounded_sm()
                        .bg(surface_tint(colors().terminal, colors().sidebar))
                        .text_xs()
                        .text_color(colors().muted)
                        .overflow_hidden()
                        .child(preview),
                )
                .child(div().text_xs().text_color(colors().muted).child(hint)),
        )
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.install_focus_observers(window, cx);
        let entity = cx.entity();
        let handle = self.handle.clone();
        let marked_text: SharedString = self.marked_text.clone().into();
        let canvas_marked_text = marked_text.clone();
        let error = self.error.clone();
        let input_error = self.input_error.clone();
        let exited = self.exited;
        let failed = self.failed;
        let search_active = self.search_active;
        let search_query = self.search_query.clone();
        let search_match_found = self.search_match_found;
        let search_pending = self.search_pending;
        let pending_confirmation = self.pending_confirmations.front().cloned();
        let cursor_visible = self.cursor_visible;
        let bell_active = self.bell_active;
        let hyperlink_hovered = self.hovered_hyperlink.is_some();
        let render_cache = self.render_cache.clone();
        let last_requested_size = self.last_requested_size.clone();
        let font_size = self.font_size;
        let line_height = font_size + (TERMINAL_LINE_HEIGHT - TERMINAL_FONT_SIZE);

        div()
            .id(SharedString::from(format!("terminal-{}", self.session_id)))
            .size_full()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .rounded(px(SURFACE_CORNER_RADIUS))
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .on_action(cx.listener(Self::copy_action))
            .on_action(cx.listener(Self::paste_action))
            .on_action(cx.listener(Self::search_action))
            .on_action(cx.listener(Self::search_next_action))
            .on_action(cx.listener(Self::search_previous_action))
            .on_action(cx.listener(Self::increase_font_size_action))
            .on_action(cx.listener(Self::decrease_font_size_action))
            .on_action(cx.listener(Self::reset_font_size_action))
            .on_action(cx.listener(Self::clear_scrollback_action))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .font_family(MONO_FONT)
            .font_weight(gpui::FontWeight::LIGHT)
            .text_size(px(font_size))
            .line_height(px(line_height))
            .border_1()
            .border_color(if bell_active {
                colors().danger
            } else {
                gpui::rgba(0x00000000)
            })
            .cursor(gpui::CursorStyle::IBeam)
            .when(hyperlink_hovered, |terminal| terminal.cursor_pointer())
            .child(
                canvas(
                    {
                        let entity = entity.clone();
                        move |bounds, window, cx| {
                            let bounds = terminal_grid_bounds(bounds);
                            let style = window.text_style();
                            let rem_size = window.rem_size();
                            let font_size = style.font_size.to_pixels(rem_size);
                            let line_height = style.line_height_in_pixels(rem_size);
                            let base_font = style.font();
                            let measure = window.text_system().shape_line(
                                "M".into(),
                                font_size,
                                &[TextRun {
                                    len: 1,
                                    font: base_font.clone(),
                                    color: style.color,
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                }],
                                None,
                            );
                            // Target grid from natural glyph metrics (what the PTY should be).
                            let natural_cell_width: f32 = measure.width.ceil().into();
                            let natural_line_height: f32 = line_height.into();
                            let width: f32 = bounds.size.width.into();
                            let height: f32 = bounds.size.height.into();
                            let target_columns = (width / natural_cell_width)
                                .floor()
                                .clamp(2.0, u16::MAX as f32)
                                as u16;
                            let target_rows = (height / natural_line_height)
                                .floor()
                                .clamp(1.0, u16::MAX as f32)
                                as u16;
                            if let Some(handle) = &handle
                                && !failed
                                && !exited
                            {
                                let size = TerminalSize {
                                    columns: target_columns,
                                    rows: target_rows,
                                    cell_width: natural_cell_width,
                                    cell_height: natural_line_height,
                                };
                                let mut last = last_requested_size
                                    .lock()
                                    .expect("terminal resize cache poisoned");
                                if *last != Some(size) && handle.resize(size).is_ok() {
                                    *last = Some(size);
                                }
                            }
                            let snapshot = entity.read(cx).snapshot();
                            // Stretch paint metrics to the *live* snapshot grid so every
                            // cell covers the pane. Using target_* while the PTY still has
                            // fewer columns/rows left a gray strip where full-screen TUIs
                            // looked cut off (cols_snap * width/cols_target < width).
                            let paint_columns = snapshot.columns.max(1) as f32;
                            let cell_width = px(width / paint_columns);
                            let focused = entity.read(cx).focus_handle.is_focused(window);
                            let shape_context = TerminalShapeContext {
                                base_font: &base_font,
                                font_size,
                                cell_width,
                                focused,
                                cursor_visible,
                                window,
                            };
                            let mut state = TerminalPaintState::from_snapshot(
                                snapshot,
                                bounds,
                                &mut render_cache.lock().expect("terminal render cache poisoned"),
                                &shape_context,
                            );
                            state.composition = (!search_active && !canvas_marked_text.is_empty())
                                .then(|| {
                                    window.text_system().shape_line(
                                        canvas_marked_text.clone(),
                                        font_size,
                                        &[TextRun {
                                            len: canvas_marked_text.len(),
                                            font: base_font,
                                            color: colors().foreground.into(),
                                            background_color: Some(colors().elevated.into()),
                                            underline: Some(UnderlineStyle {
                                                thickness: px(1.0),
                                                color: Some(colors().foreground.into()),
                                                wavy: false,
                                            }),
                                            strikethrough: None,
                                        }],
                                        Some(cell_width),
                                    )
                                });
                            state
                        }
                    },
                    move |bounds, mut state, window, cx| {
                        state.paint_backgrounds(bounds, false, window);
                        let bounds = state.grid_bounds;
                        window.handle_input(
                            &entity.read(cx).focus_handle,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        state.paint_cells(window, cx);
                        if let (Some(composition), Some(cursor_bounds)) =
                            (state.composition, state.cursor_bounds)
                        {
                            let _ = composition.paint_background(
                                cursor_bounds.origin,
                                state.line_height,
                                window,
                                cx,
                            );
                            let _ = composition.paint(
                                cursor_bounds.origin,
                                state.line_height,
                                window,
                                cx,
                            );
                        }
                        entity.update(cx, |this, _| {
                            this.last_cursor_bounds = state.cursor_bounds;
                            this.last_terminal_bounds = Some(bounds);
                            this.last_cell_width = Some(state.cell_width);
                            this.last_line_height = Some(state.line_height);
                            this.last_grid_size = Some(state.grid_size);
                            this.cursor_blinking = state.cursor_blinking;
                        });
                    },
                )
                .size_full(),
            )
            .when(search_active, |terminal| {
                terminal.child(search_overlay(
                    search_query,
                    marked_text,
                    search_match_found,
                    search_pending,
                ))
            })
            .when_some(pending_confirmation, |terminal, confirmation| {
                terminal.child(confirmation_overlay(confirmation))
            })
            .when_some(error, |terminal, error| {
                terminal.child(
                    div()
                        .absolute()
                        .inset_0()
                        .p_5()
                        .font_family(MONO_FONT)
                        .text_sm()
                        .text_color(colors().danger)
                        .child(error),
                )
            })
            .when_some(input_error, |terminal, input_error| {
                terminal.child(
                    div()
                        .absolute()
                        .left_3()
                        .bottom_3()
                        .max_w(px(500.0))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(colors().danger)
                        .bg(popover_surface())
                        .text_xs()
                        .text_color(colors().danger)
                        .child(input_error),
                )
            })
            .when(exited, |terminal| {
                terminal.child(
                    div()
                        .absolute()
                        .right_3()
                        .bottom_3()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(popover_surface())
                        .text_xs()
                        .text_color(colors().muted)
                        .child("process exited"),
                )
            })
    }
}

impl Render for TerminalDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.snapshot.clone();
        let render_cache = self.render_cache.clone();
        let focused = self.focused;
        let cursor_visible = self.cursor_visible;
        let font_size = self.font_size;
        let line_height = font_size + (TERMINAL_LINE_HEIGHT - TERMINAL_FONT_SIZE);

        div()
            .w(px(self.width))
            .h(px(self.height))
            .relative()
            .overflow_hidden()
            .rounded(px(SURFACE_CORNER_RADIUS))
            .font_family(MONO_FONT)
            .font_weight(gpui::FontWeight::LIGHT)
            .text_size(px(font_size))
            .line_height(px(line_height))
            .child(
                canvas(
                    move |bounds, window, _| {
                        let style = window.text_style();
                        let rem_size = window.rem_size();
                        let font_size = style.font_size.to_pixels(rem_size);
                        let base_font = style.font();
                        let snapshot = snapshot.clone();
                        let paint_columns = snapshot.columns.max(1) as f32;
                        let width: f32 = bounds.size.width.into();
                        let cell_width = px(width / paint_columns);
                        let shape_context = TerminalShapeContext {
                            base_font: &base_font,
                            font_size,
                            cell_width,
                            focused,
                            cursor_visible,
                            window,
                        };
                        TerminalPaintState::from_snapshot(
                            snapshot,
                            bounds,
                            &mut render_cache
                                .lock()
                                .expect("terminal drag render cache poisoned"),
                            &shape_context,
                        )
                    },
                    move |bounds, mut state, window, cx| {
                        // Match the terminal canvas paint order, but keep this copy
                        // read-only so dragging never affects the live pane.
                        state.paint_backgrounds(bounds, true, window);
                        state.paint_cells(window, cx);
                    },
                )
                .size_full(),
            )
    }
}

fn shape_snapshot_cached(
    cache: &mut TerminalRenderCache,
    snapshot: Arc<TerminalSnapshot>,
    context: &TerminalShapeContext<'_>,
) -> Vec<ShapedLine> {
    let font_size_f32: f32 = context.font_size.into();
    let cell_width_f32: f32 = context.cell_width.into();
    let theme_generation = theme::generation();
    let full_rebuild = cache.lines.len() != snapshot.rows
        || cache.font_size != font_size_f32
        || cache.cell_width != cell_width_f32
        || cache.theme_generation != theme_generation;
    if full_rebuild {
        cache.lines.clear();
        cache.lines.reserve(snapshot.rows);
        for row in 0..snapshot.rows {
            cache
                .lines
                .push(shape_snapshot_line(&snapshot, row, context));
        }
    } else {
        for row in 0..snapshot.rows {
            let cells_changed = cache.snapshot.as_ref().is_none_or(|previous| {
                previous
                    .lines
                    .get(row)
                    .zip(snapshot.lines.get(row))
                    .is_none_or(|(previous, current)| previous.as_ref() != current.as_ref())
            });
            let cursor_row_changed = cache.snapshot.as_ref().is_some_and(|previous| {
                previous.cursor.map(|cursor| cursor.row) != snapshot.cursor.map(|cursor| cursor.row)
                    && (previous.cursor.is_some_and(|cursor| cursor.row == row)
                        || snapshot.cursor.is_some_and(|cursor| cursor.row == row))
            });
            let cursor_style_changed = (cache.focused != context.focused
                || cache.cursor_visible != context.cursor_visible)
                && snapshot.cursor.is_some_and(|cursor| cursor.row == row);
            if cells_changed || cursor_row_changed || cursor_style_changed {
                cache.lines[row] = shape_snapshot_line(&snapshot, row, context);
            }
        }
    }

    cache.snapshot = Some(snapshot);
    cache.font_size = font_size_f32;
    cache.cell_width = cell_width_f32;
    cache.focused = context.focused;
    cache.cursor_visible = context.cursor_visible;
    cache.theme_generation = theme_generation;
    cache.lines.clone()
}

fn shape_snapshot_line(
    snapshot: &TerminalSnapshot,
    row: usize,
    context: &TerminalShapeContext<'_>,
) -> ShapedLine {
    let cells = snapshot.lines.get(row).map(AsRef::as_ref).unwrap_or(&[]);
    let mut text = String::with_capacity(snapshot.columns);
    let mut runs = Vec::with_capacity(cells.len());
    for cell in cells {
        let piece = display_text(cell);
        let mut font = context.base_font.clone();
        if cell.bold {
            font.weight = gpui::FontWeight::MEDIUM;
        }
        if cell.italic {
            font = font.italic();
        }
        let cursor_block = context.focused
            && context.cursor_visible
            && snapshot.cursor.is_some_and(|cursor| {
                cursor.row == cell.row
                    && cursor.column == cell.column
                    && cursor.shape == TerminalCursorShape::Block
                    && (!cursor.blinking || context.cursor_visible)
            });
        let foreground = if cursor_block {
            cell.background
        } else if cell.selected {
            theme::to_terminal_rgb(colors().foreground)
        } else {
            cell.foreground
        };
        let underline = match cell.underline {
            TerminalUnderline::None => None,
            TerminalUnderline::Single | TerminalUnderline::Dotted | TerminalUnderline::Dashed => {
                Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(to_hsla(cell.underline_color)),
                    wavy: false,
                })
            }
            TerminalUnderline::Double => Some(UnderlineStyle {
                thickness: px(2.0),
                color: Some(to_hsla(cell.underline_color)),
                wavy: false,
            }),
            TerminalUnderline::Curly => Some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(to_hsla(cell.underline_color)),
                wavy: true,
            }),
        };
        // Backgrounds are painted as per-cell quads; keep runs transparent so
        // glyph advances never leave a background gap at the pane edge.
        runs.push(TextRun {
            len: piece.len(),
            font,
            color: to_hsla(foreground),
            background_color: None,
            underline,
            strikethrough: cell.strikeout.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(to_hsla(foreground)),
            }),
        });
        text.push_str(piece);
    }
    context.window.text_system().shape_line(
        text.into(),
        context.font_size,
        &runs,
        Some(context.cell_width),
    )
}

pub(crate) fn collect_background_runs(snapshot: &TerminalSnapshot) -> Vec<TerminalBackgroundRun> {
    let fallback = snapshot_surface_color(snapshot);
    let selection = colors().selection.into();
    let columns = snapshot.columns.max(1);
    let mut backgrounds: Vec<TerminalBackgroundRun> = Vec::new();
    for row in 0..snapshot.rows.max(1) {
        let line = snapshot.lines.get(row);
        for column in 0..columns {
            let color = line
                .and_then(|line| line.get(column))
                .map_or(fallback, |cell| {
                    if cell.selected {
                        selection
                    } else {
                        to_hsla(cell.background)
                    }
                });
            if let Some(previous) = backgrounds.last_mut()
                && previous.row == row
                && previous.color == color
            {
                previous.columns.end = column + 1;
            } else {
                backgrounds.push(TerminalBackgroundRun {
                    row,
                    columns: column..column + 1,
                    color,
                });
            }
        }
    }
    backgrounds
}

/// Match padding and missing cells to the live TUI without changing shell colors.
fn snapshot_surface_color(snapshot: &TerminalSnapshot) -> Hsla {
    let sample = snapshot
        .lines
        .last()
        .and_then(|line| line.first())
        .or_else(|| snapshot.lines.first().and_then(|line| line.first()))
        .map(|cell| cell.background);
    sample
        .map(to_hsla)
        .unwrap_or_else(|| colors().terminal.into())
}

fn display_text(cell: &TerminalCell) -> &str {
    if cell.hidden || cell.wide_spacer || cell.text().contains('\n') {
        " "
    } else {
        cell.text()
    }
}

fn cursor_bounds(
    bounds: Bounds<Pixels>,
    cursor: TerminalCursor,
    cell_width: Pixels,
    line_height: Pixels,
) -> Bounds<Pixels> {
    Bounds::new(
        point(
            bounds.left() + cell_width * cursor.column,
            bounds.top() + line_height * cursor.row,
        ),
        size(cell_width, line_height),
    )
}

fn cursor_quad(
    bounds: Bounds<Pixels>,
    cursor: TerminalCursor,
    cell_width: Pixels,
    line_height: Pixels,
    focused: bool,
) -> Option<PaintQuad> {
    if cursor.shape == TerminalCursorShape::Hidden {
        return None;
    }
    let bounds = cursor_bounds(bounds, cursor, cell_width, line_height);
    let color: Hsla = if focused {
        colors().foreground.into()
    } else {
        colors().subtle.into()
    };
    if !focused && cursor.shape == TerminalCursorShape::Block {
        return Some(outline(bounds, color, Default::default()));
    }
    Some(match cursor.shape {
        TerminalCursorShape::Block => fill(bounds, color),
        TerminalCursorShape::HollowBlock => outline(bounds, color, Default::default()),
        TerminalCursorShape::Underline => fill(
            Bounds::new(
                point(bounds.left(), bounds.bottom() - px(2.0)),
                size(bounds.size.width, px(2.0)),
            ),
            color,
        ),
        TerminalCursorShape::Beam => fill(
            Bounds::new(bounds.origin, size(px(2.0), bounds.size.height)),
            color,
        ),
        TerminalCursorShape::Hidden => return None,
    })
}

pub(crate) fn to_hsla(color: TerminalRgb) -> Hsla {
    rgba(
        (u32::from(color.red) << 24)
            | (u32::from(color.green) << 16)
            | (u32::from(color.blue) << 8)
            | 0xff,
    )
    .into()
}
