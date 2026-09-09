use uuid::Uuid;

use super::types::*;

#[derive(Debug, Clone, Copy)]
struct PaneRect {
    id: Uuid,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl PaneRect {
    fn center_x(self) -> f32 {
        self.x + self.width / 2.0
    }

    fn center_y(self) -> f32 {
        self.y + self.height / 2.0
    }
}

impl PaneLayoutSnapshot {
    pub fn terminal(id: Uuid) -> Self {
        Self::Terminal { id }
    }

    pub fn terminal_ids(&self) -> Vec<Uuid> {
        match self {
            Self::Terminal { id } => vec![*id],
            Self::Split { first, second, .. } => {
                let mut ids = first.terminal_ids();
                ids.extend(second.terminal_ids());
                ids
            }
        }
    }

    pub fn contains_terminal(&self, terminal_id: Uuid) -> bool {
        match self {
            Self::Terminal { id } => *id == terminal_id,
            Self::Split { first, second, .. } => {
                first.contains_terminal(terminal_id) || second.contains_terminal(terminal_id)
            }
        }
    }

    pub fn swap_terminals(&mut self, first: Uuid, second: Uuid) -> bool {
        if first == second || !self.contains_terminal(first) || !self.contains_terminal(second) {
            return false;
        }
        self.map_terminal_ids(&mut |id| {
            if id == first {
                second
            } else if id == second {
                first
            } else {
                id
            }
        });
        true
    }

    fn map_terminal_ids(&mut self, map: &mut impl FnMut(Uuid) -> Uuid) {
        match self {
            Self::Terminal { id } => *id = map(*id),
            Self::Split { first, second, .. } => {
                first.map_terminal_ids(map);
                second.map_terminal_ids(map);
            }
        }
    }

    pub fn split_terminal(
        &mut self,
        terminal_id: Uuid,
        new_terminal_id: Uuid,
        axis: WorkspaceSplitAxis,
        insert_first: bool,
    ) -> bool {
        if let Self::Terminal { id } = self {
            if *id != terminal_id {
                return false;
            }
            let existing = Self::terminal(*id);
            let inserted = Self::terminal(new_terminal_id);
            let (first, second) = if insert_first {
                (inserted, existing)
            } else {
                (existing, inserted)
            };
            *self = Self::Split {
                axis,
                ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(first),
                second: Box::new(second),
            };
            return true;
        }

        match self {
            Self::Split { first, second, .. } => {
                first.split_terminal(terminal_id, new_terminal_id, axis, insert_first)
                    || second.split_terminal(terminal_id, new_terminal_id, axis, insert_first)
            }
            Self::Terminal { .. } => false,
        }
    }

    pub fn adjacent_terminal(
        &self,
        terminal_id: Uuid,
        direction: PaneFocusDirection,
    ) -> Option<Uuid> {
        let mut rects = Vec::new();
        self.collect_rects(0.0, 0.0, 1.0, 1.0, &mut rects);
        let current = *rects.iter().find(|rect| rect.id == terminal_id)?;
        rects
            .into_iter()
            .filter(|candidate| candidate.id != terminal_id)
            .filter_map(|candidate| {
                directional_score(current, candidate, direction).map(|score| (candidate.id, score))
            })
            .min_by(|(_, left), (_, right)| left.total_cmp(right))
            .map(|(id, _)| id)
    }

    pub fn move_nearest_divider(
        &mut self,
        terminal_id: Uuid,
        axis: WorkspaceSplitAxis,
        delta: i16,
    ) -> bool {
        let Self::Split {
            axis: split_axis,
            ratio,
            first,
            second,
        } = self
        else {
            return false;
        };
        let child = if first.contains_terminal(terminal_id) {
            first
        } else if second.contains_terminal(terminal_id) {
            second
        } else {
            return false;
        };
        if child.move_nearest_divider(terminal_id, axis, delta) {
            return true;
        }
        if *split_axis != axis {
            return false;
        }
        let adjusted = (*ratio as i32 + i32::from(delta))
            .clamp(MIN_PANE_SPLIT_RATIO as i32, MAX_PANE_SPLIT_RATIO as i32)
            as u16;
        if adjusted == *ratio {
            return false;
        }
        *ratio = adjusted;
        true
    }

    pub fn set_split_ratio(&mut self, path: &[PaneBranch], ratio: u16) -> bool {
        let Self::Split {
            ratio: current,
            first,
            second,
            ..
        } = self
        else {
            return false;
        };
        let Some((branch, remaining)) = path.split_first() else {
            let ratio = ratio.clamp(MIN_PANE_SPLIT_RATIO, MAX_PANE_SPLIT_RATIO);
            if *current == ratio {
                return false;
            }
            *current = ratio;
            return true;
        };
        match branch {
            PaneBranch::First => first.set_split_ratio(remaining, ratio),
            PaneBranch::Second => second.set_split_ratio(remaining, ratio),
        }
    }

    pub fn equalize(&mut self) -> bool {
        match self {
            Self::Terminal { .. } => false,
            Self::Split {
                ratio,
                first,
                second,
                ..
            } => {
                let changed = *ratio != DEFAULT_PANE_SPLIT_RATIO;
                *ratio = DEFAULT_PANE_SPLIT_RATIO;
                first.equalize() | second.equalize() | changed
            }
        }
    }

    pub(super) fn normalize(&mut self) {
        if let Self::Split {
            ratio,
            first,
            second,
            ..
        } = self
        {
            *ratio = (*ratio).clamp(MIN_PANE_SPLIT_RATIO, MAX_PANE_SPLIT_RATIO);
            first.normalize();
            second.normalize();
        }
    }

    fn collect_rects(&self, x: f32, y: f32, width: f32, height: f32, output: &mut Vec<PaneRect>) {
        match self {
            Self::Terminal { id } => output.push(PaneRect {
                id: *id,
                x,
                y,
                width,
                height,
            }),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let fraction = f32::from(*ratio) / 10_000.0;
                match axis {
                    WorkspaceSplitAxis::Horizontal => {
                        let first_width = width * fraction;
                        first.collect_rects(x, y, first_width, height, output);
                        second.collect_rects(
                            x + first_width,
                            y,
                            width - first_width,
                            height,
                            output,
                        );
                    }
                    WorkspaceSplitAxis::Vertical => {
                        let first_height = height * fraction;
                        first.collect_rects(x, y, width, first_height, output);
                        second.collect_rects(
                            x,
                            y + first_height,
                            width,
                            height - first_height,
                            output,
                        );
                    }
                }
            }
        }
    }

    pub fn removing_terminal(&self, terminal_id: Uuid) -> Option<Self> {
        match self {
            Self::Terminal { id } => (*id != terminal_id).then_some(self.clone()),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => match (
                first.removing_terminal(terminal_id),
                second.removing_terminal(terminal_id),
            ) {
                (None, None) => None,
                (None, Some(remaining)) | (Some(remaining), None) => Some(remaining),
                (Some(first), Some(second)) => Some(Self::Split {
                    axis: *axis,
                    ratio: *ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
            },
        }
    }

    pub fn joining(mut layouts: Vec<Self>, axis: WorkspaceSplitAxis) -> Self {
        assert!(
            !layouts.is_empty(),
            "a pane layout needs at least one terminal"
        );
        let first = layouts.remove(0);
        layouts
            .into_iter()
            .fold(first, |first, second| Self::Split {
                axis,
                ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(first),
                second: Box::new(second),
            })
    }
}

fn directional_score(
    current: PaneRect,
    candidate: PaneRect,
    direction: PaneFocusDirection,
) -> Option<f32> {
    let dx = candidate.center_x() - current.center_x();
    let dy = candidate.center_y() - current.center_y();
    let (primary, orthogonal, overlaps) = match direction {
        PaneFocusDirection::Left if dx < 0.0 => (
            -dx,
            dy.abs(),
            ranges_overlap(current.y, current.height, candidate.y, candidate.height),
        ),
        PaneFocusDirection::Right if dx > 0.0 => (
            dx,
            dy.abs(),
            ranges_overlap(current.y, current.height, candidate.y, candidate.height),
        ),
        PaneFocusDirection::Up if dy < 0.0 => (
            -dy,
            dx.abs(),
            ranges_overlap(current.x, current.width, candidate.x, candidate.width),
        ),
        PaneFocusDirection::Down if dy > 0.0 => (
            dy,
            dx.abs(),
            ranges_overlap(current.x, current.width, candidate.x, candidate.width),
        ),
        _ => return None,
    };
    Some(primary + orthogonal * 0.25 + if overlaps { 0.0 } else { 2.0 })
}

fn ranges_overlap(start: f32, length: f32, other_start: f32, other_length: f32) -> bool {
    start < other_start + other_length && other_start < start + length
}
