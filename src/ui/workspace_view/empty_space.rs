//! A space with no terminal open: a way to open one, over a quiet field of
//! twinkling tiles where a faint snake plays by itself. Clicking the field
//! opens it over the whole card and the faint snake becomes the player's;
//! leaving closes it again and hands the snake back. The resting field runs
//! on a slow tick and a game asks for a frame only on each step, both only
//! while in sight in the active window.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Div, MouseButton, MouseDownEvent, Timer, Window, canvas, div, prelude::*,
    px, svg,
};

use crate::ui::retro::snake::{Heading, SnakePhase};
use crate::ui::retro::{
    Scene, board_grid, board_offset, field_clock, field_grid, field_seed, paint_field,
};
use crate::ui::theme::colors;

use super::chrome::ease_out_cubic;
use super::{WorkspaceSection, WorkspaceView};

/// The resting field's twinkle and its snake are slow, so ten frames a second
/// keep them smooth at a fraction of the cost of every display refresh.
const REST_FRAME: Duration = Duration::from_millis(100);
/// The resting snake moves every other frame: five cells a second.
const ATTRACT_EVERY: u32 = 2;
/// The button sits a little below the middle, where the field has faded.
const BUTTON_DROP: f32 = 56.0;
/// How long the field takes to open into a game, or close back.
const STAGE_DURATION: Duration = Duration::from_millis(420);
const STAGE_FRAME: Duration = Duration::from_millis(16);

impl WorkspaceView {
    /// Whether the center shows the empty space, and with it the field.
    pub(super) fn arcade_shown(&self, cx: &Context<Self>) -> bool {
        self.workspace_section == WorkspaceSection::Workspace
            && self.snapshot.selected_tab().is_none()
            && !self.review_covers_terminal(cx)
    }

    /// Runs on every render. A game out of sight or in a background window
    /// pauses, and steps come only while one runs. The resting field
    /// twinkles on a slow tick, only while it is in sight in the active window.
    pub(super) fn sync_arcade_clock(&mut self, cx: &mut Context<Self>) {
        let in_sight = self.arcade_shown(cx) && self.window_is_active;
        if !in_sight {
            self.snake.pause();
        }
        let phase = self.snake.phase();
        let stage_target = if phase == SnakePhase::Title { 0.0 } else { 1.0 };
        if self.arcade_stage_target != stage_target {
            self.open_stage(stage_target, cx);
        }
        // The snake waits for the field to finish opening.
        if phase != SnakePhase::Playing || self.arcade_stage < 0.999 {
            self._arcade_step_task = None;
        } else if self._arcade_step_task.is_none() {
            self._arcade_step_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    let Ok(interval) = this.update(cx, |this, _| this.snake.interval()) else {
                        break;
                    };
                    Timer::after(interval).await;
                    if this.update(cx, |this, cx| this.step_snake(cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        if !(in_sight && phase == SnakePhase::Title) {
            self._arcade_rest_task = None;
        } else if self._arcade_rest_task.is_none() {
            self._arcade_rest_task = Some(cx.spawn(async move |this, cx| {
                let mut frame = 0_u32;
                loop {
                    Timer::after(REST_FRAME).await;
                    frame = frame.wrapping_add(1);
                    let alive = this.update(cx, |this, cx| {
                        if frame.is_multiple_of(ATTRACT_EVERY) {
                            let bounds = this.arcade_bounds.get();
                            let (columns, rows) = field_grid(
                                f32::from(bounds.size.width),
                                f32::from(bounds.size.height),
                            );
                            this.attract.step(columns, rows);
                        }
                        cx.notify();
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            }));
        }
    }

    /// Eases the field toward `to`: 1 open over the card, 0 at rest.
    fn open_stage(&mut self, to: f32, cx: &mut Context<Self>) {
        self.arcade_stage_target = to;
        let from = self.arcade_stage;
        let started = Instant::now();
        self._arcade_stage_task = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(STAGE_FRAME).await;
                let opening = this
                    .update(cx, |this, cx| {
                        let t = (started.elapsed().as_secs_f32() / STAGE_DURATION.as_secs_f32())
                            .min(1.0);
                        this.arcade_stage = from + (to - from) * ease_out_cubic(t);
                        cx.notify();
                        t < 1.0
                    })
                    .unwrap_or(false);
                if !opening {
                    break;
                }
            }
        }));
    }

    /// The field and board sizes, and where the board sits in the field.
    fn arcade_grids(&self) -> ((i32, i32), (i32, i32), (i32, i32)) {
        let bounds = self.arcade_bounds.get();
        let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let field = field_grid(width, height);
        let board = board_grid(width, height);
        (field, board, board_offset(field, board))
    }

    fn step_snake(&mut self, cx: &mut Context<Self>) {
        self.snake.step();
        if self.snake.best() > self.settings.snake_best {
            self.settings.snake_best = self.snake.best();
            self.persist_settings(cx);
        }
        cx.notify();
    }

    /// From the resting field, the faint snake becomes the player's, where
    /// it is; after a game, a fresh snake starts in the middle.
    pub(super) fn start_snake(&mut self) {
        let (_, (columns, rows), (dx, dy)) = self.arcade_grids();
        if self.snake.phase() != SnakePhase::Title {
            self.snake.start(columns, rows);
            return;
        }
        let body: Vec<(i32, i32)> = self.attract.body().map(|(x, y)| (x - dx, y - dy)).collect();
        let food = self.attract.food().map(|(x, y)| (x - dx, y - dy));
        let heading = self.attract.heading();
        self.snake.start_from(columns, rows, &body, heading, food);
    }

    /// Back to the resting field, which takes the player's snake with it.
    fn leave_snake(&mut self) {
        let ((columns, rows), _, (dx, dy)) = self.arcade_grids();
        let body: Vec<(i32, i32)> = self.snake.body().map(|(x, y)| (x + dx, y + dy)).collect();
        self.attract
            .adopt(&body, self.snake.heading(), columns, rows);
        self.snake.quit();
    }

    /// Keys for the game, while the empty space has the keyboard. Returns
    /// whether the key was the game's.
    pub(super) fn arcade_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let phase = self.snake.phase();
        let heading = match key {
            "up" | "w" | "k" => Some(Heading::Up),
            "down" | "s" | "j" => Some(Heading::Down),
            "left" | "a" | "h" => Some(Heading::Left),
            "right" | "d" | "l" => Some(Heading::Right),
            _ => None,
        };
        match (key, phase) {
            // The resting field stays a picture until it is clicked.
            ("space" | "enter", SnakePhase::Over) => self.start_snake(),
            ("space" | "enter" | "p", SnakePhase::Paused) => self.snake.resume(),
            ("space" | "p" | "escape", SnakePhase::Playing) => self.snake.pause(),
            ("escape", SnakePhase::Paused | SnakePhase::Over) => self.leave_snake(),
            (_, SnakePhase::Playing) if heading.is_some() => {
                self.snake.turn(heading.unwrap_or(Heading::Right));
                return true;
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn on_field_click(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The game takes its keys from the workspace itself.
        self.focus_handle.focus(window);
        match self.snake.phase() {
            SnakePhase::Title | SnakePhase::Over => self.start_snake(),
            SnakePhase::Paused => self.snake.resume(),
            SnakePhase::Playing => {}
        }
        cx.notify();
    }

    pub(super) fn empty_project_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let snake = self.snake.clone();
        let attract = self.attract.clone();
        let measured = self.arcade_bounds.clone();
        let hovered = self.arcade_hovered;
        let seed = field_seed(
            self.snapshot
                .selected_project()
                .map_or("", |project| project.name.as_str()),
        );
        let ink = colors().foreground;
        let stage = self.arcade_stage;
        let phase = self.snake.phase();
        let idle = matches!(phase, SnakePhase::Title | SnakePhase::Over);
        div()
            .id("snake-field")
            .size_full()
            .relative()
            .when(idle, |field| field.cursor_pointer())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.arcade_hovered = *hovered;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_field_click))
            .child(
                canvas(
                    move |bounds, _, _| measured.set(bounds),
                    move |bounds, _, window, _| {
                        let scene = Scene {
                            attract: &attract,
                            hovered,
                            seed,
                            time: field_clock(),
                            stage,
                        };
                        paint_field(&snake, scene, bounds, ink, window)
                    },
                )
                .absolute()
                .inset_0(),
            )
            // A game takes the whole card; at rest the way to a terminal
            // sits where the field fades out, and it fades with the field.
            .when(stage < 0.999, |field| {
                field.child(self.new_terminal_button(cx).opacity(1.0 - stage))
            })
            .into_any_element()
    }

    /// The way to open a terminal, over the faded part of the field.
    fn new_terminal_button(&self, cx: &mut Context<Self>) -> Div {
        let (label, shortcut) = if self.snapshot.selected_project().is_some() {
            ("New terminal", "⌘T")
        } else {
            ("New space", "⌘N")
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("empty-project-new-tab")
                    .mt(px(BUTTON_DROP))
                    .h(px(36.0))
                    .pl(px(12.0))
                    .pr(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(colors().foreground)
                    .hover(|button| button.bg(colors().hover))
                    // The button is not part of the field.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_terminal_tab_in_project(window, cx)
                    }))
                    .child(
                        svg()
                            .path("chrome-icons/terminal.svg")
                            .size(px(15.0))
                            .flex_none()
                            .text_color(colors().muted),
                    )
                    .child(label)
                    .child(
                        div()
                            .h(px(20.0))
                            .px(px(6.0))
                            .flex()
                            .items_center()
                            .rounded(px(5.0))
                            .bg(colors().hover)
                            .text_size(px(11.0))
                            .text_color(colors().muted)
                            .child(shortcut),
                    ),
            )
    }
}
