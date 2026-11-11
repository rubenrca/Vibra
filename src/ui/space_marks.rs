//! A space's mark: rings spreading from a drop, like water. The space's name
//! places the drop, spaces the rings, and picks the color, so a repository
//! keeps its mark in every space that opens it. The rings move while the
//! space's agents work.

use std::sync::OnceLock;
use std::time::Instant;

use gpui::{
    Bounds, Div, PathBuilder, Pixels, Point, Rgba, Window, canvas, fill, point, prelude::*, px,
    rgb, size,
};

use crate::ui::agent_marks::brand_tile;

/// What the agents in a space are doing, as its mark shows it. Later states
/// outrank earlier ones when a space runs several agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpaceActivity {
    /// No agent runs in the space: the rings fade back.
    Quiet,
    /// An agent is open but done: the rings hold still.
    Idle,
    /// An agent is working: the rings spread slowly.
    Working,
    /// An agent waits for an answer or a permission: the rings spread fast.
    NeedsYou,
}

impl SpaceActivity {
    /// Rings move only while an agent works or waits.
    pub fn moves(self) -> bool {
        matches!(self, Self::Working | Self::NeedsYou)
    }

    /// How far the rings travel per second, in tile units (a tile is 20 wide).
    fn speed(self) -> f32 {
        match self {
            Self::Working => 2.2,
            Self::NeedsYou => 7.0,
            Self::Quiet | Self::Idle => 0.0,
        }
    }
}

/// The drop's place on the tile, in tile units.
const CENTERS: [(f32, f32); 5] = [
    (10.0, 10.0),
    (4.5, 4.5),
    (15.5, 4.5),
    (4.5, 15.5),
    (15.5, 15.5),
];
const SPACINGS: [f32; 3] = [2.7, 3.3, 4.0];
/// Coral, tangerine, sun, lime, mint, teal, sky, iris, orchid, rose: each
/// deep enough to carry white rings, like the agents' brand tiles.
const HUES: [u32; 10] = [
    0xe5604a, 0xe9822b, 0xc99512, 0x55a334, 0x22a982, 0x1f9fb3, 0x3b87ec, 0x6466ec, 0xa35ee6,
    0xe2588f,
];
/// The rings and the drop, as the agents' marks are on their tiles.
const INK: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.96,
};
/// Tiles are drawn on a 20-unit grid and scaled to their size.
const GRID: f32 = 20.0;
/// Rings stay inside this rounded square, just within the tile's outline.
const CLIP_INSET: f32 = 1.5;
const CLIP_RADIUS: f32 = 4.5;
/// Rings fade out by this radius.
const FADE_RADIUS: f32 = 22.0;
const DOT_RADIUS: f32 = 1.5;
const RING_WIDTH: f32 = 1.1;
/// Points per ring before clipping.
const RING_SAMPLES: usize = 72;

/// The parts of a mark that come from the space's name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ripple {
    center: (f32, f32),
    spacing: f32,
    hue: Rgba,
}

impl Ripple {
    pub fn for_name(name: &str) -> Self {
        // The same picks, in the same order, as the design prototype.
        let mut random = Mulberry32(fnv1a(name));
        let center = CENTERS[random.index(CENTERS.len())];
        let spacing = SPACINGS[random.index(SPACINGS.len())];
        let hue = rgb(HUES[random.index(HUES.len())]);
        Self {
            center,
            spacing,
            hue,
        }
    }

    /// How far the rings have spread past their resting places.
    fn phase(self, activity: SpaceActivity) -> f32 {
        let travelled = clock_seconds() * f64::from(activity.speed());
        (travelled % f64::from(self.spacing)) as f32
    }

    fn paint(self, bounds: Bounds<Pixels>, ink: Rgba, phase: f32, fade: f32, window: &mut Window) {
        let unit = f32::from(bounds.size.width) / GRID;
        let at = |(x, y): (f32, f32)| -> Point<Pixels> {
            point(bounds.left() + px(x * unit), bounds.top() + px(y * unit))
        };
        let mut radius = phase + self.spacing;
        while radius < FADE_RADIUS {
            let alpha = (1.0 - radius / FADE_RADIUS) * fade;
            for run in clipped_ring(self.center, radius) {
                let mut path = PathBuilder::stroke(px(RING_WIDTH * unit));
                path.move_to(at(run[0]));
                for &position in &run[1..] {
                    path.line_to(at(position));
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, Rgba { a: alpha, ..ink });
                }
            }
            radius += self.spacing;
        }
        let (x, y) = self.center;
        let corner = at((x - DOT_RADIUS, y - DOT_RADIUS));
        let diameter = px(2.0 * DOT_RADIUS * unit);
        window.paint_quad(
            fill(Bounds::new(corner, size(diameter, diameter)), ink)
                .corner_radii(px(DOT_RADIUS * unit)),
        );
    }
}

/// A space's tile: the drop and its rings in white on a tile of its color,
/// built like the agents' brand tiles.
pub fn ripple_tile(name: &str, activity: SpaceActivity, tile_size: f32) -> Div {
    let ripple = Ripple::for_name(name);
    let phase = ripple.phase(activity);
    // With no agent in the space, the rings fall back and the drop stays.
    let fade = if activity == SpaceActivity::Quiet {
        0.45
    } else {
        1.0
    };
    let rings = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| ripple.paint(bounds, INK, phase, fade, window),
    )
    // Over the outline too, so the drawing uses the tile's own grid.
    .absolute()
    .top(px(-1.0))
    .left(px(-1.0))
    .size(px(tile_size))
    .into_any_element();
    brand_tile(ripple.hue, tile_size, rings).relative()
}

/// Seconds on one clock shared by every tile, so rings keep their place
/// between frames.
fn clock_seconds() -> f64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// The arcs of a ring that fall inside the tile, each as a run of points.
fn clipped_ring(center: (f32, f32), radius: f32) -> Vec<Vec<(f32, f32)>> {
    let at = |index: usize| {
        let angle = index as f32 / RING_SAMPLES as f32 * std::f32::consts::TAU;
        (
            center.0 + radius * angle.cos(),
            center.1 + radius * angle.sin(),
        )
    };
    let Some(start) = (0..RING_SAMPLES).find(|&index| !inside_tile(at(index))) else {
        // Wholly inside: one closed loop.
        return vec![
            (0..=RING_SAMPLES)
                .map(|index| at(index % RING_SAMPLES))
                .collect(),
        ];
    };
    let mut runs = Vec::new();
    let mut run: Vec<(f32, f32)> = Vec::new();
    let mut previous = at(start);
    for step in 1..=RING_SAMPLES {
        let position = at((start + step) % RING_SAMPLES);
        match (inside_tile(previous), inside_tile(position)) {
            (false, true) => run.extend([edge_between(position, previous), position]),
            (true, true) => run.push(position),
            (true, false) => {
                run.push(edge_between(previous, position));
                if run.len() > 1 {
                    runs.push(std::mem::take(&mut run));
                }
            }
            (false, false) => {}
        }
        previous = position;
    }
    runs
}

/// Whether a point lies inside the rounded square the rings are clipped to.
fn inside_tile((x, y): (f32, f32)) -> bool {
    let half = GRID / 2.0 - CLIP_INSET;
    let straight = half - CLIP_RADIUS;
    let dx = ((x - GRID / 2.0).abs() - straight).max(0.0);
    let dy = ((y - GRID / 2.0).abs() - straight).max(0.0);
    (x - GRID / 2.0).abs() <= half
        && (y - GRID / 2.0).abs() <= half
        && dx * dx + dy * dy <= CLIP_RADIUS * CLIP_RADIUS
}

/// The point where the segment from `inside` to `outside` leaves the tile,
/// so arcs end on the clip line instead of at the last sample.
fn edge_between(inside: (f32, f32), outside: (f32, f32)) -> (f32, f32) {
    let (mut near, mut far) = (inside, outside);
    for _ in 0..8 {
        let middle = ((near.0 + far.0) / 2.0, (near.1 + far.1) / 2.0);
        if inside_tile(middle) {
            near = middle;
        } else {
            far = middle;
        }
    }
    near
}

/// FNV-1a over UTF-8: stable across launches and Rust releases.
fn fnv1a(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// Mulberry32, matching the prototype's generator bit for bit.
struct Mulberry32(u32);

impl Mulberry32 {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let a = self.0;
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    fn index(&mut self, len: usize) -> usize {
        ((self.next() * len as f64) as usize).min(len - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_pick_the_same_marks_as_the_prototype() {
        // Reference picks from the prototype's JavaScript generator.
        let cases = [
            ("Vibra", 4, 0, 4),
            ("api", 1, 0, 1),
            ("landing", 0, 1, 0),
            ("scripts", 4, 2, 3),
            ("Ñandú", 4, 0, 0),
        ];
        for (name, center, spacing, hue) in cases {
            assert_eq!(
                Ripple::for_name(name),
                Ripple {
                    center: CENTERS[center],
                    spacing: SPACINGS[spacing],
                    hue: rgb(HUES[hue]),
                },
                "{name}"
            );
        }
    }

    #[test]
    fn rings_stay_inside_the_tile() {
        for (center, radius) in [((10.0, 10.0), 3.0), ((4.5, 4.5), 9.0), ((15.5, 15.5), 14.0)] {
            for run in clipped_ring(center, radius) {
                assert!(run.len() > 1);
                for point in run {
                    assert!(inside_tile(point), "{point:?} at r={radius}");
                }
            }
        }
        // A small ring at the center is one closed loop.
        let runs = clipped_ring((10.0, 10.0), 3.0);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].first(), runs[0].last());
        // A ring past every edge draws nothing.
        assert!(clipped_ring((10.0, 10.0), 15.0).is_empty());
    }

    #[test]
    fn only_working_and_waiting_spaces_move() {
        assert!(SpaceActivity::Working.moves());
        assert!(SpaceActivity::NeedsYou.moves());
        assert!(!SpaceActivity::Idle.moves());
        assert!(!SpaceActivity::Quiet.moves());
        let ripple = Ripple::for_name("Vibra");
        assert_eq!(ripple.phase(SpaceActivity::Idle), 0.0);
        assert!(ripple.phase(SpaceActivity::Working) < ripple.spacing);
    }
}
