//! A quiet field of tiles behind an empty space, a few of them twinkling and
//! a faint snake playing by itself, all fading out toward the middle. It is
//! Snake's board: click it and the field opens over the whole card while the
//! faint snake brightens into yours. Everything is drawn in the theme's ink
//! at low strength, so it reads as texture until someone goes looking.

pub mod attract;
pub mod font;
pub mod snake;

use std::sync::OnceLock;
use std::time::Instant;

use gpui::{BorderStyle, Bounds, Pixels, Rgba, Window, fill, outline, point, px, size};

use self::attract::Attract;
use self::snake::{MIN_COLUMNS, MIN_ROWS, Snake, SnakePhase};

/// A field cell: a tile and the gap around it.
pub const CELL: f32 = 14.0;
const TILE_GAP: f32 = 3.0;
/// Past this the board stops growing, so a huge window stays playable.
const MAX_COLUMNS: i32 = 120;
const MAX_ROWS: i32 = 90;
/// Share of the resting field's tiles that are lit, in percent.
const LIT_PERCENT: u32 = 7;
/// Strengths of the resting field's lit tiles.
const LIT_LEVELS: [f32; 3] = [0.10, 0.18, 0.28];
/// Strength of every unlit tile.
const TILE_ALPHA: f32 = 0.035;
/// How sharply the resting field fades, and the share of its height it
/// reaches before it is gone.
const FADE_POWER: f32 = 1.4;
const FADE_REACH: f32 = 0.62;
/// A snake's strength at its head and at its tail, and its food's: faint
/// while it plays by itself, full in a game.
const ATTRACT_HEAD: f32 = 0.42;
const ATTRACT_TAIL: f32 = 0.1;
const ATTRACT_FOOD: f32 = 0.32;
const GAME_HEAD: f32 = 0.95;
const GAME_TAIL: f32 = 0.4;
const GAME_FOOD: f32 = 0.9;
/// The twinkle's strength behind a game, against its strength at rest.
const GAME_TWINKLE: f32 = 0.4;
/// Pixel size of the field's words, in points.
const TEXT_SCALE: f32 = 2.0;

/// The board that fits a field `width` by `height` points, in cells.
pub fn board_grid(width: f32, height: f32) -> (i32, i32) {
    let (columns, rows) = field_grid(width, height);
    (
        columns.clamp(MIN_COLUMNS, MAX_COLUMNS),
        rows.clamp(MIN_ROWS, MAX_ROWS),
    )
}

/// The tiles that fit `width` by `height` points.
pub fn field_grid(width: f32, height: f32) -> (i32, i32) {
    (
        (width / CELL).floor().max(0.0) as i32,
        (height / CELL).floor().max(0.0) as i32,
    )
}

/// A seed for the resting field, so each space lights its own tiles.
pub fn field_seed(name: &str) -> u32 {
    name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// Seconds on one clock shared by every field, so the twinkle keeps its
/// place between frames.
pub fn field_clock() -> f32 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    // Wrapped well before f32 loses the fraction of a second.
    (EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64() % 3_600.0) as f32
}

/// How lit a resting tile is at `time`: each one breathes at its own pace,
/// never fully out, so the field shimmers without blinking.
fn twinkle(hash: u32, time: f32) -> f32 {
    let speed = 0.5 + (hash >> 8 & 0xff) as f32 / 255.0 * 0.9;
    let offset = (hash >> 16 & 0xff) as f32 / 255.0 * std::f32::consts::TAU;
    0.25 + 0.75 * (0.5 + 0.5 * (time * speed + offset).sin())
}

/// What the field shows besides the game itself.
pub struct Scene<'a> {
    /// The snake that plays by itself at rest.
    pub attract: &'a Attract,
    pub hovered: bool,
    pub seed: u32,
    /// Drives the twinkle.
    pub time: f32,
    /// How far the field has opened into the game: 0 at rest, 1 in a game.
    /// Everything eases between the two, so starting a game grows the
    /// resting picture into the board instead of swapping it out.
    pub stage: f32,
}

fn mix(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

/// Paints the field and, during a game, Snake on it.
pub fn paint_field(
    game: &Snake,
    scene: Scene,
    bounds: Bounds<Pixels>,
    ink: Rgba,
    window: &mut Window,
) {
    let field = Field::new(bounds, ink);
    if field.columns <= 0 || field.rows <= 0 {
        return;
    }
    let Scene {
        attract,
        hovered,
        seed,
        time,
        stage,
    } = scene;
    let stage = stage.clamp(0.0, 1.0);
    let rest = 1.0 - stage;
    // Pointing at the resting field lifts it a little, the only hint it has.
    let lift = if hovered && game.phase() == SnakePhase::Title {
        1.0 + 0.5 * rest
    } else {
        1.0
    };
    // At rest the field fades out past the middle; it opens to the whole card.
    let fade = |row: i32| mix(field.fade(row), 1.0, stage);
    for row in 0..field.rows {
        for column in 0..field.columns {
            field.tile(column, row, TILE_ALPHA * lift * fade(row), window);
        }
    }
    // The twinkle stays behind a game, quieter, so both look like one place.
    let twinkle_strength = mix(1.0, GAME_TWINKLE, stage);
    for row in 0..field.rows {
        for column in 0..field.columns {
            let hash = cell_hash(column, row, seed);
            if hash % 100 < LIT_PERCENT {
                let level = LIT_LEVELS[(hash / 100) as usize % LIT_LEVELS.len()];
                let alpha = level * twinkle(hash, time) * twinkle_strength * lift * fade(row);
                field.tile(column, row, alpha, window);
            }
        }
    }
    if game.phase() == SnakePhase::Title {
        let snake = Brightness::at(stage);
        field.snake(
            attract.body(),
            attract.food(),
            (0, 0),
            Brightness {
                head: snake.head * lift,
                tail: snake.tail * lift,
                food: snake.food * lift,
            },
            &fade,
            window,
        );
        if hovered {
            field.words("CLICK TO PLAY", field.height * 0.26, 0.45 * rest, window);
        }
        return;
    }
    let dim = match game.phase() {
        SnakePhase::Paused => 0.3,
        SnakePhase::Over => 0.25,
        _ => 1.0,
    };
    field.board(game, Brightness::at(stage).dimmed(dim), &fade, window);
    match game.phase() {
        SnakePhase::Playing => {
            let score = format!(
                "{:04}   HI {:04}",
                game.score(),
                game.best().max(game.score())
            );
            let width = font::text_columns(&score) as f32 * TEXT_SCALE;
            field.text_at(
                &score,
                field.width - width - 12.0,
                field.height - 7.0 * TEXT_SCALE - 8.0,
                0.4 * stage,
                window,
            );
        }
        SnakePhase::Paused => {
            let top = field.height * 0.4;
            field.words("PAUSED", top, 0.8, window);
            field.words("SPACE TO GO ON   ESC TO LEAVE", top + 26.0, 0.45, window);
        }
        SnakePhase::Over => {
            let line = if game.new_best() {
                format!("NEW HIGH SCORE {:04}", game.score())
            } else {
                format!("SCORE {:04}   HI {:04}", game.score(), game.best())
            };
            let top = field.height * 0.36;
            field.words("GAME OVER", top, 0.85, window);
            field.words(&line, top + 26.0, 0.55, window);
            field.words(
                "SPACE TO PLAY AGAIN   ESC TO LEAVE",
                top + 52.0,
                0.35,
                window,
            );
        }
        SnakePhase::Title => {}
    }
}

/// How strongly a snake and its food are drawn.
#[derive(Clone, Copy)]
struct Brightness {
    head: f32,
    tail: f32,
    food: f32,
}

impl Brightness {
    /// Faint at rest, full in a game, and in between while the field opens.
    fn at(stage: f32) -> Self {
        Self {
            head: mix(ATTRACT_HEAD, GAME_HEAD, stage),
            tail: mix(ATTRACT_TAIL, GAME_TAIL, stage),
            food: mix(ATTRACT_FOOD, GAME_FOOD, stage),
        }
    }

    fn dimmed(self, amount: f32) -> Self {
        Self {
            head: self.head * amount,
            tail: self.tail * amount,
            food: self.food * amount,
        }
    }
}

/// Where a board sits in a field, in cells, so its cells line up with the
/// field's tiles.
pub fn board_offset(field: (i32, i32), board: (i32, i32)) -> (i32, i32) {
    (
        ((field.0 - board.0) / 2).max(0),
        ((field.1 - board.1) / 2).max(0),
    )
}

/// The field's tiles, centered in its bounds, in the theme's ink.
struct Field {
    bounds: Bounds<Pixels>,
    width: f32,
    height: f32,
    columns: i32,
    rows: i32,
    left: f32,
    ink: Rgba,
}

impl Field {
    fn new(bounds: Bounds<Pixels>, ink: Rgba) -> Self {
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        let (columns, rows) = field_grid(width, height);
        Self {
            bounds,
            width,
            height,
            columns,
            rows,
            left: (width - columns as f32 * CELL) / 2.0,
            ink,
        }
    }

    /// The resting field thins out and is gone a little past the middle.
    fn fade(&self, row: i32) -> f32 {
        (1.0 - row as f32 / (self.rows as f32 * FADE_REACH))
            .max(0.0)
            .powf(FADE_POWER)
    }

    fn rect(&self, x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(
            point(self.bounds.left() + px(x), self.bounds.top() + px(y)),
            size(px(width), px(height)),
        )
    }

    fn tile(&self, column: i32, row: i32, alpha: f32, window: &mut Window) {
        if alpha <= 0.005 {
            return;
        }
        let side = CELL - TILE_GAP;
        let x = self.left + column as f32 * CELL + TILE_GAP / 2.0;
        let y = row as f32 * CELL + TILE_GAP / 2.0;
        window.paint_quad(
            fill(
                self.rect(x, y, side, side),
                Rgba {
                    a: alpha,
                    ..self.ink
                },
            )
            .corner_radii(px(1.5)),
        );
    }

    /// A snake from head to tail, fading along its length, and its food.
    fn snake(
        &self,
        body: impl Iterator<Item = (i32, i32)> + Clone,
        food: Option<(i32, i32)>,
        (offset_column, offset_row): (i32, i32),
        brightness: Brightness,
        fade: &dyn Fn(i32) -> f32,
        window: &mut Window,
    ) {
        let length = body.clone().count().max(1) as f32;
        for (index, (column, row)) in body.enumerate() {
            let along = index as f32 / length;
            let alpha = mix(brightness.head, brightness.tail, along);
            let (column, row) = (offset_column + column, offset_row + row);
            self.tile(column, row, alpha * fade(row), window);
        }
        if let Some((column, row)) = food {
            let (column, row) = (offset_column + column, offset_row + row);
            self.tile(column, row, brightness.food * fade(row), window);
        }
    }

    /// The game's board, lined up with the field's tiles.
    fn board(
        &self,
        game: &Snake,
        brightness: Brightness,
        fade: &dyn Fn(i32) -> f32,
        window: &mut Window,
    ) {
        let (columns, rows) = game.grid();
        let offset = board_offset((self.columns, self.rows), (columns, rows));
        if columns < self.columns || rows < self.rows {
            let edge = self.rect(
                self.left + offset.0 as f32 * CELL,
                offset.1 as f32 * CELL,
                columns as f32 * CELL,
                rows as f32 * CELL,
            );
            window.paint_quad(outline(
                edge,
                Rgba {
                    a: 0.12 * brightness.head,
                    ..self.ink
                },
                BorderStyle::Solid,
            ));
        }
        self.snake(
            game.body(),
            Some(game.food()),
            offset,
            brightness,
            fade,
            window,
        );
    }

    /// A centered line in the bitmap font.
    fn words(&self, text: &str, top: f32, alpha: f32, window: &mut Window) {
        let left = (self.width - font::text_columns(text) as f32 * TEXT_SCALE) / 2.0;
        self.text_at(text, left, top, alpha, window);
    }

    fn text_at(&self, text: &str, left: f32, top: f32, alpha: f32, window: &mut Window) {
        let color = Rgba {
            a: alpha,
            ..self.ink
        };
        for (column, row) in font::lit_pixels(text) {
            window.paint_quad(fill(
                self.rect(
                    left + column as f32 * TEXT_SCALE,
                    top + row as f32 * TEXT_SCALE,
                    TEXT_SCALE,
                    TEXT_SCALE,
                ),
                color,
            ));
        }
    }
}

/// A stable pseudo-random number for a cell, so the resting field never
/// flickers between frames.
fn cell_hash(column: i32, row: i32, seed: u32) -> u32 {
    let mut x =
        (column as u32).wrapping_mul(0x9e37_79b1) ^ (row as u32).wrapping_mul(0x85eb_ca77) ^ seed;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_lines_up_with_the_field_within_playable_limits() {
        let (columns, rows) = board_grid(800.0, 280.0);
        assert_eq!((columns, rows), (57, 20));
        assert_eq!(board_grid(100.0, 60.0), (MIN_COLUMNS, MIN_ROWS));
        assert_eq!(board_grid(5_000.0, 5_000.0), (MAX_COLUMNS, MAX_ROWS));
        assert_eq!(board_grid(640.0, 600.0), (45, 42), "a game fills the card");
    }

    #[test]
    fn the_resting_field_is_sparse_and_the_same_every_frame() {
        let seed = field_seed("Vibra");
        let lit = (0..50)
            .flat_map(|row| (0..50).map(move |column| (column, row)))
            .filter(|&(column, row)| cell_hash(column, row, seed) % 100 < LIT_PERCENT)
            .count();
        assert!((80..=270).contains(&lit), "{lit} of 2500 lit");
        assert_eq!(cell_hash(3, 4, seed), cell_hash(3, 4, seed));
        assert_ne!(field_seed("Vibra"), field_seed("api"));
    }

    #[test]
    fn resting_tiles_breathe_but_never_go_out() {
        let hash = cell_hash(7, 2, field_seed("Vibra"));
        let samples: Vec<f32> = (0..200)
            .map(|step| twinkle(hash, step as f32 * 0.1))
            .collect();
        assert!(samples.iter().all(|&level| (0.25..=1.0).contains(&level)));
        let spread = samples.iter().cloned().fold(0.0, f32::max)
            - samples.iter().cloned().fold(1.0, f32::min);
        assert!(spread > 0.5, "it visibly breathes");
    }
}
