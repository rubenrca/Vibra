use gpui::SharedString;

use crate::ports::git::{GitDiff, GitDiffRow, GitDiffRowKind, GitDiffSources};
use crate::ui::diff_rows::ContextFold;
use crate::ui::syntax::{Highlighter, SyntaxSpan, expand_tabs, highlight_diff_rows};

/// Prepared, immutable data consumed by the GPUI diff renderer.
///
/// Building a document performs the CPU-heavy syntax pass and layout scan, so
/// callers should construct it on a background executor and share it by `Arc`.
#[derive(Debug)]
pub struct DiffDocument {
    pub diff: GitDiff,
    pub highlights: Vec<Vec<SyntaxSpan>>,
    /// Expanded text is shared by measurement and paint, without per-frame copies.
    pub display_lines: Vec<SharedString>,
    /// Character columns of the widest code row; drives horizontal scrolling.
    pub widest_columns: usize,
    /// Largest old or new line number, which sizes the gutters.
    pub max_line_number: usize,
    /// Unchanged stretches between (and around) hunks, filled in from the
    /// whole new file. Empty when the file was unavailable: the diff then
    /// keeps its hunk headers instead.
    pub folds: Vec<ContextFold>,
    /// True when highlights came from the whole old/new files rather than the
    /// hunks alone, so constructs opened outside a hunk color correctly.
    #[cfg(test)]
    pub full_context: bool,
}

impl DiffDocument {
    #[cfg(test)]
    pub fn prepare(diff: GitDiff) -> Self {
        Self::prepare_with_sources(diff, None)
    }

    pub fn prepare_with_sources(mut diff: GitDiff, sources: Option<&GitDiffSources>) -> Self {
        let mut display_lines: Vec<SharedString> = diff
            .rows
            .iter()
            .map(|row| expand_tabs(&row.text).into())
            .collect();
        let texts: Vec<&str> = display_lines.iter().map(SharedString::as_ref).collect();
        let full = sources.and_then(|sources| full_context_highlights(&diff, &texts, sources));
        #[cfg(test)]
        let full_context = full.is_some();
        let mut folds = Vec::new();
        let highlights = match full {
            Some((highlights, new)) => {
                match unfold_context(&diff.rows, &display_lines, &highlights, &new) {
                    Some(unfolded) => {
                        diff.rows = unfolded.rows;
                        display_lines = unfolded.display_lines;
                        folds = unfolded.folds;
                        unfolded.highlights
                    }
                    None => highlights,
                }
            }
            None => highlight_diff_rows(&diff.path, &diff.rows, &texts),
        };
        let widest_columns = display_lines
            .iter()
            .zip(&diff.rows)
            .filter(|(_, row)| is_code_row(row))
            .map(|(line, _)| line.chars().count())
            .max()
            .unwrap_or(0);
        let max_line_number = diff
            .rows
            .iter()
            .flat_map(|row| [row.old_line, row.new_line])
            .flatten()
            .max()
            .unwrap_or(0);

        Self {
            diff,
            highlights,
            display_lines,
            widest_columns,
            max_line_number,
            folds,
            #[cfg(test)]
            full_context,
        }
    }
}

fn is_code_row(row: &GitDiffRow) -> bool {
    matches!(
        row.kind,
        GitDiffRowKind::Context | GitDiffRowKind::Addition | GitDiffRowKind::Deletion
    )
}

type SideLines = Vec<(String, Vec<SyntaxSpan>)>;

/// Highlight each side's whole file and pick every diff row's spans from the
/// line it cites. Any row that disagrees with its source (stale read, rename,
/// multi-section diff) abandons the attempt; per-row highlighting takes over.
/// The new side comes back too, for [`unfold_context`].
fn full_context_highlights(
    diff: &GitDiff,
    texts: &[&str],
    sources: &GitDiffSources,
) -> Option<(Vec<Vec<SyntaxSpan>>, SideLines)> {
    if diff.binary
        || diff.truncated
        || diff
            .rows
            .iter()
            .any(|row| row.kind == GitDiffRowKind::Section)
    {
        return None;
    }
    let needs_old = diff
        .rows
        .iter()
        .any(|row| row.kind == GitDiffRowKind::Deletion);
    let needs_new = diff
        .rows
        .iter()
        .any(|row| matches!(row.kind, GitDiffRowKind::Addition | GitDiffRowKind::Context));
    let old = side_lines(&diff.path, sources.old.as_deref(), needs_old)?;
    // A present new file is always read: it fills the folds between hunks.
    let new = side_lines(
        &diff.path,
        sources.new.as_deref(),
        needs_new || sources.new.is_some(),
    )?;

    let highlights = diff
        .rows
        .iter()
        .zip(texts)
        .map(|(row, text)| {
            let (side, line) = match row.kind {
                GitDiffRowKind::Deletion => (&old, row.old_line?),
                GitDiffRowKind::Addition | GitDiffRowKind::Context => (&new, row.new_line?),
                _ => return Some(Vec::new()),
            };
            let (source, spans) = side.get(line.checked_sub(1)?)?;
            (source == text).then(|| spans.clone())
        })
        .collect::<Option<_>>()?;
    Some((highlights, new))
}

struct Unfolded {
    rows: Vec<GitDiffRow>,
    display_lines: Vec<SharedString>,
    highlights: Vec<Vec<SyntaxSpan>>,
    folds: Vec<ContextFold>,
}

/// Replace every hunk header with the unchanged lines it skipped (read from
/// the whole new file), and add the lines after the last hunk, recording each
/// stretch as a fold. Old and new line numbers advance in step between hunks,
/// so one gap length serves both sides. Any inconsistency keeps the headers.
fn unfold_context(
    rows: &[GitDiffRow],
    display_lines: &[SharedString],
    highlights: &[Vec<SyntaxSpan>],
    new: &SideLines,
) -> Option<Unfolded> {
    if !rows.iter().any(|row| row.kind == GitDiffRowKind::Hunk) {
        return None;
    }
    let mut out = Unfolded {
        rows: Vec::with_capacity(rows.len()),
        display_lines: Vec::with_capacity(rows.len()),
        highlights: Vec::with_capacity(rows.len()),
        folds: Vec::new(),
    };
    // Next unseen line of each side (1-based).
    let mut old_next = 1;
    let mut new_next = 1;
    let push_gap = |out: &mut Unfolded, old_next: &mut usize, new_next: &mut usize, len| {
        if len == 0 {
            return Some(());
        }
        let start = out.rows.len();
        for offset in 0..len {
            let (text, spans) = new.get(*new_next + offset - 1)?;
            out.rows.push(GitDiffRow {
                old_line: Some(*old_next + offset),
                new_line: Some(*new_next + offset),
                kind: GitDiffRowKind::Context,
                text: text.clone(),
            });
            out.display_lines.push(text.clone().into());
            out.highlights.push(spans.clone());
        }
        out.folds.push(ContextFold { start, len });
        *old_next += len;
        *new_next += len;
        Some(())
    };

    for (index, row) in rows.iter().enumerate() {
        if row.kind == GitDiffRowKind::Hunk {
            let hunk = rows[index + 1..]
                .iter()
                .take_while(|row| row.kind != GitDiffRowKind::Hunk);
            let first_old = hunk.clone().find_map(|row| row.old_line);
            let first_new = hunk.clone().find_map(|row| row.new_line);
            let gap = match (first_old, first_new) {
                (Some(old), _) => old.checked_sub(old_next)?,
                (None, Some(new)) => new.checked_sub(new_next)?,
                (None, None) => 0,
            };
            push_gap(&mut out, &mut old_next, &mut new_next, gap)?;
            if first_old.is_some_and(|old| old != old_next)
                || first_new.is_some_and(|new| new != new_next)
            {
                return None;
            }
            continue;
        }
        if let Some(old) = row.old_line {
            old_next = old + 1;
        }
        if let Some(new) = row.new_line {
            new_next = new + 1;
        }
        out.rows.push(row.clone());
        out.display_lines.push(display_lines[index].clone());
        out.highlights.push(highlights[index].clone());
    }
    let trailing = (new.len() + 1).checked_sub(new_next)?;
    push_gap(&mut out, &mut old_next, &mut new_next, trailing)?;
    Some(out)
}

/// `(display text, spans)` per line of one side, or an empty side when the
/// diff never cites it. `None` only when a cited side is unavailable.
fn side_lines(
    path: &str,
    source: Option<&str>,
    needed: bool,
) -> Option<Vec<(String, Vec<SyntaxSpan>)>> {
    if !needed {
        return Some(Vec::new());
    }
    let mut highlighter = Highlighter::for_path(path);
    Some(
        source?
            .lines()
            .map(|line| {
                let text = expand_tabs(line);
                let spans = highlighter.highlight_line(&text);
                (text, spans)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::git::{GitDiffRow, GitDiffRowKind};
    use crate::ui::syntax::SyntaxKind;

    #[test]
    fn prepares_highlights_and_layout_metadata_once() {
        let document = DiffDocument::prepare(GitDiff {
            path: "src/main.rs".into(),
            rows: vec![
                GitDiffRow {
                    old_line: Some(1),
                    new_line: Some(1),
                    kind: GitDiffRowKind::Context,
                    text: "fn short() {}".into(),
                },
                GitDiffRow {
                    old_line: None,
                    new_line: Some(2),
                    kind: GitDiffRowKind::Addition,
                    text: "\tlet message = \"a much wider line\";".into(),
                },
            ],
            additions: 1,
            deletions: 0,
            binary: false,
            truncated: false,
        });

        assert_eq!(document.highlights.len(), document.diff.rows.len());
        assert!(!document.highlights[1].is_empty());
        assert_eq!(
            document.widest_columns,
            "    let message = \"a much wider line\";".len()
        );
        assert!(!document.full_context);
        assert_eq!(
            document.display_lines[1].as_ref(),
            "    let message = \"a much wider line\";"
        );
        assert!(
            document.highlights[1]
                .iter()
                .all(|span| { document.display_lines[1].get(span.range.clone()).is_some() })
        );
    }

    fn comment_tail_diff() -> GitDiff {
        // The hunk starts inside a block comment opened on line 1, which the
        // patch alone never shows.
        GitDiff {
            path: "src/lib.rs".into(),
            rows: vec![
                GitDiffRow {
                    old_line: None,
                    new_line: None,
                    kind: GitDiffRowKind::Hunk,
                    text: "@@ -2,2 +2,2 @@".into(),
                },
                GitDiffRow {
                    old_line: Some(2),
                    new_line: Some(2),
                    kind: GitDiffRowKind::Context,
                    text: "let inside = 1;".into(),
                },
                GitDiffRow {
                    old_line: Some(3),
                    new_line: None,
                    kind: GitDiffRowKind::Deletion,
                    text: "old */".into(),
                },
                GitDiffRow {
                    old_line: None,
                    new_line: Some(3),
                    kind: GitDiffRowKind::Addition,
                    text: "new */".into(),
                },
            ],
            additions: 1,
            deletions: 1,
            binary: false,
            truncated: false,
        }
    }

    #[test]
    fn whole_file_sources_carry_state_from_outside_the_hunk() {
        let sources = GitDiffSources {
            old: Some("/* opened\nlet inside = 1;\nold */\n".into()),
            new: Some("/* opened\nlet inside = 1;\nnew */\n".into()),
        };
        let document = DiffDocument::prepare_with_sources(comment_tail_diff(), Some(&sources));
        assert!(document.full_context);
        assert!(
            document.highlights[1]
                .iter()
                .all(|span| span.kind == SyntaxKind::Comment)
        );
        assert!(
            document.highlights[2]
                .iter()
                .any(|span| span.kind == SyntaxKind::Comment)
        );
    }

    #[test]
    fn mismatched_sources_fall_back_to_per_row_highlighting() {
        let sources = GitDiffSources {
            old: Some("/* opened\nlet inside = 1;\nstale */\n".into()),
            new: Some("/* opened\nlet inside = 1;\nnew */\n".into()),
        };
        let document = DiffDocument::prepare_with_sources(comment_tail_diff(), Some(&sources));
        assert!(!document.full_context);
        assert!(
            document.highlights[1]
                .iter()
                .any(|span| span.kind == SyntaxKind::Keyword)
        );
    }

    fn row(kind: GitDiffRowKind, old: Option<usize>, new: Option<usize>, text: &str) -> GitDiffRow {
        GitDiffRow {
            old_line: old,
            new_line: new,
            kind,
            text: text.into(),
        }
    }

    /// Ten-line file; line 4 changed and line 8 removed, one line of context.
    fn two_hunk_diff() -> GitDiff {
        GitDiff {
            path: "notes.txt".into(),
            rows: vec![
                row(GitDiffRowKind::Hunk, None, None, "@@ -3,3 +3,3 @@"),
                row(GitDiffRowKind::Context, Some(3), Some(3), "3"),
                row(GitDiffRowKind::Deletion, Some(4), None, "four"),
                row(GitDiffRowKind::Addition, None, Some(4), "4"),
                row(GitDiffRowKind::Context, Some(5), Some(5), "5"),
                row(GitDiffRowKind::Hunk, None, None, "@@ -7,3 +7,2 @@"),
                row(GitDiffRowKind::Context, Some(7), Some(7), "7"),
                row(GitDiffRowKind::Deletion, Some(8), None, "8"),
                row(GitDiffRowKind::Context, Some(9), Some(8), "9"),
            ],
            additions: 1,
            deletions: 2,
            binary: false,
            truncated: false,
        }
    }

    fn two_hunk_sources() -> GitDiffSources {
        GitDiffSources {
            old: Some("1\n2\n3\nfour\n5\n6\n7\n8\n9\n10\n".into()),
            new: Some("1\n2\n3\n4\n5\n6\n7\n9\n10\n".into()),
        }
    }

    #[test]
    fn hunk_headers_become_folds_of_the_unchanged_lines() {
        let document =
            DiffDocument::prepare_with_sources(two_hunk_diff(), Some(&two_hunk_sources()));
        let rows = &document.diff.rows;
        assert!(rows.iter().all(|row| row.kind != GitDiffRowKind::Hunk));
        assert_eq!(
            document.folds,
            vec![
                ContextFold { start: 0, len: 2 },
                ContextFold { start: 6, len: 1 },
                ContextFold { start: 10, len: 1 },
            ]
        );
        assert_eq!(rows.len(), document.display_lines.len());
        assert_eq!(rows.len(), document.highlights.len());
        // Leading gap: lines 1–2 on both sides.
        assert_eq!((rows[1].old_line, rows[1].new_line), (Some(2), Some(2)));
        // Between hunks: old 6 is new 6.
        assert_eq!((rows[6].old_line, rows[6].new_line), (Some(6), Some(6)));
        assert_eq!(document.display_lines[6].as_ref(), "6");
        // Trailing gap after the removal: old 10 is new 9.
        assert_eq!((rows[10].old_line, rows[10].new_line), (Some(10), Some(9)));
        assert_eq!(document.display_lines[10].as_ref(), "10");
    }

    #[test]
    fn without_the_new_file_hunk_headers_stay() {
        let sources = GitDiffSources {
            old: two_hunk_sources().old,
            new: None,
        };
        let document = DiffDocument::prepare_with_sources(two_hunk_diff(), Some(&sources));
        assert!(document.folds.is_empty());
        assert_eq!(document.diff.rows[0].kind, GitDiffRowKind::Hunk);
    }

    #[test]
    fn a_hunk_at_odds_with_the_new_file_keeps_the_headers() {
        let mut diff = two_hunk_diff();
        // Claims the second hunk starts a line later than the file allows.
        diff.rows[6].old_line = Some(8);
        let document = DiffDocument::prepare_with_sources(diff, Some(&two_hunk_sources()));
        assert!(document.folds.is_empty());
        assert_eq!(document.diff.rows[0].kind, GitDiffRowKind::Hunk);
    }
}
