//! Line diff for agent edits. Agents send whole old/new texts, so this keeps
//! the simple form: the common head and tail are dropped, and the rest of the
//! old text is shown as removed and the rest of the new text as added.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiffKind {
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// Changed lines of an edit. `None` old text means a new file.
pub(super) fn changed_lines(old: Option<&str>, new: &str) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old.map(|text| text.lines().collect()).unwrap_or_default();
    let new_lines: Vec<&str> = new.lines().collect();
    let head = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(old, new)| old == new)
        .count();
    let tail_limit = (old_lines.len() - head).min(new_lines.len() - head);
    let tail = old_lines
        .iter()
        .rev()
        .zip(new_lines.iter().rev())
        .take(tail_limit)
        .take_while(|(old, new)| old == new)
        .count();
    let removed = &old_lines[head..old_lines.len() - tail];
    let added = &new_lines[head..new_lines.len() - tail];
    removed
        .iter()
        .map(|text| DiffLine {
            kind: DiffKind::Removed,
            text: (*text).to_owned(),
        })
        .chain(added.iter().map(|text| DiffLine {
            kind: DiffKind::Added,
            text: (*text).to_owned(),
        }))
        .collect()
}

/// Count of added and removed lines, for the card summary.
pub(super) fn line_counts(lines: &[DiffLine]) -> (usize, usize) {
    lines
        .iter()
        .fold((0, 0), |(added, removed), line| match line.kind {
            DiffKind::Added => (added + 1, removed),
            DiffKind::Removed => (added, removed + 1),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[DiffLine]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                let sign = match line.kind {
                    DiffKind::Added => '+',
                    DiffKind::Removed => '-',
                };
                format!("{sign}{}", line.text)
            })
            .collect()
    }

    #[test]
    fn single_line_replacement_shows_one_removed_and_one_added_line() {
        let lines = changed_lines(Some("a\nb\nc"), "a\nB\nc");
        assert_eq!(text(&lines), ["-b", "+B"]);
    }

    #[test]
    fn appended_and_removed_lines_are_reported_on_their_own() {
        assert_eq!(text(&changed_lines(Some("a"), "a\nb")), ["+b"]);
        assert_eq!(text(&changed_lines(Some("a\nb"), "a")), ["-b"]);
    }

    #[test]
    fn a_new_file_is_all_added_and_identical_text_has_no_lines() {
        assert_eq!(text(&changed_lines(None, "x\ny")), ["+x", "+y"]);
        assert!(changed_lines(Some("same\ntext"), "same\ntext").is_empty());
    }

    #[test]
    fn counts_split_added_and_removed_lines() {
        let lines = changed_lines(Some("a\nb\nc"), "x\ny\nc");
        assert_eq!(line_counts(&lines), (2, 2));
    }
}
