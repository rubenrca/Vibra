//! Virtualized review rows, folding, scrolling, and comment cards.

use super::{
    BASE_DIFF_FONT_SIZE, BASE_DIFF_ROW_HEIGHT, CODE_PADDING_LEFT, CODE_PADDING_RIGHT,
    COMMENT_ACTION_WIDTH, COMMENT_BUTTON_SIZE, DiffView, FILE_HEADER_HEIGHT, FOLD_DURATION,
    FoldActions, GitPanelMode, RowKey, RowMetrics, SECTION_HEIGHT, SPLIT_DIVIDER_WIDTH,
    STICKY_PUSH_SCAN, git_status_badge,
};
use crate::ports::files::FileEntryKind;
use crate::ports::git::{GitDiffRow, GitDiffRowKind, GitFileChange};
use crate::ui::diff_document::DiffDocument;
use crate::ui::diff_rows::{
    BodyRow, CommentAnchor, CommentSide, FlattenFile, FoldDirection, ReviewComment, ReviewRow,
    body_row_anchors, body_rows, flatten,
};
use crate::ui::syntax::SyntaxSpan;
use crate::ui::theme::{MONO_FONT, colors, mix, surface_tint};
use crate::ui::workspace_view::{file_tree_icon, file_tree_icon_color};
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, Div, HighlightStyle, ListOffset, Rgba,
    SharedString, Stateful, StyledText, TextStyle, WhiteSpace, Window, div, ease_out_quint,
    prelude::*, px,
};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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
        let rows = flatten(
            &entries,
            self.layout,
            &self.comments,
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
        let gutter = metrics.gutter_width(max_line);
        let code_width = if self.layout.is_split() {
            (viewport - SPLIT_DIVIDER_WIDTH) / 2.0 - gutter
        } else {
            viewport - gutter
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
        if matches!(row, ReviewRow::FileHeader { .. }) && top.offset_in_item <= px(0.0) {
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
        match row {
            ReviewRow::StagedSection => {
                let count = files.iter().filter(|change| change.staged).count();
                Self::file_section_header("Staged", count).into_any_element()
            }
            ReviewRow::FileHeader { file } => match files.get(file) {
                Some(change) => self.file_header(change, false, cx).into_any_element(),
                None => div().into_any_element(),
            },
            ReviewRow::FileLoading { .. } => div()
                .w_full()
                .px_3()
                .py_3()
                .bg(surface_tint(colors().background, colors().panel))
                .text_size(px(11.0))
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
            ReviewRow::Draft { .. } => self.draft_card(metrics, cx).into_any_element(),
            ReviewRow::Folding { file } => match files.get(file) {
                Some(change) => self.folding_body(&change.path, metrics, viewport_height),
                None => div().into_any_element(),
            },
        }
    }

    fn file_section_header(label: &'static str, count: usize) -> Div {
        div()
            .h(px(SECTION_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px_3()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().muted)
                    .child(label),
            )
            .child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(4.0))
                    .bg(colors().elevated)
                    .text_size(px(9.0))
                    .text_color(colors().subtle)
                    .child(count.to_string()),
            )
    }

    /// `pinned` is the sticky copy over the list: folding from it scrolls the
    /// file back into view, since its own header sits above the viewport.
    pub(super) fn file_header(
        &self,
        change: &GitFileChange,
        pinned: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id: gpui::ElementId = if pinned {
            "review-sticky-header".into()
        } else {
            SharedString::from(format!("review-file-{}", change.path)).into()
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

        div()
            .id(id)
            .h(px(FILE_HEADER_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px_3()
            .border_b_1()
            .border_color(colors().border_subtle)
            .bg(surface_tint(
                mix(colors().background, colors().foreground, 0.02),
                colors().background,
            ))
            .cursor_pointer()
            .hover(|row| row.bg(surface_tint(colors().hover, colors().panel)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if pinned {
                    this.pending_reveal = Some(path_for_click.clone());
                }
                this.toggle_path(path_for_click.clone(), cx);
            }))
            .child(
                div()
                    .w(px(12.0))
                    .h(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        gpui::svg()
                            .path(if expanded {
                                "chrome-icons/chevron-down.svg"
                            } else {
                                "chrome-icons/chevron-right.svg"
                            })
                            .size(px(14.0))
                            .flex_none()
                            .text_color(colors().subtle),
                    ),
            )
            .child({
                let name = change.path.rsplit('/').next().unwrap_or(&change.path);
                div()
                    .w(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(file_tree_icon(
                        FileEntryKind::File,
                        false,
                        name,
                        file_tree_icon_color(FileEntryKind::File, name),
                    ))
            })
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(px(12.0))
                    .text_color(mix(colors().foreground, colors().background, 0.15))
                    .child(change.path.clone()),
            )
            .when(comments > 0, |row| {
                row.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .px_1()
                        .rounded(px(4.0))
                        .bg(colors().selection)
                        .text_size(px(10.0))
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
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
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
                                    .child(format!("-{deletions}")),
                            )
                        }),
                )
            })
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
                let left_button = button(0, cx);
                let right_button = button(1, cx);
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
                    .into_any_element()
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
                Self::fold_bar(hidden, leading, trailing, metrics, Some((actions, cx)))
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

    /// Unchanged lines hidden between hunks: arrows reveal `FOLD_STEP` lines
    /// from the adjacent hunk; the label reveals them all. `actions` is absent
    /// on the inert copy drawn inside a folding animation.
    fn fold_bar(
        hidden: usize,
        leading: bool,
        trailing: bool,
        metrics: RowMetrics,
        actions: Option<(FoldActions, &mut Context<Self>)>,
    ) -> Div {
        let mut bar = div()
            .w_full()
            .h(px(metrics.fold_height))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .bg(surface_tint(
                mix(colors().background, colors().foreground, 0.08),
                colors().background,
            ));
        let (actions, mut cx) = match actions {
            Some((actions, cx)) => (Some(actions), Some(cx)),
            None => (None, None),
        };
        let mut arrow = |icon: &'static str, direction: FoldDirection| {
            let group = SharedString::from(format!(
                "diff-fold-{icon}-{}",
                actions.as_ref().map_or(0, |actions| actions.ix)
            ));
            let button = div()
                .id(group.clone())
                .group(group.clone())
                .size(px(20.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .hover(|button| button.bg(surface_tint(colors().hover, colors().background)))
                .child(
                    gpui::svg()
                        .path(icon)
                        .size(px(12.0))
                        .text_color(colors().subtle)
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
        if !leading {
            bar = bar.child(arrow("chrome-icons/chevron-down.svg", FoldDirection::Down));
        }
        if !trailing {
            bar = bar.child(arrow("chrome-icons/chevron-up.svg", FoldDirection::Up));
        }
        let label = div()
            .id(SharedString::from(format!(
                "diff-fold-all-{}",
                actions.as_ref().map_or(0, |actions| actions.ix)
            )))
            .min_w(px(0.0))
            .flex_1()
            .h_full()
            .flex()
            .items_center()
            .pl_1()
            .truncate()
            .font_family(MONO_FONT)
            .text_size(px(metrics.font_size - 1.0))
            .text_color(colors().subtle)
            .hover(|label| label.text_color(colors().muted))
            .child(format!(
                "{hidden} unmodified line{}",
                if hidden == 1 { "" } else { "s" }
            ));
        bar.child(match (actions, cx) {
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
            .child(Self::code_cell(
                &document.display_lines[index],
                document
                    .highlights
                    .get(index)
                    .map_or(&[][..], Vec::as_slice),
                row.kind,
                metrics,
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
            return cell.bg(surface_tint(colors().elevated, colors().background));
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
            .child(Self::code_cell(
                &document.display_lines[index],
                document
                    .highlights
                    .get(index)
                    .map_or(&[][..], Vec::as_slice),
                kind,
                metrics,
            ))
    }

    fn gutter_cell(
        number: Option<usize>,
        width: f32,
        background: Rgba,
        kind: GitDiffRowKind,
        metrics: RowMetrics,
        button: Option<AnyElement>,
    ) -> Div {
        let color = match kind {
            GitDiffRowKind::Addition => colors().diff_added,
            GitDiffRowKind::Deletion => colors().diff_deleted,
            _ => colors().subtle,
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
            .text_color(color)
            .child(number.map(|line| line.to_string()).unwrap_or_default())
            .children(button)
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
            .left(px((COMMENT_ACTION_WIDTH - COMMENT_BUTTON_SIZE) / 2.0))
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
        text: &SharedString,
        spans: &[SyntaxSpan],
        kind: GitDiffRowKind,
        metrics: RowMetrics,
    ) -> Div {
        let code = Self::styled_code_line(text, spans, kind, metrics);
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
                .pr(px(CODE_PADDING_LEFT))
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
        let (height, background, color) = match row.kind {
            GitDiffRowKind::Hunk => (metrics.hunk_height, colors().diff_hunk_bg, colors().subtle),
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
            .pl(px(if row.kind == GitDiffRowKind::Hunk {
                gutter + CODE_PADDING_LEFT
            } else {
                12.0
            }))
            .pr_3()
            .font_family(MONO_FONT)
            .text_size(px(if row.kind == GitDiffRowKind::Section {
                10.0
            } else {
                metrics.font_size - 1.0
            }))
            .when(row.kind == GitDiffRowKind::Section, |row| {
                row.font_weight(gpui::FontWeight::MEDIUM)
            })
            .text_color(color)
            .whitespace_nowrap()
            .child(text.clone())
    }

    fn styled_code_line(
        text: &SharedString,
        spans: &[SyntaxSpan],
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
        if spans.is_empty()
            || !matches!(
                kind,
                GitDiffRowKind::Context | GitDiffRowKind::Addition | GitDiffRowKind::Deletion
            )
        {
            return StyledText::new(text.clone()).with_default_highlights(
                &default_style,
                std::iter::empty::<(std::ops::Range<usize>, HighlightStyle)>(),
            );
        }
        let highlights = spans.iter().filter_map(|span| {
            if span.range.start >= text.len() || span.range.end > text.len() {
                return None;
            }
            if !text.is_char_boundary(span.range.start) || !text.is_char_boundary(span.range.end) {
                return None;
            }
            Some((span.range.clone(), span.kind.highlight_style()))
        });
        StyledText::new(text.clone()).with_default_highlights(&default_style, highlights)
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
                BodyRow::Split { left, right } => div()
                    .w_full()
                    .h(px(metrics.line_height))
                    .flex()
                    .child(Self::split_cell(
                        document, left, true, gutter, metrics, None,
                    ))
                    .child(
                        div()
                            .w(px(SPLIT_DIVIDER_WIDTH))
                            .flex_none()
                            .bg(colors().border_subtle),
                    )
                    .child(Self::split_cell(
                        document, right, false, gutter, metrics, None,
                    )),
                BodyRow::Fold { hidden, .. } => Self::fold_bar(hidden, false, false, metrics, None),
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
            metrics.gutter_width(99)
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
                .child("Tell the agent what to change here…")
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
