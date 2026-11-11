//! Attract mode: a faint snake that plays by itself on the resting field,
//! like an arcade cabinet waiting for a coin. It heads for a lit tile,
//! sidesteps the walls and itself, and grows a little each time it eats.

use std::collections::VecDeque;

use super::snake::Heading;

/// It keeps to the upper part of the field, where the field is still visible.
const VISIBLE_SHARE: f32 = 0.55;
const START_LENGTH: usize = 6;
const MAX_LENGTH: usize = 16;
/// Chance per step, in percent, of a turn that is not the shortest way.
const WANDER_PERCENT: u32 = 12;

#[derive(Debug, Clone)]
pub struct Attract {
    columns: i32,
    rows: i32,
    body: VecDeque<(i32, i32)>,
    heading: Heading,
    food: (i32, i32),
    length: usize,
    random: u32,
}

impl Attract {
    pub fn new(seed: u32) -> Self {
        Self {
            columns: 0,
            rows: 0,
            body: VecDeque::new(),
            heading: Heading::Right,
            food: (0, 0),
            length: START_LENGTH,
            random: seed.max(1),
        }
    }

    /// Cells from head to tail.
    pub fn body(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.body.iter().copied()
    }

    pub fn food(&self) -> Option<(i32, i32)> {
        (!self.body.is_empty()).then_some(self.food)
    }

    /// Moves one cell on a field `columns` by `rows`.
    pub fn step(&mut self, columns: i32, rows: i32) {
        let rows = ((rows as f32 * VISIBLE_SHARE).ceil() as i32).min(rows);
        if columns < 4 || rows < 4 {
            self.body.clear();
            return;
        }
        if (columns, rows) != (self.columns, self.rows) || self.body.is_empty() {
            self.reset(columns, rows);
            return;
        }
        let Some(heading) = self.choose_heading() else {
            // Boxed in: start over somewhere else.
            self.reset(columns, rows);
            return;
        };
        self.heading = heading;
        let next = self.ahead(heading);
        self.body.push_front(next);
        if next == self.food {
            self.length = (self.length + 2).min(MAX_LENGTH);
            self.place_food();
        }
        while self.body.len() > self.length {
            self.body.pop_back();
        }
    }

    fn reset(&mut self, columns: i32, rows: i32) {
        self.columns = columns;
        self.rows = rows;
        self.length = START_LENGTH;
        let x = self.next_random() as i32 % columns;
        let y = self.next_random() as i32 % rows;
        self.heading = Heading::Right;
        self.body = VecDeque::from([(x, y)]);
        self.place_food();
    }

    fn ahead(&self, heading: Heading) -> (i32, i32) {
        let (x, y) = self.body[0];
        let (dx, dy) = heading.delta();
        (x + dx, y + dy)
    }

    fn open(&self, cell: (i32, i32)) -> bool {
        let inside = cell.0 >= 0 && cell.1 >= 0 && cell.0 < self.columns && cell.1 < self.rows;
        // The tail leaves its cell this step.
        let tail = self.body.len().saturating_sub(1);
        inside && !self.body.iter().take(tail).any(|&part| part == cell)
    }

    /// Straight on or a quarter turn, toward the food unless it wanders.
    fn choose_heading(&mut self) -> Option<Heading> {
        let candidates: Vec<Heading> = [
            self.heading,
            self.heading.turned(),
            self.heading.turned().opposite(),
        ]
        .into_iter()
        .filter(|&heading| self.open(self.ahead(heading)))
        .collect();
        if candidates.is_empty() {
            return None;
        }
        if self.next_random() % 100 < WANDER_PERCENT {
            let index = self.next_random() as usize % candidates.len();
            return Some(candidates[index]);
        }
        let distance =
            |cell: (i32, i32)| (cell.0 - self.food.0).abs() + (cell.1 - self.food.1).abs();
        candidates
            .into_iter()
            .min_by_key(|&heading| distance(self.ahead(heading)))
    }

    fn place_food(&mut self) {
        for _ in 0..32 {
            let cell = (
                self.next_random() as i32 % self.columns,
                self.next_random() as i32 % self.rows,
            );
            if !self.body.contains(&cell) {
                self.food = cell;
                return;
            }
        }
    }

    /// Xorshift32, kept positive for cell picks.
    fn next_random(&mut self) -> u32 {
        let mut x = self.random;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.random = x;
        x >> 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_roams_the_upper_field_without_leaving_it_or_biting_itself() {
        let mut attract = Attract::new(5);
        for _ in 0..5_000 {
            attract.step(40, 20);
            let body: Vec<_> = attract.body().collect();
            for &(x, y) in &body {
                assert!((0..40).contains(&x) && (0..11).contains(&y), "{x},{y}");
            }
            let mut unique = body.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), body.len(), "no cell twice");
            assert!(body.len() <= MAX_LENGTH);
        }
    }

    #[test]
    fn it_eats_and_grows() {
        let mut attract = Attract::new(17);
        let mut longest = 0;
        for _ in 0..3_000 {
            attract.step(30, 16);
            longest = longest.max(attract.body().count());
        }
        assert!(longest > START_LENGTH, "it found food");
    }

    #[test]
    fn a_tiny_field_shows_no_snake() {
        let mut attract = Attract::new(3);
        attract.step(3, 3);
        assert_eq!(attract.body().count(), 0);
        assert_eq!(attract.food(), None);
    }
}
