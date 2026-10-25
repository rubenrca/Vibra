//! Layout for the commit graph in the Git history view.

use crate::ports::git::GitCommit;

/// One row of a first-parent-aware lane graph for the history list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitGraphRow {
    pub lane: usize,
    pub color: usize,
    pub lane_count: usize,
    /// A child above continues into this commit's lane; a branch tip has none.
    pub incoming: bool,
    pub through: Vec<GitGraphRail>,
    /// Other lanes that also led to this commit and end at its node: branches
    /// that started from it.
    pub merges_in: Vec<GitGraphRail>,
    /// Secondary parents that branch away from the current commit.
    pub edges: Vec<GitGraphRail>,
    pub continues: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitGraphRail {
    pub lane: usize,
    pub color: usize,
}

struct Lane {
    commit: String,
    color: usize,
}

fn occupy_lane(lanes: &mut Vec<Option<Lane>>, commit: &str, color: usize) -> usize {
    let index = lanes.iter().position(Option::is_none).unwrap_or_else(|| {
        lanes.push(None);
        lanes.len() - 1
    });
    lanes[index] = Some(Lane {
        commit: commit.into(),
        color,
    });
    index
}

/// Assigns left-to-right lanes so a commit list can draw a compact ancestry graph.
/// A lane runs from a commit down to its first parent, so a branch stays in
/// its lane until the commit it started from, where the lanes converge.
pub fn assign_commit_lanes(commits: &[GitCommit]) -> Vec<GitGraphRow> {
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let mut next_color = 1;
    let mut remaining: std::collections::HashSet<&str> =
        commits.iter().map(|commit| commit.sha.as_str()).collect();

    for commit in commits {
        remaining.remove(commit.sha.as_str());
        let waiting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter_map(|(index, occupant)| {
                (occupant.as_ref()?.commit == commit.sha).then_some(index)
            })
            .collect();
        let incoming = !waiting.is_empty();
        let lane = match waiting.first() {
            Some(index) => *index,
            None => {
                let color = if lanes.iter().all(Option::is_none) {
                    0
                } else {
                    let color = next_color;
                    next_color += 1;
                    color
                };
                occupy_lane(&mut lanes, &commit.sha, color)
            }
        };
        let color = lanes[lane].as_ref().map_or(0, |lane| lane.color);
        // The other lanes that waited for this commit end at its node.
        let merges_in: Vec<GitGraphRail> = waiting
            .iter()
            .skip(1)
            .map(|index| GitGraphRail {
                lane: *index,
                color: lanes[*index].as_ref().map_or(0, |lane| lane.color),
            })
            .collect();
        for rail in &merges_in {
            lanes[rail.lane] = None;
        }

        let through: Vec<GitGraphRail> = lanes
            .iter()
            .enumerate()
            .filter_map(|(index, occupant)| {
                let occupant = occupant.as_ref()?;
                (index != lane).then_some(GitGraphRail {
                    lane: index,
                    color: occupant.color,
                })
            })
            .collect();

        let mut edges = Vec::new();
        let mut continues = false;
        match commit.parents.first() {
            Some(first) if remaining.contains(first.as_str()) => {
                lanes[lane] = Some(Lane {
                    commit: first.clone(),
                    color,
                });
                continues = true;
            }
            _ => lanes[lane] = None,
        }
        for parent in commit.parents.iter().skip(1) {
            if !remaining.contains(parent.as_str()) {
                continue;
            }
            if let Some(existing) = lanes.iter().position(|occupant| {
                occupant.as_ref().map(|lane| lane.commit.as_str()) == Some(parent.as_str())
            }) {
                if existing != lane {
                    edges.push(GitGraphRail {
                        lane: existing,
                        color: lanes[existing].as_ref().map_or(0, |lane| lane.color),
                    });
                }
                continue;
            }
            let parent_color = next_color;
            next_color += 1;
            edges.push(GitGraphRail {
                lane: occupy_lane(&mut lanes, parent, parent_color),
                color: parent_color,
            });
        }

        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let widest = merges_in
            .iter()
            .chain(&through)
            .chain(&edges)
            .map(|rail| rail.lane)
            .chain([lane])
            .max()
            .unwrap_or(0);

        rows.push(GitGraphRow {
            lane,
            color,
            lane_count: lanes.len().max(widest + 1),
            incoming,
            through,
            merges_in,
            edges,
            continues,
        });
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(sha: &str, parents: &[&str]) -> GitCommit {
        GitCommit {
            sha: sha.to_owned(),
            short_sha: sha.to_owned(),
            subject: sha.to_owned(),
            author: "t".into(),
            date: "2026-01-01".into(),
            parents: parents.iter().map(|parent| (*parent).to_owned()).collect(),
            refs: Vec::new(),
        }
    }

    #[test]
    fn linear_history_stays_on_one_lane() {
        let rows =
            assign_commit_lanes(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert!(rows.iter().all(|row| row.lane == 0 && row.lane_count == 1));
        assert!(rows.iter().all(|row| row.edges.is_empty()));
        assert!(rows[0].continues);
        assert!(rows[1].continues);
        assert!(!rows[2].continues);
        assert!(!rows[0].incoming, "the newest commit has nothing above it");
        assert!(rows[1].incoming && rows[2].incoming);
    }

    #[test]
    fn merge_opens_a_side_lane_that_rejoins_where_it_started() {
        let rows = assign_commit_lanes(&[
            commit("m", &["a", "b"]),
            commit("c", &["b"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].edges[0].lane, 1);
        assert_eq!(rows[0].edges[0].color, 1);
        // An unmerged tip opens its own lane, with nothing above it.
        assert_eq!(rows[1].lane, 2);
        assert!(!rows[1].incoming);
        // The merged branch keeps its lane down to the commit it started from…
        assert_eq!(rows[2].lane, 1);
        assert!(rows[2].continues);
        assert_eq!(rows[2].merges_in.len(), 1, "the tip ends at b");
        assert_eq!(rows[2].merges_in[0].lane, 2);
        // …and converges into the main lane there.
        assert_eq!(rows[3].lane, 0);
        assert_eq!(rows[3].merges_in[0].lane, 1);
        assert!(rows[3].through.is_empty());
    }

    #[test]
    fn branches_from_an_old_commit_keep_their_lanes_until_it() {
        let rows = assign_commit_lanes(&[
            commit("main", &["base"]),
            commit("bump-a", &["base"]),
            commit("bump-b", &["base"]),
            commit("base", &[]),
        ]);
        assert_eq!(
            rows.iter().map(|row| row.lane).collect::<Vec<_>>(),
            [0, 1, 2, 0]
        );
        assert!(rows[1..3].iter().all(|row| !row.incoming && row.continues));
        assert_eq!(rows[2].through.len(), 2);
        assert_eq!(
            rows[3]
                .merges_in
                .iter()
                .map(|rail| rail.lane)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(rows[3].lane_count, 3);
    }

    #[test]
    fn off_page_merge_parent_does_not_keep_a_spare_lane() {
        let rows = assign_commit_lanes(&[commit("m", &["a", "missing"]), commit("a", &[])]);
        assert_eq!(rows[0].lane, 0);
        assert!(rows[0].edges.is_empty());
        assert_eq!(rows[1].lane, 0);
        assert_eq!(rows[1].lane_count, 1);
    }
}
