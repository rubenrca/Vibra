//! Word-level emphasis inside changed lines.
//!
//! Each run of deletions followed by additions is paired row by row (the same
//! pairing the split layout uses). For every pair, a token-level longest
//! common subsequence finds what actually changed, so the renderer can paint
//! those tokens a stronger tint than the rest of the line, the way GitHub and
//! most review tools do. Pairs that share too little are left alone: when a
//! line was rewritten, emphasizing nearly all of it only adds noise.

use std::ops::Range;

use crate::ports::git::{GitDiffRow, GitDiffRowKind};

/// Longer lines are rare and costly to compare; they keep the line tint only.
const MAX_LINE_BYTES: usize = 480;
/// Upper bound on the token comparison table for one pair.
const MAX_TABLE_CELLS: usize = 60_000;
/// Minimum share of unchanged, non-blank text for a pair to get emphasis.
const MIN_SIMILARITY: f32 = 0.4;

/// Changed byte ranges of every row, indexed like `rows`; empty when the row
/// is unchanged, unpaired, or too different from its partner to emphasize.
pub fn emphasis(rows: &[GitDiffRow], lines: &[&str]) -> Vec<Vec<Range<usize>>> {
    let mut out = vec![Vec::new(); rows.len()];
    let kind = |index: usize| rows.get(index).map(|row| row.kind);
    let mut index = 0;
    while index < rows.len() {
        if kind(index) != Some(GitDiffRowKind::Deletion) {
            index += 1;
            continue;
        }
        let deletions = index;
        while kind(index) == Some(GitDiffRowKind::Deletion) {
            index += 1;
        }
        let additions = index;
        while kind(index) == Some(GitDiffRowKind::Addition) {
            index += 1;
        }
        let pairs = (additions - deletions).min(index - additions);
        for offset in 0..pairs {
            let (old, new) = (deletions + offset, additions + offset);
            let (Some(old_text), Some(new_text)) = (lines.get(old), lines.get(new)) else {
                continue;
            };
            if let Some((old_ranges, new_ranges)) = changed_ranges(old_text, new_text) {
                out[old] = old_ranges;
                out[new] = new_ranges;
            }
        }
    }
    out
}

type RangePair = (Vec<Range<usize>>, Vec<Range<usize>>);

/// Byte ranges that differ between two versions of a line, or `None` when
/// the lines are identical, too long, or too different to emphasize.
pub fn changed_ranges(old: &str, new: &str) -> Option<RangePair> {
    if old == new || old.len() > MAX_LINE_BYTES || new.len() > MAX_LINE_BYTES {
        return None;
    }
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    fn token<'a>(text: &'a str, range: &Range<usize>) -> &'a str {
        &text[range.clone()]
    }

    // Shared head and tail need no table.
    let mut head = 0;
    while head < old_tokens.len()
        && head < new_tokens.len()
        && token(old, &old_tokens[head]) == token(new, &new_tokens[head])
    {
        head += 1;
    }
    let mut tail = 0;
    while tail < old_tokens.len() - head
        && tail < new_tokens.len() - head
        && token(old, &old_tokens[old_tokens.len() - 1 - tail])
            == token(new, &new_tokens[new_tokens.len() - 1 - tail])
    {
        tail += 1;
    }
    let old_middle = &old_tokens[head..old_tokens.len() - tail];
    let new_middle = &new_tokens[head..new_tokens.len() - tail];
    if (old_middle.len() + 1) * (new_middle.len() + 1) > MAX_TABLE_CELLS {
        return None;
    }

    let (old_common, new_common) = common_tokens(old, old_middle, new, new_middle);
    let old_changed: Vec<Range<usize>> = old_middle
        .iter()
        .zip(&old_common)
        .filter(|(_, common)| !**common)
        .map(|(range, _)| range.clone())
        .collect();
    let new_changed: Vec<Range<usize>> = new_middle
        .iter()
        .zip(&new_common)
        .filter(|(_, common)| !**common)
        .map(|(range, _)| range.clone())
        .collect();

    // Characters that are not blank, overall and in the changed ranges.
    let solid = |text: &str| text.chars().filter(|c| !c.is_whitespace()).count();
    let changed = |text: &str, ranges: &[Range<usize>]| -> usize {
        ranges.iter().map(|range| solid(&text[range.clone()])).sum()
    };
    let old_solid = solid(old);
    let new_solid = solid(new);
    let unchanged =
        (old_solid - changed(old, &old_changed)) + (new_solid - changed(new, &new_changed));
    let total = old_solid + new_solid;
    if total > 0 && (unchanged as f32) < MIN_SIMILARITY * total as f32 {
        return None;
    }

    let old_ranges = merge(old, old_changed);
    let new_ranges = merge(new, new_changed);
    (!old_ranges.is_empty() || !new_ranges.is_empty()).then_some((old_ranges, new_ranges))
}

/// Words (letters, digits, `_`), blank runs, and single other characters.
fn tokenize(text: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Blank,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Blank
        } else {
            Class::Other
        }
    };
    let mut tokens: Vec<Range<usize>> = Vec::new();
    let mut current: Option<(usize, Class)> = None;
    for (at, c) in text.char_indices() {
        let next = class(c);
        match &current {
            Some((_, kind)) if *kind == next && next != Class::Other => {}
            Some((start, _)) => {
                tokens.push(*start..at);
                current = Some((at, next));
            }
            None => current = Some((at, next)),
        }
    }
    if let Some((start, _)) = current {
        tokens.push(start..text.len());
    }
    tokens
}

/// Longest common subsequence of two token lists: which tokens of each side
/// are part of it.
fn common_tokens(
    old: &str,
    old_tokens: &[Range<usize>],
    new: &str,
    new_tokens: &[Range<usize>],
) -> (Vec<bool>, Vec<bool>) {
    let (rows, columns) = (old_tokens.len(), new_tokens.len());
    let mut old_common = vec![false; rows];
    let mut new_common = vec![false; columns];
    if rows == 0 || columns == 0 {
        return (old_common, new_common);
    }
    let width = columns + 1;
    // lengths[i][j]: LCS of old_tokens[i..] and new_tokens[j..].
    let mut lengths = vec![0u16; (rows + 1) * width];
    for i in (0..rows).rev() {
        for j in (0..columns).rev() {
            lengths[i * width + j] = if old[old_tokens[i].clone()] == new[new_tokens[j].clone()] {
                lengths[(i + 1) * width + j + 1] + 1
            } else {
                lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < rows && j < columns {
        if old[old_tokens[i].clone()] == new[new_tokens[j].clone()] {
            old_common[i] = true;
            new_common[j] = true;
            i += 1;
            j += 1;
        } else if lengths[(i + 1) * width + j] >= lengths[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (old_common, new_common)
}

/// Join touching changed tokens, and those separated only by blanks, so a
/// rewritten phrase reads as one highlight rather than a row of fragments.
fn merge(text: &str, ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && text[last.end..range.start].chars().all(char::is_whitespace)
        {
            last.end = range.end;
            continue;
        }
        merged.push(range);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slices<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|range| &text[range.clone()]).collect()
    }

    #[test]
    fn only_the_changed_tokens_are_emphasized() {
        let old = "let total = price * quantity;";
        let new = "let total = price * count + tax;";
        let (old_ranges, new_ranges) = changed_ranges(old, new).unwrap();
        assert_eq!(slices(old, &old_ranges), ["quantity"]);
        assert_eq!(slices(new, &new_ranges), ["count + tax"]);
    }

    #[test]
    fn rewritten_lines_get_no_emphasis() {
        assert!(changed_ranges("fn alpha() -> u8 { 1 }", "struct Beta;").is_none());
        assert!(changed_ranges("same", "same").is_none());
    }

    #[test]
    fn unicode_text_keeps_character_boundaries() {
        let old = "println!(\"café olé\");";
        let new = "println!(\"café ñandú\");";
        let (old_ranges, new_ranges) = changed_ranges(old, new).unwrap();
        assert_eq!(slices(old, &old_ranges), ["olé"]);
        assert_eq!(slices(new, &new_ranges), ["ñandú"]);
    }

    #[test]
    fn indentation_changes_are_visible() {
        let (old_ranges, new_ranges) = changed_ranges("  call();", "    call();").unwrap();
        assert_eq!(slices("  call();", &old_ranges), ["  "]);
        assert_eq!(slices("    call();", &new_ranges), ["    "]);
    }

    fn row(kind: GitDiffRowKind) -> GitDiffRow {
        GitDiffRow {
            old_line: None,
            new_line: None,
            kind,
            text: String::new(),
        }
    }

    #[test]
    fn deletion_runs_pair_with_the_additions_that_follow() {
        let rows = [
            row(GitDiffRowKind::Context),
            row(GitDiffRowKind::Deletion),
            row(GitDiffRowKind::Deletion),
            row(GitDiffRowKind::Addition),
            row(GitDiffRowKind::Context),
            row(GitDiffRowKind::Addition),
        ];
        let lines = [
            "keep();",
            "let a = 1;",
            "let b = 2;",
            "let a = 10;",
            "keep();",
            "let c = 3;",
        ];
        let out = emphasis(&rows, &lines);
        assert_eq!(slices(lines[1], &out[1]), ["1"]);
        assert_eq!(slices(lines[3], &out[3]), ["10"]);
        // Unpaired rows keep only their line tint.
        assert!(out[2].is_empty());
        assert!(out[5].is_empty());
        assert!(out[0].is_empty() && out[4].is_empty());
    }
}
