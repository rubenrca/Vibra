//! Virtualized review rows, folding, scrolling, and comment cards.

use super::{
    BASE_DIFF_FONT_SIZE, BASE_DIFF_ROW_HEIGHT, CARD_BORDERS, CARD_FOOTER, CARD_GAP, CARD_INSET,
    CARD_RADIUS, CHANGE_BAR_WIDTH, CODE_PADDING_LEFT, CODE_PADDING_RIGHT, COMMENT_ACTION_WIDTH,
    COMMENT_BUTTON_SIZE, DiffView, FILE_HEADER_HEIGHT, FOLD_DURATION, FoldActions, GitPanelMode,
    RowKey, RowMetrics, SECTION_HEIGHT, SPLIT_DIVIDER_WIDTH, STICKY_PUSH_SCAN, git_status_badge,
};
use crate::domain::work_items::FileViewed;
use crate::ports::files::FileEntryKind;
use crate::ports::git::{GitDiffRow, GitDiffRowKind, GitFileChange, GitFileStatus};
use crate::ui::diff_document::DiffDocument;
use crate::ui::diff_rows::{
    BodyRow, CommentAnchor, CommentSide, FlattenFile, FoldDirection, RemoteThread, ReviewComment,
    ReviewRow, body_row_anchors, body_rows, flatten_with_threads,
};
use crate::ui::syntax::SyntaxSpan;
use crate::ui::theme::{MONO_FONT, colors, mix, surface_tint};
use crate::ui::workspace_view::{file_tree_icon, file_tree_icon_color};
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, Div, FontWeight, HighlightStyle, Hsla,
    ListOffset, Rgba, SharedString, Stateful, StyledText, TextStyle, WhiteSpace, Window, div,
    ease_out_quint, prelude::*, px,
};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

impl DiffView {
    pub(super) fn ordered_file_refs(&self) -> impl Iterator<Item = &GitFileChange> {
        let changes = self
            .active_snapshot()
            .map(|snapshot| snapshot.changes.as_slice())
            .unwrap_or(&[]);
        changes
            .iter()
            .filter(|change| !change.staged)
            .chain(changes.iter().filter(|change| change.staged))
    }

    pub(super) fn document(&self, path: &str) -> Option<&Arc<DiffDocument>> {
        self.documents.get(path).map(|cached| &cached.document)
    }

    /// Rebuild the flat rows when anything they depend on changed, keeping the
    /// row at the top of the viewport in place.
    pub(super) fn sync_rows(&mut self) {
        let mut hasher = DefaultHasher::new();
        (
            self.mode,
            self.selected_commit.is_some(),
            self.layout,
            self.wrap,
        )
            .hash(&mut hasher);
        self.font_size.to_bits().hash(&mut hasher);
        for change in self.ordered_file_refs() {
            (
                &change.path,
                &change.old_path,
                change.staged,
                change.unstaged,
                change.additions,
                change.deletions,
                git_status_badge(change.status),
            )
                .hash(&mut hasher);
            self.expanded.contains(&change.path).hash(&mut hasher);
            self.folds
                .get(&change.path)
                .map(|fold| fold.generation)
                .hash(&mut hasher);
            self.document(&change.path)
                .map(|document| Arc::as_ptr(document) as usize)
                .hash(&mut hasher);
            if let Some(reveals) = self.fold_reveals.get(&change.path) {
                let mut reveals: Vec<_> = reveals.iter().collect();
                reveals.sort_unstable();
                reveals.hash(&mut hasher);
            }
        }
        for comment in &self.comments {
            (comment.id, &comment.anchor).hash(&mut hasher);
        }
        for thread in &self.remote_threads {
            (&thread.id, &thread.anchor, thread.comments.len()).hash(&mut hasher);
        }
        self.draft
            .as_ref()
            .map(|draft| &draft.anchor)
            .hash(&mut hasher);
        let signature = hasher.finish();
        if self.rows_signature == Some(signature) && self.pending_reveal.is_none() {
            return;
        }
        let files: Vec<_> = self.ordered_file_refs().cloned().collect();
        self.rows_signature = Some(signature);

        let worktree = self.mode == GitPanelMode::Worktree;
        let first_staged = files.iter().position(|change| change.staged);
        let entries: Vec<FlattenFile<'_>> = files
            .iter()
            .enumerate()
            .map(|(index, change)| FlattenFile {
                path: &change.path,
                starts_staged_section: worktree && first_staged == Some(index),
                expanded: self.expanded.contains(&change.path),
                folding: self.folds.contains_key(&change.path),
                rows: self
                    .document(&change.path)
                    .map(|document| document.diff.rows.as_slice()),
                folds: self
                    .document(&change.path)
                    .map_or(&[][..], |document| document.folds.as_slice()),
                reveals: self.fold_reveals.get(&change.path),
            })
            .collect();
        let rows = flatten_with_threads(
            &entries,
            self.layout,
            &self.comments,
            &self.remote_threads,
            self.draft.as_ref().map(|draft| &draft.anchor),
        );
        drop(entries);

        let top = self.list_state.logical_scroll_top();
        let anchor = self
            .rows
            .get(top.item_ix)
            .map(|row| self.row_key(*row, &self.row_files));
        let reveal = self.pending_reveal.take();
        let new_files = Arc::new(files);
        let key_at = |row: ReviewRow| self.row_key(row, &new_files);
        let header_of = |path: &str| {
            rows.iter().position(|row| {
                matches!(row, ReviewRow::FileHeader { file } if new_files[*file].path == path)
            })
        };
        let target = if let Some(path) = reveal {
            header_of(&path).map(|item_ix| ListOffset {
                item_ix,
                offset_in_item: px(0.0),
            })
        } else if let Some(key) = anchor {
            rows.iter()
                .position(|row| key_at(*row) == key)
                .map(|item_ix| ListOffset {
                    item_ix,
                    offset_in_item: top.offset_in_item,
                })
                .or_else(|| {
                    Self::key_path(&key)
                        .and_then(header_of)
                        .map(|item_ix| ListOffset {
                            item_ix,
                            offset_in_item: px(0.0),
                        })
                })
        } else {
            None
        };

        self.list_state.reset(rows.len());
        if let Some(target) = target {
            self.list_state.scroll_to(target);
        }
        self.rows = Arc::new(rows);
        self.row_files = new_files;
    }

    fn row_key(&self, row: ReviewRow, files: &[GitFileChange]) -> RowKey {
        let path = |file: usize| {
            files
                .get(file)
                .map(|change| change.path.clone())
                .unwrap_or_default()
        };
        match row {
            ReviewRow::StagedSection => RowKey::StagedSection,
            ReviewRow::FileHeader { file } => RowKey::Header(path(file)),
            ReviewRow::FileLoading { file } => RowKey::Loading(path(file)),
            ReviewRow::Folding { file } => RowKey::Folding(path(file)),
            ReviewRow::Draft { .. } => RowKey::Draft,
            ReviewRow::Comment { comment, .. } => {
                RowKey::Comment(self.comments.get(comment).map_or(0, |comment| comment.id))
            }
            ReviewRow::Thread { thread, .. } => RowKey::Thread(
                self.remote_threads
                    .get(thread)
                    .map(|thread| thread.id.clone())
                    .unwrap_or_default(),
            ),
            ReviewRow::Body { file, row } => match row {
                BodyRow::Line(index) => RowKey::Body(path(file), index),
                BodyRow::Split { left, right } => {
                    RowKey::Body(path(file), right.or(left).unwrap_or(0))
                }
                BodyRow::Fold { fold, .. } => RowKey::ContextFold(path(file), fold),
            },
        }
    }

    fn key_path(key: &RowKey) -> Option<&str> {
        match key {
            RowKey::Header(path)
            | RowKey::Loading(path)
            | RowKey::Body(path, _)
            | RowKey::ContextFold(path, _)
            | RowKey::Folding(path) => Some(path),
            _ => None,
        }
    }

    pub(super) fn metrics(&self) -> RowMetrics {
        let line_height = (self.font_size * BASE_DIFF_ROW_HEIGHT / BASE_DIFF_FONT_SIZE).round();
        RowMetrics {
            font_size: self.font_size,
            line_height,
            hunk_height: line_height + 2.0,
            fold_height: line_height + 12.0,
            char_width: self.char_width,
            wrap: self.wrap,
            h_offset: if self.wrap { 0.0 } else { self.h_offset },
        }
    }

    /// Measure the monospace advance and clamp the shared horizontal scroll to
    /// the widest expanded line.
    pub(super) fn sync_horizontal_metrics(&mut self, window: &mut Window) {
        let font_id = window.text_system().resolve_font(&gpui::font(MONO_FONT));
        if let Ok(width) = window.text_system().ch_advance(font_id, px(self.font_size)) {
            self.char_width = f32::from(width);
        }
        if self.wrap {
            self.h_offset = 0.0;
            self.h_max = 0.0;
            return;
        }
        let metrics = self.metrics();
        let (widest, max_line) = self
            .expanded
            .iter()
            .filter_map(|path| self.document(path))
            .fold((0, 0), |(widest, max_line), document| {
                (
                    widest.max(document.widest_columns),
                    max_line.max(document.max_line_number),
                )
            });
        let viewport = f32::from(self.list_state.viewport_bounds().size.width);
        let card = viewport - CARD_INSET * 2.0 - CARD_BORDERS;
        let gutter = metrics.gutter_width(max_line) + metrics.sign_width();
        let code_width = if self.layout.is_split() {
            (card - SPLIT_DIVIDER_WIDTH) / 2.0 - gutter
        } else {
            card - gutter
        };
        let content = widest as f32 * self.char_width + CODE_PADDING_LEFT + CODE_PADDING_RIGHT;
        self.h_max = if viewport > 0.0 {
            (content - code_width).max(0.0)
        } else {
            0.0
        };
        self.h_offset = self.h_offset.clamp(0.0, self.h_max);
    }

    pub(super) fn scroll_code_horizontally(&mut self, delta: f32, cx: &mut Context<Self>) -> bool {
        if self.wrap {
            return false;
        }
        let next = (self.h_offset - delta).clamp(0.0, self.h_max);
        if next == self.h_offset {
            return false;
        }
        self.h_offset = next;
        cx.notify();
        true
    }

    /// The file header pinned over the list and how far the next header has
    /// pushed it up (≤ 0).
    pub(super) fn sticky_header(&self) -> Option<(usize, f32)> {
        let top = self.list_state.logical_scroll_top();
        let row = *self.rows.get(top.item_ix)?;
        let file = row.file()?;
        // A collapsed card is only its header: nothing to pin it over.
        if !self.expanded.contains(&self.row_files.get(file)?.path) {
            return None;
        }
        let lead = if top.item_ix == 0 { CARD_GAP } else { 0.0 };
        if matches!(row, ReviewRow::FileHeader { .. }) && top.offset_in_item <= px(lead) {
            return None;
        }
        let viewport_top = self.list_state.viewport_bounds().top();
        let mut offset = 0.0;
        let end = (top.item_ix + STICKY_PUSH_SCAN).min(self.rows.len());
        for index in top.item_ix + 1..end {
            if !matches!(
                self.rows[index],
                ReviewRow::FileHeader { .. } | ReviewRow::StagedSection
            ) {
                continue;
            }
            if let Some(bounds) = self.list_state.bounds_for_item(index) {
                let distance = f32::from(bounds.top() - viewport_top);
                if distance < FILE_HEADER_HEIGHT {
                    offset = distance - FILE_HEADER_HEIGHT;
                }
            }
            break;
        }
        Some((file, offset.min(0.0)))
    }

    pub(super) fn render_row(
        &mut self,
        ix: usize,
        viewport_height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.rows.get(ix).copied() else {
            return div().into_any_element();
        };
        let files = self.row_files.clone();
        let metrics = self.metrics();
        // A file's rows form one card: the header opens it and the row before
        // the next header (or the end of the list) closes it.
        let closes_card = self.rows.get(ix + 1).is_none_or(|next| {
            matches!(
                next,
                ReviewRow::FileHeader { .. } | ReviewRow::StagedSection
            )
        });
        let body = match row {
            ReviewRow::StagedSection => {
                let count = files.iter().filter(|change| change.staged).count();
                return Self::file_section_header("Staged", count).into_any_element();
            }
            ReviewRow::FileHeader { file } => {
                return match files.get(file) {
                    Some(change) => {
                        let placement = HeaderPlacement {
                            open: !closes_card,
                            first: ix == 0,
                        };
                        self.file_header(change, Some(placement), cx)
                            .into_any_element()
                    }
                    None => div().into_any_element(),
                };
            }
            ReviewRow::FileLoading { .. } => div()
                .w_full()
                .h(px(metrics.fold_height))
                .flex()
                .items_center()
                .pl(px(metrics.gutter_width(99) + metrics.sign_width()))
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .child("Loading diff…")
                .into_any_element(),
            ReviewRow::Body { file, row } => {
                let Some(change) = files.get(file) else {
                    return div().into_any_element();
                };
                let Some(document) = self.document(&change.path).cloned() else {
                    return div().into_any_element();
                };
                self.body_row(ix, &change.path, &document, row, metrics, cx)
            }
            ReviewRow::Comment { comment, .. } => {
                let Some(comment) = self.comments.get(comment).cloned() else {
                    return div().into_any_element();
                };
                self.comment_card(&comment, metrics, cx).into_any_element()
            }
            ReviewRow::Thread { thread, .. } => {
                let Some(thread) = self.remote_threads.get(thread).cloned() else {
                    return div().into_any_element();
                };
                self.thread_card(&thread, metrics).into_any_element()
            }
            ReviewRow::Draft { .. } => self.draft_card(metrics, cx).into_any_element(),
            ReviewRow::Folding { file } => match files.get(file) {
                Some(change) => self.folding_body(&change.path, metrics, viewport_height),
                None => div().into_any_element(),
            },
        };
        Self::card_body(body, closes_card)
    }

    /// Side borders of a file card around one body row; the last row also
    /// draws the rounded bottom and the gap before the next card.
    fn card_body(row: AnyElement, last: bool) -> AnyElement {
        div()
            .w_full()
            .px(px(CARD_INSET))
            .when(last, |frame| frame.pb(px(CARD_GAP)))
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .border_l_1()
                    .border_r_1()
                    .border_color(colors().border_subtle)
                    .when(last, |card| {
                        card.border_b_1()
                            .rounded_b(px(CARD_RADIUS))
                            .pb(px(CARD_FOOTER))
                    })
                    .child(row),
            )
            .into_any_element()
    }

    fn file_section_header(label: &'static str, count: usize) -> Div {
        div()
            .h(px(SECTION_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_end()
            .gap(px(6.0))
            .px(px(CARD_INSET + 2.0))
            .pb(px(8.0))
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors().muted)
                    .child(label.to_uppercase()),
            )
            .child(
                div()
                    .min_w(px(16.0))
                    .h(px(16.0))
                    .px(px(5.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(surface_tint(colors().elevated, colors().background))
                    .text_size(px(9.5))
                    .text_color(colors().subtle)
                    .child(count.to_string()),
            )
    }

    /// `placement` is `None` for the sticky copy pinned over the list: folding
    /// from it scrolls the file back into view, since its own header sits
    /// above the viewport.
    pub(super) fn file_header(
        &self,
        change: &GitFileChange,
        placement: Option<HeaderPlacement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let pinned = placement.is_none();
        let open = placement.is_none_or(|placement| placement.open);
        let first = placement.is_some_and(|placement| placement.first);
        let id: SharedString = if pinned {
            "review-sticky-header".into()
        } else {
            format!("review-file-{}", change.path).into()
        };
        let path = change.path.clone();
        let expanded = self.expanded.contains(&path);
        let document = self.document(&path);

        let additions = change
            .additions
            .or_else(|| document.map(|d| d.diff.additions))
            .unwrap_or(0);
        let deletions = change
            .deletions
            .or_else(|| document.map(|d| d.diff.deletions))
            .unwrap_or(0);
        let comments = self
            .comments
            .iter()
            .filter(|comment| comment.anchor.path == path)
            .count();
        let path_for_click = path.clone();
        let (name, parent) = Self::path_parts(&change.path);
        let renamed_from = change
            .old_path
            .as_ref()
            .filter(|old| **old != change.path)
            .cloned();

        let header = div()
            .id(id.clone())
            .group(id.clone())
            .h(px(FILE_HEADER_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(10.0))
            .pr(px(8.0))
            .border_1()
            .border_color(colors().border_subtle)
            .map(|header| {
                if open {
                    header.rounded_t(px(CARD_RADIUS))
                } else {
                    header.rounded(px(CARD_RADIUS))
                }
            })
            .bg(surface_tint(
                mix(colors().background, colors().foreground, 0.035),
                colors().background,
            ))
            .cursor_pointer()
            .hover(|row| {
                row.bg(surface_tint(
                    mix(colors().background, colors().foreground, 0.065),
                    colors().background,
                ))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if pinned {
                    this.pending_reveal = Some(path_for_click.clone());
                }
                this.toggle_path(path_for_click.clone(), cx);
            }))
            .child(
                gpui::svg()
                    .path(if expanded {
                        "chrome-icons/chevron-down.svg"
                    } else {
                        "chrome-icons/chevron-right.svg"
                    })
                    .size(px(14.0))
                    .flex_none()
                    .text_color(colors().subtle)
                    .group_hover(id, |icon| icon.text_color(colors().foreground)),
            )
            .child(
                div()
                    .w(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(file_tree_icon(
                        FileEntryKind::File,
                        false,
                        &name,
                        file_tree_icon_color(FileEntryKind::File, &name),
                    )),
            )
            .child(
                // The folder recedes and gives way first; the name stays whole.
                div()
                    .min_w(px(0.0))
                    .flex_shrink()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.5))
                    .when_some(renamed_from, |label, old| {
                        label.child(
                            div()
                                .min_w(px(0.0))
                                .flex_shrink()
                                .truncate()
                                .text_color(colors().subtle)
                                .child(format!("{old} → ")),
                        )
                    })
                    .when(!parent.is_empty(), |label| {
                        label.child(
                            div()
                                .min_w(px(0.0))
                                .flex_shrink()
                                .truncate()
                                .text_color(colors().subtle)
                                .child(format!("{parent}/")),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors().foreground)
                            .child(name),
                    ),
            )
            .children(status_pill(change.status))
            .child(div().flex_1().min_w(px(8.0)))
            .when(comments > 0, |row| {
                row.child(
                    div()
                        .flex_none()
                        .h(px(18.0))
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .px(px(5.0))
                        .rounded(px(4.0))
                        .bg(tinted(colors().accent, 0.14))
                        .text_size(px(10.5))
                        .text_color(colors().accent)
                        .child(
                            gpui::svg()
                                .path("chrome-icons/comment.svg")
                                .size(px(10.0))
                                .text_color(colors().accent),
                        )
                        .child(comments.to_string()),
                )
            })
            .when(change.staged, |row| {
                row.child(
                    div()
                        .size(px(6.0))
                        .flex_none()
                        .rounded_full()
                        .bg(colors().diff_added),
                )
            })
            .when(additions > 0 || deletions > 0, |row| {
                row.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .font_family(MONO_FONT)
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .when(additions > 0, |stats| {
                            stats.child(
                                div()
                                    .text_color(colors().diff_added)
                                    .child(format!("+{additions}")),
                            )
                        })
                        .when(deletions > 0, |stats| {
                            stats.child(
                                div()
                                    .text_color(colors().diff_deleted)
                                    .child(format!("−{deletions}")),
                            )
                        })
                        .child(diffstat(additions, deletions)),
                )
            })
            .when_some(
                self.viewed
                    .as_ref()
                    .map(|viewed| viewed.get(&path).copied()),
                |row, state| {
                    let path = path.clone();
                    row.child(
                        viewed_toggle(
                            if pinned {
                                "review-sticky-viewed".into()
                            } else {
                                SharedString::from(format!("review-viewed-{path}"))
                            },
                            state,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_viewed(&path, cx);
                        })),
                    )
                },
            );

        div()
            .w_full()
            .flex_none()
            .px(px(CARD_INSET))
            .when(first, |frame| frame.pt(px(CARD_GAP)))
            .when(!open, |frame| frame.pb(px(CARD_GAP)))
            .child(header)
    }

    // -----------------------------------------------------------------------
    // Diff lines
    // -----------------------------------------------------------------------

    fn body_row(
        &mut self,
        ix: usize,
        path: &str,
        document: &Arc<DiffDocument>,
        row: BodyRow,
        metrics: RowMetrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gutter = metrics.gutter_width(document.max_line_number);
        let rows = &document.diff.rows;
        let anchors = body_row_anchors(rows, row);
        let button = |slot: usize, cx: &mut Context<Self>| -> Option<AnyElement> {
            let (side, line) = anchors[slot]?;
            let index = match row {
                BodyRow::Line(index) => index,
                BodyRow::Split { left, right } => if slot == 0 { left } else { right }?,
                BodyRow::Fold { .. } => return None,
            };
            let excerpt = document.display_lines.get(index)?.to_string();
            let anchor = CommentAnchor {
                path: path.to_owned(),
                side,
                line,
            };
            let group = match (row, slot) {
                (BodyRow::Line(_) | BodyRow::Fold { .. }, _) => "diff-line",
                (_, 0) => "diff-cell-left",
                _ => "diff-cell-right",
            };
            Some(
                Self::comment_button(ix * 2 + slot, group, metrics, anchor, excerpt, cx)
                    .into_any_element(),
            )
        };
        match row {
            BodyRow::Line(index) => {
                let Some(line) = rows.get(index) else {
                    return div().into_any_element();
                };
                match line.kind {
                    GitDiffRowKind::Hunk | GitDiffRowKind::Section | GitDiffRowKind::Notice => {
                        Self::banner_row(line, &document.display_lines[index], gutter, metrics)
                            .into_any_element()
                    }
                    _ => {
                        let button = button(0, cx);
                        Self::unified_line(document, index, gutter, metrics, button)
                            .into_any_element()
                    }
                }
            }
            BodyRow::Split { left, right } => {
                let buttons = [button(0, cx), button(1, cx)];
                Self::split_row(document, left, right, gutter, metrics, buttons).into_any_element()
            }
            BodyRow::Fold { fold, hidden, .. } => {
                let edges = document.folds.get(fold).map(|context| {
                    (
                        context.start == 0,
                        context.start + context.len >= rows.len(),
                    )
                });
                let (leading, trailing) = edges.unwrap_or_default();
                let actions = FoldActions {
                    ix,
                    path: path.to_owned(),
                    fold,
                    len: document
                        .folds
                        .get(fold)
                        .map_or(hidden, |context| context.len),
                };
                Self::fold_bar(
                    hidden,
                    leading,
                    trailing,
                    gutter,
                    metrics,
                    Some((actions, cx)),
                )
                .into_any_element()
            }
        }
    }

    fn reveal_fold(
        &mut self,
        path: String,
        fold: usize,
        len: usize,
        direction: FoldDirection,
        cx: &mut Context<Self>,
    ) {
        let reveals = self.fold_reveals.entry(path).or_default();
        let reveal = reveals.entry(fold).or_default();
        *reveal = reveal.expand(len, direction);
        cx.notify();
    }

    /// Soft tint of hunk separators and folds, with a stronger strip under
    /// the line numbers so they read as part of the gutter.
    fn separator_colors() -> (Rgba, Rgba) {
        let hunk = colors().diff_hunk_bg;
        (
            surface_tint(mix(colors().background, hunk, 0.55), colors().background),
            surface_tint(hunk, colors().background),
        )
    }

    /// Unchanged lines hidden between hunks: arrows in the gutter reveal
    /// `FOLD_STEP` lines from the adjacent hunk; the label reveals them all.
    /// `actions` is absent on the inert copy drawn inside a folding animation.
    fn fold_bar(
        hidden: usize,
        leading: bool,
        trailing: bool,
        gutter: f32,
        metrics: RowMetrics,
        actions: Option<(FoldActions, &mut Context<Self>)>,
    ) -> Div {
        let (background, gutter_background) = Self::separator_colors();
        let (actions, mut cx) = match actions {
            Some((actions, cx)) => (Some(actions), Some(cx)),
            None => (None, None),
        };
        let ix = actions.as_ref().map_or(0, |actions| actions.ix);
        let mut arrow = |icon: &'static str, direction: FoldDirection| {
            let group = SharedString::from(format!("diff-fold-{icon}-{ix}"));
            let button = div()
                .id(group.clone())
                .group(group.clone())
                .w(px(22.0))
                .h(px((metrics.fold_height - 6.0).max(16.0)))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.0))
                .hover(|button| button.bg(tinted(colors().accent, 0.18)))
                .child(
                    gpui::svg()
                        .path(icon)
                        .size(px(13.0))
                        .text_color(colors().muted)
                        .group_hover(group, |icon| icon.text_color(colors().foreground)),
                );
            match (&actions, cx.as_deref_mut()) {
                (Some(actions), Some(cx)) => {
                    let FoldActions {
                        path, fold, len, ..
                    } = actions.clone();
                    button
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.reveal_fold(path.clone(), fold, len, direction, cx);
                        }))
                }
                _ => button,
            }
        };
        let mut arrows = div()
            .w(px(gutter + metrics.sign_width()))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .bg(gutter_background);
        if !leading {
            arrows = arrows.child(arrow("chrome-icons/chevron-down.svg", FoldDirection::Down));
        }
        if !trailing {
            arrows = arrows.child(arrow("chrome-icons/chevron-up.svg", FoldDirection::Up));
        }
        let label_group = SharedString::from(format!("diff-fold-all-{ix}"));
        let label = div()
            .id(label_group.clone())
            .group(label_group.clone())
            .min_w(px(0.0))
            .flex_1()
            .h_full()
            .flex()
            .items_center()
            .gap(px(6.0))
            .pl(px(CODE_PADDING_LEFT + 4.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(11.5))
            .text_color(colors().subtle)
            .child(
                gpui::svg()
                    .path("chrome-icons/unfold-vertical.svg")
                    .size(px(12.0))
                    .flex_none()
                    .text_color(colors().subtle)
                    .group_hover(label_group.clone(), |icon| {
                        icon.text_color(colors().foreground)
                    }),
            )
            .child(
                div()
                    .group_hover(label_group, |text| text.text_color(colors().foreground))
                    .child(format!(
                        "{hidden} unmodified line{}",
                        if hidden == 1 { "" } else { "s" }
                    )),
            );
        div()
            .w_full()
            .h(px(metrics.fold_height))
            .flex_none()
            .flex()
            .items_center()
            .bg(background)
            .child(arrows)
            .child(match (actions, cx) {
                (
                    Some(FoldActions {
                        path, fold, len, ..
                    }),
                    Some(cx),
                ) => label
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.reveal_fold(path.clone(), fold, len, FoldDirection::All, cx);
                    })),
                _ => label,
            })
    }

    fn line_colors(kind: GitDiffRowKind) -> (Rgba, Rgba) {
        let background = match kind {
            GitDiffRowKind::Addition => colors().diff_added_bg,
            GitDiffRowKind::Deletion => colors().diff_deleted_bg,
            _ => colors().background,
        };
        // Changed lines tint their number more strongly than their code;
        // unchanged numbers sit directly on the page, without a gutter strip.
        let gutter = match kind {
            GitDiffRowKind::Addition => mix(background, colors().diff_added, 0.22),
            GitDiffRowKind::Deletion => mix(background, colors().diff_deleted, 0.22),
            _ => background,
        };
        (
            surface_tint(background, colors().background),
            surface_tint(gutter, background),
        )
    }

    fn unified_line(
        document: &DiffDocument,
        index: usize,
        gutter: f32,
        metrics: RowMetrics,
        button: Option<AnyElement>,
    ) -> Div {
        let row = &document.diff.rows[index];
        let (background, gutter_background) = Self::line_colors(row.kind);
        div()
            .group("diff-line")
            .w_full()
            .flex()
            .map(|line| {
                if metrics.wrap {
                    line.min_h(px(metrics.line_height))
                } else {
                    line.h(px(metrics.line_height))
                }
            })
            .bg(background)
            .child(Self::gutter_cell(
                if row.kind == GitDiffRowKind::Deletion {
                    row.old_line
                } else {
                    row.new_line
                },
                gutter,
                gutter_background,
                row.kind,
                metrics,
                button,
            ))
            .child(Self::sign_cell(row.kind, metrics))
            .child(Self::code_cell(document, index, row.kind, metrics))
    }

    fn split_row(
        document: &DiffDocument,
        left: Option<usize>,
        right: Option<usize>,
        gutter: f32,
        metrics: RowMetrics,
        [left_button, right_button]: [Option<AnyElement>; 2],
    ) -> Div {
        div()
            .w_full()
            .flex()
            .when(!metrics.wrap, |row| row.h(px(metrics.line_height)))
            .child(Self::split_cell(
                document,
                left,
                true,
                gutter,
                metrics,
                left_button,
            ))
            .child(
                div()
                    .w(px(SPLIT_DIVIDER_WIDTH))
                    .flex_none()
                    .bg(colors().border_subtle),
            )
            .child(Self::split_cell(
                document,
                right,
                false,
                gutter,
                metrics,
                right_button,
            ))
    }

    fn split_cell(
        document: &DiffDocument,
        index: Option<usize>,
        left: bool,
        gutter: f32,
        metrics: RowMetrics,
        button: Option<AnyElement>,
    ) -> Div {
        let cell = div()
            .group(if left {
                "diff-cell-left"
            } else {
                "diff-cell-right"
            })
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .overflow_hidden();
        let Some(row) = index.and_then(|index| document.diff.rows.get(index)) else {
            // Blank half: the other side added or removed lines here.
            return cell.bg(surface_tint(
                mix(colors().background, colors().foreground, 0.03),
                colors().background,
            ));
        };
        let index = index.unwrap_or_default();
        let kind = row.kind;
        let (background, gutter_background) = Self::line_colors(kind);
        let number = if left { row.old_line } else { row.new_line };
        cell.bg(background)
            .child(Self::gutter_cell(
                number,
                gutter,
                gutter_background,
                kind,
                metrics,
                button,
            ))
            .child(Self::sign_cell(kind, metrics))
            .child(Self::code_cell(document, index, kind, metrics))
    }

    fn gutter_cell(
        number: Option<usize>,
        width: f32,
        background: Rgba,
        kind: GitDiffRowKind,
        metrics: RowMetrics,
        button: Option<AnyElement>,
    ) -> Div {
        let changed = match kind {
            GitDiffRowKind::Addition => Some(colors().diff_added),
            GitDiffRowKind::Deletion => Some(colors().diff_deleted),
            _ => None,
        };
        div()
            .relative()
            .w(px(width))
            .flex_none()
            .flex()
            .justify_end()
            .pr_2()
            .bg(background)
            .font_family(MONO_FONT)
            .text_size(px(metrics.font_size - 1.0))
            .line_height(px(metrics.line_height))
            .text_color(changed.map_or(colors().subtle, |color| {
                mix(color, colors().foreground, 0.12)
            }))
            .child(number.map(|line| line.to_string()).unwrap_or_default())
            .when_some(changed, |cell, color| {
                cell.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(CHANGE_BAR_WIDTH))
                        .bg(color),
                )
            })
            .children(button)
    }

    /// `+` or `−` before changed code, so a line's role never rests on color.
    fn sign_cell(kind: GitDiffRowKind, metrics: RowMetrics) -> Div {
        let (sign, color) = match kind {
            GitDiffRowKind::Addition => ("+", colors().diff_added),
            GitDiffRowKind::Deletion => ("−", colors().diff_deleted),
            _ => ("", colors().subtle),
        };
        div()
            .w(px(metrics.sign_width()))
            .flex_none()
            .flex()
            .justify_center()
            .font_family(MONO_FONT)
            .text_size(px(metrics.font_size))
            .line_height(px(metrics.line_height))
            .text_color(color)
            .child(sign)
    }

    /// Revealed while its row (or split half) `group` is hovered.
    fn comment_button(
        id: usize,
        group: &'static str,
        metrics: RowMetrics,
        anchor: CommentAnchor,
        excerpt: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(("diff-comment", id))
            .absolute()
            .top(px(
                ((metrics.line_height - COMMENT_BUTTON_SIZE) / 2.0).max(0.0)
            ))
            .left(px((COMMENT_ACTION_WIDTH - COMMENT_BUTTON_SIZE) / 2.0 + 2.0))
            .size(px(COMMENT_BUTTON_SIZE))
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(colors().accent)
            .cursor_pointer()
            .opacity(0.0)
            .group_hover(group, |style| style.opacity(1.0))
            .child(
                gpui::svg()
                    .path("chrome-icons/plus.svg")
                    .size(px(11.0))
                    .text_color(colors().background),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.open_draft(anchor.clone(), excerpt.clone(), window, cx);
            }))
    }

    fn code_cell(
        document: &DiffDocument,
        index: usize,
        kind: GitDiffRowKind,
        metrics: RowMetrics,
    ) -> Div {
        let code = Self::styled_code_line(
            &document.display_lines[index],
            document
                .highlights
                .get(index)
                .map_or(&[][..], Vec::as_slice),
            document.emphasis.get(index).map_or(&[][..], Vec::as_slice),
            kind,
            metrics,
        );
        let cell = div()
            .flex_1()
            .min_w(px(0.0))
            .font_family(MONO_FONT)
            .text_size(px(metrics.font_size))
            .line_height(px(metrics.line_height))
            // Unchanged code recedes so the changes read first.
            .when(kind == GitDiffRowKind::Context, |cell| cell.opacity(0.72));
        if metrics.wrap {
            cell.pl(px(CODE_PADDING_LEFT))
                .pr(px(CODE_PADDING_LEFT * 2.0))
                .child(code)
        } else {
            // Only the code plane moves; line numbers and comment actions stay fixed.
            cell.h_full().overflow_hidden().child(
                div()
                    .relative()
                    .left(px(-metrics.h_offset))
                    .pl(px(CODE_PADDING_LEFT))
                    .whitespace_nowrap()
                    .child(code),
            )
        }
    }

    /// Hunk, section, and notice rows span the full width in both layouts.
    fn banner_row(row: &GitDiffRow, text: &SharedString, gutter: f32, metrics: RowMetrics) -> Div {
        if row.kind == GitDiffRowKind::Hunk {
            // `@@ -3,3 +3,3 @@ fn name()`: the range is bookkeeping, the
            // enclosing scope after it is what orients a reader.
            let (range, scope) = split_hunk_header(text);
            let (background, gutter_background) = Self::separator_colors();
            return div()
                .w_full()
                .h(px(metrics.hunk_height))
                .flex()
                .items_center()
                .overflow_hidden()
                .bg(background)
                .child(
                    div()
                        .w(px(gutter + metrics.sign_width()))
                        .h_full()
                        .flex_none()
                        .bg(gutter_background),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .flex_1()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .pl(px(CODE_PADDING_LEFT + 4.0))
                        .pr_3()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family(MONO_FONT)
                        .text_size(px(metrics.font_size - 1.0))
                        .child(
                            div()
                                .flex_none()
                                .text_color(colors().subtle)
                                .child(range.to_owned()),
                        )
                        .when(!scope.is_empty(), |line| {
                            line.child(
                                div()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_color(colors().muted)
                                    .child(scope.to_owned()),
                            )
                        }),
                );
        }
        let (height, background, color) = match row.kind {
            GitDiffRowKind::Section => (SECTION_HEIGHT - 4.0, colors().elevated, colors().subtle),
            _ => (metrics.line_height, colors().background, colors().warning),
        };
        div()
            .w_full()
            .h(px(height))
            .flex()
            .items_center()
            .overflow_hidden()
            .bg(surface_tint(background, colors().background))
            .pl(px(12.0))
            .pr_3()
            .font_family(MONO_FONT)
            .text_size(px(if row.kind == GitDiffRowKind::Section {
                10.0
            } else {
                metrics.font_size - 1.0
            }))
            .when(row.kind == GitDiffRowKind::Section, |row| {
                row.font_weight(FontWeight::MEDIUM)
            })
            .text_color(color)
            .whitespace_nowrap()
            .child(text.clone())
    }

    fn styled_code_line(
        text: &SharedString,
        spans: &[SyntaxSpan],
        emphasis: &[Range<usize>],
        kind: GitDiffRowKind,
        metrics: RowMetrics,
    ) -> StyledText {
        let default_style = TextStyle {
            // Context dims through its cell's opacity, syntax colors included.
            color: colors().foreground.into(),
            font_family: MONO_FONT.into(),
            font_size: px(metrics.font_size).into(),
            line_height: px(metrics.line_height).into(),
            white_space: if metrics.wrap {
                WhiteSpace::Normal
            } else {
                WhiteSpace::Nowrap
            },
            ..Default::default()
        };
        if !matches!(
            kind,
            GitDiffRowKind::Context | GitDiffRowKind::Addition | GitDiffRowKind::Deletion
        ) {
            return StyledText::new(text.clone()).with_default_highlights(
                &default_style,
                std::iter::empty::<(Range<usize>, HighlightStyle)>(),
            );
        }
        let alpha = if colors().is_dark() { 0.28 } else { 0.2 };
        let mark = match kind {
            GitDiffRowKind::Addition => Some(tinted(colors().diff_added, alpha)),
            GitDiffRowKind::Deletion => Some(tinted(colors().diff_deleted, alpha)),
            _ => None,
        };
        StyledText::new(text.clone())
            .with_default_highlights(&default_style, code_highlights(text, spans, emphasis, mark))
    }

    /// A body folding open or shut: a clipped stand-in whose height tweens,
    /// built only from the rows the clip can reveal.
    fn folding_body(&self, path: &str, metrics: RowMetrics, viewport_height: f32) -> AnyElement {
        let (Some(document), Some(fold)) = (self.document(path), self.folds.get(path)) else {
            return div().into_any_element();
        };
        let rows = &document.diff.rows;
        let viewport = viewport_height.max(200.0);
        let gutter = metrics.gutter_width(document.max_line_number);
        let mut height = 0.0;
        let mut children = Vec::new();
        let no_reveals = HashMap::new();
        let reveals = self.fold_reveals.get(path).unwrap_or(&no_reveals);
        for row in body_rows(rows, self.layout, &document.folds, reveals) {
            if height >= viewport {
                break;
            }
            height += metrics.body_row_height(rows, row);
            children.push(match row {
                BodyRow::Line(index) => match rows[index].kind {
                    GitDiffRowKind::Hunk | GitDiffRowKind::Section | GitDiffRowKind::Notice => {
                        Self::banner_row(
                            &rows[index],
                            &document.display_lines[index],
                            gutter,
                            metrics,
                        )
                    }
                    _ => Self::unified_line(document, index, gutter, metrics, None),
                },
                BodyRow::Split { left, right } => Self::split_row(
                    document,
                    left,
                    right,
                    gutter,
                    RowMetrics {
                        wrap: false,
                        ..metrics
                    },
                    [None, None],
                ),
                BodyRow::Fold { hidden, .. } => {
                    Self::fold_bar(hidden, false, false, gutter, metrics, None)
                }
            });
        }
        let height = height.min(viewport);
        let expanding = fold.expanding;
        div()
            .w_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .children(children)
            .with_animation(
                ("review-fold", fold.generation as usize),
                Animation::new(FOLD_DURATION).with_easing(ease_out_quint()),
                move |body, delta| {
                    let progress = if expanding { delta } else { 1.0 - delta };
                    body.h(px(height * progress))
                },
            )
            .into_any_element()
    }

    // -----------------------------------------------------------------------
    // Comments
    // -----------------------------------------------------------------------

    fn card_indent(&self, metrics: RowMetrics) -> f32 {
        if self.layout.is_split() {
            12.0
        } else {
            metrics.gutter_width(99) + metrics.sign_width()
        }
    }

    fn comment_card(
        &self,
        comment: &ReviewComment,
        metrics: RowMetrics,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = comment.id;
        let cite = match comment.anchor.side {
            CommentSide::New => format!("Line {}", comment.anchor.line),
            CommentSide::Old => format!("Removed line {}", comment.anchor.line),
        };
        div()
            .w_full()
            .py(px(6.0))
            .pl(px(self.card_indent(metrics)))
            .pr_3()
            .bg(surface_tint(colors().background, colors().panel))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(surface_tint(colors().elevated, colors().panel))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                gpui::svg()
                                    .path("chrome-icons/comment.svg")
                                    .size(px(11.0))
                                    .text_color(colors().accent),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.0))
                                    .text_color(colors().subtle)
                                    .child(cite),
                            )
                            .child(Self::card_action(
                                ("comment-edit", id as usize),
                                "Edit",
                                cx.listener(move |this, _, window, cx| {
                                    this.edit_comment(id, window, cx);
                                }),
                            ))
                            .child(Self::card_action(
                                ("comment-delete", id as usize),
                                "Delete",
                                cx.listener(move |this, _, _, cx| this.delete_comment(id, cx)),
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(17.0))
                            .text_color(colors().foreground)
                            .child(comment.body.clone()),
                    ),
            )
    }

    /// A published conversation: read-only, replies happen on the source service.
    fn thread_card(&self, thread: &RemoteThread, metrics: RowMetrics) -> Div {
        div()
            .w_full()
            .py(px(6.0))
            .pl(px(self.card_indent(metrics)))
            .pr_3()
            .bg(surface_tint(colors().background, colors().panel))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(surface_tint(colors().panel, colors().background))
                    .when(thread.resolved, |card| card.opacity(0.7))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(px(10.0))
                            .text_color(colors().subtle)
                            .child(
                                gpui::svg()
                                    .path("chrome-icons/github.svg")
                                    .size(px(11.0))
                                    .text_color(colors().subtle),
                            )
                            .child(format!(
                                "{} comment{}",
                                thread.comments.len(),
                                if thread.comments.len() == 1 { "" } else { "s" }
                            ))
                            .when(thread.resolved, |row| row.child("· Resolved")),
                    )
                    .children(thread.comments.iter().map(|(author, body)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(colors().muted)
                                    .child(author.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .line_height(px(17.0))
                                    .text_color(colors().foreground)
                                    .child(body.clone()),
                            )
                    })),
            )
    }

    fn card_action(
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .text_size(px(10.5))
            .text_color(colors().muted)
            .cursor_pointer()
            .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
            .child(label)
            .on_click(on_click)
    }

    fn draft_card(&self, metrics: RowMetrics, cx: &mut Context<Self>) -> Div {
        let Some(draft) = self.draft.as_ref() else {
            return div();
        };
        let empty = draft.body.trim().is_empty();
        let cite = match draft.anchor.side {
            CommentSide::New => format!("Comment on line {}", draft.anchor.line),
            CommentSide::Old => format!("Comment on removed line {}", draft.anchor.line),
        };
        let body = if draft.body.is_empty() {
            div()
                .text_color(colors().subtle)
                .child(if self.external.is_some() {
                    "Write a review comment…"
                } else {
                    "Tell the agent what to change here…"
                })
        } else {
            div()
                .text_color(colors().foreground)
                .child(format!("{}▍", draft.body))
        };
        div()
            .w_full()
            .py(px(6.0))
            .pl(px(self.card_indent(metrics)))
            .pr_3()
            .bg(surface_tint(colors().background, colors().panel))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(colors().accent)
                    .bg(surface_tint(colors().elevated, colors().panel))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(colors().subtle)
                            .child(cite),
                    )
                    .child(
                        div()
                            .min_h(px(34.0))
                            .text_size(px(12.0))
                            .line_height(px(17.0))
                            .child(body),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(9.5))
                                    .text_color(colors().subtle)
                                    .child("↵ save · ⇧↵ new line · esc cancel"),
                            )
                            .child(Self::card_action(
                                "comment-draft-cancel",
                                "Cancel",
                                cx.listener(|this, _, _, cx| this.cancel_draft(cx)),
                            ))
                            .child(
                                div()
                                    .id("comment-draft-save")
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(5.0))
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .bg(if empty {
                                        colors().selection
                                    } else {
                                        colors().accent
                                    })
                                    .text_color(if empty {
                                        colors().subtle
                                    } else {
                                        colors().background
                                    })
                                    .when(!empty, |button| button.cursor_pointer())
                                    .child("Comment")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.commit_draft();
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }

    // -----------------------------------------------------------------------
    // Chrome
    // -----------------------------------------------------------------------
}

/// Syntax colors with word-level emphasis laid over them: one sorted,
/// non-overlapping run per stretch that differs from the default style.
fn code_highlights(
    text: &str,
    spans: &[SyntaxSpan],
    emphasis: &[Range<usize>],
    mark: Option<Hsla>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let valid = |range: &Range<usize>| {
        range.start < range.end
            && range.end <= text.len()
            && text.is_char_boundary(range.start)
            && text.is_char_boundary(range.end)
    };
    let mut end = 0;
    let syntax: Vec<(Range<usize>, HighlightStyle)> = spans
        .iter()
        .filter(|span| {
            let keep = valid(&span.range) && span.range.start >= end;
            if keep {
                end = span.range.end;
            }
            keep
        })
        .map(|span| (span.range.clone(), span.kind.highlight_style()))
        .collect();
    let marks: Vec<&Range<usize>> = match mark {
        Some(_) => emphasis.iter().filter(|range| valid(range)).collect(),
        None => Vec::new(),
    };
    if marks.is_empty() {
        return syntax;
    }
    let mut cuts: Vec<usize> = syntax
        .iter()
        .flat_map(|(range, _)| [range.start, range.end])
        .chain(marks.iter().flat_map(|range| [range.start, range.end]))
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let (mut next_syntax, mut next_mark) = (0, 0);
    let mut out = Vec::with_capacity(cuts.len());
    for cut in cuts.windows(2) {
        let (start, end) = (cut[0], cut[1]);
        while syntax
            .get(next_syntax)
            .is_some_and(|(range, _)| range.end <= start)
        {
            next_syntax += 1;
        }
        while marks.get(next_mark).is_some_and(|range| range.end <= start) {
            next_mark += 1;
        }
        let style = syntax
            .get(next_syntax)
            .filter(|(range, _)| range.start <= start)
            .map(|(_, style)| *style);
        let marked = marks
            .get(next_mark)
            .is_some_and(|range| range.start <= start);
        if style.is_none() && !marked {
            continue;
        }
        let mut style = style.unwrap_or_default();
        if marked {
            style.background_color = mark;
        }
        out.push((start..end, style));
    }
    out
}

/// `@@ -3,3 +3,3 @@ fn name()` → (`@@ -3,3 +3,3 @@`, `fn name()`).
fn split_hunk_header(text: &str) -> (&str, &str) {
    text.strip_prefix("@@")
        .and_then(|rest| rest.find("@@"))
        .map(|close| {
            let end = close + 4;
            (&text[..end], text[end..].trim())
        })
        .unwrap_or((text, ""))
}

/// Where a file header sits in the list.
#[derive(Debug, Clone, Copy)]
pub(super) struct HeaderPlacement {
    /// Body rows follow, so the card stays open below the header.
    pub open: bool,
    /// The list's first row, which keeps a gap above it.
    pub first: bool,
}

/// `color` at `alpha`, painted over whatever surface is below.
fn tinted(color: Rgba, alpha: f32) -> Hsla {
    Rgba { a: alpha, ..color }.into()
}

/// A short label for every status but the common "modified".
fn status_pill(status: GitFileStatus) -> Option<Div> {
    let (label, color) = match status {
        GitFileStatus::Added | GitFileStatus::Untracked => ("New", colors().diff_added),
        GitFileStatus::Deleted => ("Deleted", colors().diff_deleted),
        GitFileStatus::Renamed => ("Renamed", colors().accent),
        GitFileStatus::Copied => ("Copied", colors().accent),
        GitFileStatus::Conflicted => ("Conflict", colors().warning),
        GitFileStatus::Modified | GitFileStatus::TypeChanged => return None,
    };
    Some(
        div()
            .flex_none()
            .h(px(18.0))
            .px(px(6.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .bg(tinted(color, 0.14))
            .text_size(px(10.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .child(label),
    )
}

/// Blocks filled in proportion to added and removed lines, as on GitHub.
/// Small changes leave some blocks neutral, so size reads at a glance.
fn diffstat_blocks(additions: usize, deletions: usize) -> (usize, usize) {
    const BLOCKS: usize = 5;
    let total = additions + deletions;
    if total == 0 {
        return (0, 0);
    }
    let filled = total.min(BLOCKS);
    let mut added = (filled as f32 * additions as f32 / total as f32).round() as usize;
    if additions > 0 {
        added = added.max(1);
    }
    if deletions > 0 {
        added = added.min(filled - 1);
    }
    (added, filled - added)
}

fn diffstat(additions: usize, deletions: usize) -> Div {
    let (added, deleted) = diffstat_blocks(additions, deletions);
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(2.0))
        .children((0..5).map(|block| {
            div().size(px(7.0)).rounded(px(1.5)).bg(if block < added {
                tinted(colors().diff_added, 1.0)
            } else if block < added + deleted {
                tinted(colors().diff_deleted, 1.0)
            } else {
                tinted(colors().foreground, 0.14)
            })
        }))
}

/// GitHub-style "Viewed" checkbox for a file header.
fn viewed_toggle(id: SharedString, state: Option<FileViewed>) -> Stateful<Div> {
    let viewed = state == Some(FileViewed::Viewed);
    div()
        .id(id)
        .flex_none()
        .h(px(22.0))
        .px(px(6.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(if viewed {
            colors().accent
        } else {
            colors().border_subtle
        })
        .when(viewed, |toggle| toggle.bg(colors().selection))
        .cursor_pointer()
        .hover(|toggle| toggle.bg(colors().hover))
        .text_size(px(10.5))
        .text_color(if viewed {
            colors().foreground
        } else {
            colors().muted
        })
        .child(
            div()
                .size(px(11.0))
                .flex_none()
                .rounded(px(3.0))
                .border_1()
                .border_color(if viewed {
                    colors().accent
                } else {
                    colors().subtle
                })
                .when(viewed, |box_| box_.bg(colors().accent))
                .flex()
                .items_center()
                .justify_center()
                .when(viewed, |box_| {
                    box_.child(
                        gpui::svg()
                            .path("chrome-icons/check.svg")
                            .size(px(9.0))
                            .text_color(colors().background),
                    )
                }),
        )
        .child(if state == Some(FileViewed::Dismissed) {
            "Changed since viewed"
        } else {
            "Viewed"
        })
}

#[cfg(test)]
mod tests {
    use super::{code_highlights, diffstat_blocks, split_hunk_header};
    use crate::ui::syntax::{SyntaxKind, SyntaxSpan};

    #[test]
    fn emphasis_splits_syntax_runs_without_overlap() {
        let text = "let total = count;";
        let spans = [
            SyntaxSpan {
                range: 0..3,
                kind: SyntaxKind::Keyword,
            },
            SyntaxSpan {
                range: 10..11,
                kind: SyntaxKind::Operator,
            },
        ];
        let mark = Some(gpui::Hsla::red());
        let runs = code_highlights(text, &spans, &[1..9, 12..17], mark);
        let ranges: Vec<_> = runs.iter().map(|(range, _)| range.clone()).collect();
        assert_eq!(ranges, [0..1, 1..3, 3..9, 10..11, 12..17]);
        assert!(runs[0].1.background_color.is_none());
        assert!(runs[1].1.color.is_some() && runs[1].1.background_color == mark);
        assert!(runs[2].1.color.is_none() && runs[2].1.background_color == mark);
        assert!(runs[3].1.background_color.is_none());
        // Without a mark (context lines), syntax passes through untouched.
        let mark = 1..9;
        assert_eq!(
            code_highlights(text, &spans, std::slice::from_ref(&mark), None).len(),
            2
        );
    }

    #[test]
    fn hunk_headers_separate_the_range_from_the_scope() {
        assert_eq!(
            split_hunk_header("@@ -3,3 +3,4 @@ fn main() {"),
            ("@@ -3,3 +3,4 @@", "fn main() {")
        );
        assert_eq!(split_hunk_header("@@ -1 +1 @@"), ("@@ -1 +1 @@", ""));
        assert_eq!(
            split_hunk_header("Binary files differ"),
            ("Binary files differ", "")
        );
    }

    #[test]
    fn diffstat_shows_both_sides_and_the_size_of_small_changes() {
        assert_eq!(diffstat_blocks(0, 0), (0, 0));
        assert_eq!(diffstat_blocks(1, 0), (1, 0));
        assert_eq!(diffstat_blocks(100, 0), (5, 0));
        assert_eq!(diffstat_blocks(0, 34), (0, 5));
        assert_eq!(diffstat_blocks(99, 1), (4, 1));
        assert_eq!(diffstat_blocks(1, 99), (1, 4));
        assert_eq!(diffstat_blocks(6, 9), (2, 3));
    }
}
