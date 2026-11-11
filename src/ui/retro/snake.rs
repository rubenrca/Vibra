//! Snake on a grid: steer the snake to the food, grow, and keep clear of the
//! walls and your own tail. It moves a cell at a time, so it needs a frame
//! only on each step, not every display refresh.

use std::collections::VecDeque;
use std::time::Duration;

/// Points for each food.
const FOOD_POINTS: u32 = 10;
/// Cells a food adds to the snake.
const GROWTH: u32 = 2;
const START_LENGTH: i32 = 4;
/// Step interval at the start and at full speed.
const SLOWEST_STEP_MS: u64 = 130;
const FASTEST_STEP_MS: u64 = 65;
/// Turns typed ahead of the snake that it remembers.
const QUEUED_TURNS: usize = 3;
pub const MIN_COLUMNS: i32 = 10;
pub const MIN_ROWS: i32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heading {
    Up,
    Down,
    Left,
    Right,
}

impl Heading {
    pub(super) fn delta(self) -> (i32, i32) {
        match self {
            Self::Up => (0, -1),
            Self::Down => (0, 1),
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
        }
    }

    /// A quarter turn: up and down become left, left and right become up.
    pub(super) fn turned(self) -> Self {
        match self {
            Self::Up | Self::Down => Self::Left,
            Self::Left | Self::Right => Self::Up,
        }
    }

    pub(super) fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnakePhase {
    /// The title screen, before a game.
    Title,
    Playing,
    Paused,
    /// The snake hit a wall or itself.
    Over,
}

#[derive(Debug, Clone)]
pub struct Snake {
    columns: i32,
    rows: i32,
    /// Cells from head to tail.
    body: VecDeque<(i32, i32)>,
    heading: Heading,
    turns: VecDeque<Heading>,
    food: (i32, i32),
    growing: u32,
    score: u32,
    best: u32,
    new_best: bool,
    phase: SnakePhase,
    random: u32,
}

impl Snake {
    /// A title screen remembering `best` as the high score.
    pub fn new(best: u32, seed: u32) -> Self {
        Self {
            columns: 0,
            rows: 0,
            body: VecDeque::new(),
            heading: Heading::Right,
            turns: VecDeque::new(),
            food: (0, 0),
            growing: 0,
            score: 0,
            best,
            new_best: false,
            phase: SnakePhase::Title,
            random: seed.max(1),
        }
    }

    pub fn phase(&self) -> SnakePhase {
        self.phase
    }

    pub fn score(&self) -> u32 {
        self.score
    }

    pub fn best(&self) -> u32 {
        self.best
    }

    /// Whether the last game set the high score.
    pub fn new_best(&self) -> bool {
        self.new_best
    }

    pub fn grid(&self) -> (i32, i32) {
        (self.columns, self.rows)
    }

    pub fn body(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.body.iter().copied()
    }

    pub fn food(&self) -> (i32, i32) {
        self.food
    }

    /// Starts a game on a grid `columns` by `rows`.
    pub fn start(&mut self, columns: i32, rows: i32) {
        self.columns = columns.max(MIN_COLUMNS);
        self.rows = rows.max(MIN_ROWS);
        let (x, y) = (self.columns / 2, self.rows / 2);
        self.body = (0..START_LENGTH).map(|offset| (x - offset, y)).collect();
        self.heading = Heading::Right;
        self.turns.clear();
        self.growing = 0;
        self.score = 0;
        self.new_best = false;
        self.place_food();
        self.phase = SnakePhase::Playing;
    }

    /// Queues a turn; the snake cannot fold back onto itself.
    pub fn turn(&mut self, heading: Heading) {
        if self.phase != SnakePhase::Playing || self.turns.len() >= QUEUED_TURNS {
            return;
        }
        let last = self.turns.back().copied().unwrap_or(self.heading);
        if heading != last && heading != last.opposite() {
            self.turns.push_back(heading);
        }
    }

    pub fn pause(&mut self) {
        if self.phase == SnakePhase::Playing {
            self.phase = SnakePhase::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.phase == SnakePhase::Paused {
            self.phase = SnakePhase::Playing;
        }
    }

    /// Leaves a game for the title screen.
    pub fn quit(&mut self) {
        self.phase = SnakePhase::Title;
    }

    /// How long until the next step: quicker as the score climbs.
    pub fn interval(&self) -> Duration {
        let faster = u64::from(self.score / FOOD_POINTS) * 3;
        Duration::from_millis(SLOWEST_STEP_MS.saturating_sub(faster).max(FASTEST_STEP_MS))
    }

    /// Moves the snake one cell.
    pub fn step(&mut self) {
        if self.phase != SnakePhase::Playing {
            return;
        }
        if let Some(turn) = self.turns.pop_front() {
            self.heading = turn;
        }
        let (dx, dy) = self.heading.delta();
        let (x, y) = self.body[0];
        let next = (x + dx, y + dy);
        let off_grid = next.0 < 0 || next.1 < 0 || next.0 >= self.columns || next.1 >= self.rows;
        // The tail moves out of the way this step unless the snake is growing.
        let tail_leaves = self.growing == 0;
        let length = self.body.len() - usize::from(tail_leaves);
        let bites_itself = self.body.iter().take(length).any(|&cell| cell == next);
        if off_grid || bites_itself {
            self.phase = SnakePhase::Over;
            if self.score > self.best {
                self.best = self.score;
                self.new_best = true;
            }
            return;
        }
        self.body.push_front(next);
        if next == self.food {
            self.score += FOOD_POINTS;
            self.growing += GROWTH;
            self.place_food();
        }
        if self.growing > 0 {
            self.growing -= 1;
        } else {
            self.body.pop_back();
        }
    }

    fn place_food(&mut self) {
        let free: Vec<(i32, i32)> = (0..self.rows)
            .flat_map(|y| (0..self.columns).map(move |x| (x, y)))
            .filter(|cell| !self.body.contains(cell))
            .collect();
        if free.is_empty() {
            return;
        }
        let index = self.next_random() as usize % free.len();
        self.food = free[index];
    }

    /// Xorshift32: plenty for placing food.
    fn next_random(&mut self) -> u32 {
        let mut x = self.random;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.random = x;
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> Snake {
        let mut snake = Snake::new(0, 9);
        snake.start(20, 12);
        snake
    }

    #[test]
    fn a_game_starts_mid_grid_heading_right_with_food_off_the_snake() {
        let snake = playing();
        assert_eq!(snake.phase(), SnakePhase::Playing);
        assert_eq!(
            snake.body().collect::<Vec<_>>(),
            [(10, 6), (9, 6), (8, 6), (7, 6)]
        );
        assert!(!snake.body().any(|cell| cell == snake.food()));
    }

    #[test]
    fn the_snake_moves_turns_and_never_folds_back() {
        let mut snake = playing();
        snake.food = (0, 0);
        snake.step();
        assert_eq!(snake.body().next(), Some((11, 6)));
        snake.turn(Heading::Left);
        snake.step();
        assert_eq!(snake.body().next(), Some((12, 6)), "reversing is ignored");
        snake.turn(Heading::Down);
        snake.turn(Heading::Left);
        snake.step();
        snake.step();
        assert_eq!(
            snake.body().next(),
            Some((11, 7)),
            "queued turns play in order"
        );
        assert_eq!(snake.body().count(), 4);
    }

    #[test]
    fn food_scores_grows_and_speeds_up() {
        let mut snake = playing();
        let slow = snake.interval();
        snake.food = (11, 6);
        snake.step();
        assert_eq!(snake.score(), 10);
        snake.food = (0, 0);
        snake.step();
        snake.step();
        assert_eq!(snake.body().count(), 6);
        assert!(snake.interval() < slow);
        snake.score = 10_000;
        assert_eq!(snake.interval(), Duration::from_millis(FASTEST_STEP_MS));
    }

    #[test]
    fn walls_end_the_game_and_keep_the_high_score() {
        let mut snake = playing();
        snake.food = (0, 0);
        snake.score = 30;
        for _ in 0..20 {
            snake.step();
        }
        assert_eq!(snake.phase(), SnakePhase::Over);
        assert_eq!(snake.best(), 30);
        assert!(snake.new_best());
        snake.start(20, 12);
        assert!(!snake.new_best());
        assert_eq!(snake.best(), 30);
    }

    #[test]
    fn biting_the_tail_ends_the_game_but_chasing_it_does_not() {
        let mut snake = playing();
        snake.food = (0, 0);
        // A tight loop: the head enters the cell the tail is leaving.
        snake.turn(Heading::Down);
        snake.step();
        snake.turn(Heading::Left);
        snake.step();
        snake.turn(Heading::Up);
        snake.step();
        assert_eq!(snake.phase(), SnakePhase::Playing);
        // Growing keeps the tail in place, so the same loop now bites it.
        let mut snake = playing();
        snake.food = (0, 0);
        snake.growing = 4;
        snake.turn(Heading::Down);
        snake.step();
        snake.turn(Heading::Left);
        snake.step();
        snake.turn(Heading::Up);
        snake.step();
        assert_eq!(snake.phase(), SnakePhase::Over);
    }

    #[test]
    fn pausing_freezes_the_snake() {
        let mut snake = playing();
        snake.pause();
        let before: Vec<_> = snake.body().collect();
        snake.step();
        assert_eq!(snake.body().collect::<Vec<_>>(), before);
        snake.turn(Heading::Down);
        assert!(snake.turns.is_empty(), "no turns while paused");
        snake.resume();
        assert_eq!(snake.phase(), SnakePhase::Playing);
        snake.quit();
        assert_eq!(snake.phase(), SnakePhase::Title);
    }
}
