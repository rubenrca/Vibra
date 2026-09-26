//! The Changes sidebar, laid out like an IDE's source control view: branch
//! and actions on top, a commit box, the changed files, and the commit graph.
//! The review itself opens beside the terminal, in the center.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, FocusHandle, KeyDownEvent, SharedString, Task, Window, div, prelude::*,
    px, relative, svg, uniform_list,
};

use super::{DiffView, DiffViewEvent, GitPanelMode};
use crate::ports::git::{GitCommit, GitCommitOptions, GitSyncOperation};
use crate::ui::text_edit::{TextKeyOutcome, apply_text_key};
use crate::ui::theme::{colors, popover_surface, surface_tint};

const GRAPH_ROW_HEIGHT: f32 = 26.0;
/// The graph polls less often than the file list; commits change rarely.
const GRAPH_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
const MAX_COMMIT_MESSAGE_CHARS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommitAction {
    Commit,
    CommitAndPush,
    Amend,
}

impl CommitAction {
    fn label(self) -> &'static str {
        match self {
            Self::Commit => "Commit",
            Self::CommitAndPush => "Commit y push",
            Self::Amend => "Modificar último commit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelMenu {
    Commit,
    More,
}

pub(super) struct ChangesPanelState {
    pub(super) message: String,
    focus: FocusHandle,
    busy: Option<&'static str>,
    /// Last result of a commit or sync: text and whether it failed.
    pub(super) feedback: Option<(SharedString, bool)>,
    menu: Option<PanelMenu>,
    graph_open: bool,
    graph_refreshed_at: Option<Instant>,
    generating: bool,
    _task: Option<Task<()>>,
    _generate_task: Option<Task<()>>,
}

impl ChangesPanelState {
    pub(super) fn new(cx: &mut Context<DiffView>) -> Self {
        Self {
            message: String::new(),
            focus: cx.focus_handle(),
            busy: None,
            feedback: None,
            menu: None,
            graph_open: true,
            graph_refreshed_at: None,
            generating: false,
            _task: None,
            _generate_task: None,
        }
    }
}

impl DiffView {
    /// Keeps the Changes graph fresh without reloading history on every poll.
    pub(super) fn refresh_graph(&mut self, force: bool, cx: &mut Context<Self>) {
        if !self.changes.graph_open {
            return;
        }
        let recent = self
            .changes
            .graph_refreshed_at
            .is_some_and(|at| at.elapsed() < GRAPH_REFRESH_INTERVAL);
        if recent && !force && self.history.is_some() {
            return;
        }
        self.changes.graph_refreshed_at = Some(Instant::now());
        self.refresh_history(false, cx);
    }

    pub(crate) fn set_review_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.review_focused != focused {
            self.review_focused = focused;
            cx.emit(DiffViewEvent::Changed);
            cx.notify();
        }
    }

    pub(super) fn open_commit_from_graph(&mut self, commit: GitCommit, cx: &mut Context<Self>) {
        let from_worktree = self.mode == GitPanelMode::Worktree || self.return_to_worktree;
        if self.mode != GitPanelMode::History {
            self.set_mode(GitPanelMode::History, cx);
        }
        self.return_to_worktree = from_worktree;
        self.select_commit(commit, cx);
    }

    /// Counts for the review toolbar: `7 files +149 −10`.
    pub(super) fn review_summary(&self) -> impl IntoElement {
        let (files, additions, deletions) = self.active_snapshot().map_or((0, 0, 0), |snapshot| {
            (
                snapshot.changes.len(),
                snapshot.additions,
                snapshot.deletions,
            )
        });
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px_1()
            .text_size(px(12.0))
            .child(
                div()
                    .text_color(colors().foreground)
                    .child(format!("{files} file{}", if files == 1 { "" } else { "s" })),
            )
            .when(additions > 0, |row| {
                row.child(
                    div()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(colors().diff_added)
                        .child(format!("+{additions}")),
                )
            })
            .when(deletions > 0, |row| {
                row.child(
                    div()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(colors().diff_deleted)
                        .child(format!("−{deletions}")),
                )
            })
    }

    /// Title bar of the review when it sits beside the terminal: what is
    /// shown, back to the full tab, and close.
    pub(super) fn review_pane_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.review_focused;
        let icon = if self.selected_commit.is_some() && self.mode == GitPanelMode::History {
            "chrome-icons/git-commit.svg"
        } else {
            "chrome-icons/diff-unified.svg"
        };
        div()
            .w_full()
            .h(px(36.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                svg()
                    .path(icon)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(colors().muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().foreground)
                    .child(self.review_title()),
            )
            .child(icon_button(
                "review-toggle-focus",
                "chrome-icons/maximize.svg",
                true,
                cx.listener(move |this, _, _, cx| this.set_review_focused(!focused, cx)),
            ))
            .child(icon_button(
                "review-close",
                "chrome-icons/close.svg",
                true,
                cx.listener(|this, _, window, cx| {
                    this.toggle_review_expanded(window, cx);
                }),
            ))
    }

    /// The whole Changes sidebar.
    pub(crate) fn changes_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let empty_message = self.empty_message();
        let selected_commit = self.selected_commit.clone();
        let worktree = self.mode == GitPanelMode::Worktree;
        let show_history = empty_message.is_none()
            && self.mode == GitPanelMode::History
            && selected_commit.is_none();
        let error = match self.mode {
            GitPanelMode::Branch => self.branch_error.clone().or_else(|| self.error.clone()),
            GitPanelMode::LatestTurn => self.turn_error.clone().or_else(|| self.error.clone()),
            GitPanelMode::History => self.commit_error.clone().or_else(|| self.error.clone()),
            GitPanelMode::Worktree => self.error.clone(),
        };

        div()
            .id("git-changes-panel")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.panel_header(cx))
            .when(!worktree, |panel| panel.child(self.scope_meta()))
            .when(worktree, |panel| panel.child(self.commit_box(window, cx)))
            .when(self.mode == GitPanelMode::Branch, |panel| {
                panel.child(self.branch_controls(cx))
            })
            .when_some(selected_commit, |panel, commit| {
                panel.child(self.commit_controls(&commit, cx))
            })
            .when_some(error, |panel, error| {
                panel.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .text_size(px(11.0))
                        .line_height(px(16.0))
                        .text_color(colors().danger)
                        .child(error),
                )
            })
            .map(|panel| {
                if show_history {
                    panel.child(self.history_list(cx))
                } else {
                    panel.child(self.compact_file_list(cx))
                }
            })
            .when(worktree || self.return_to_worktree, |panel| {
                panel.child(self.graph_section(cx))
            })
            .when_some(self.changes.menu, |panel, menu| {
                panel.child(self.panel_menu(menu, cx))
            })
            .into_any_element()
    }

    fn panel_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let branch = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.branch.clone())
            .or_else(|| self.history.as_ref().map(|history| history.branch.clone()));
        let title = match self.mode {
            GitPanelMode::Worktree => "Changes",
            mode => mode.label(),
        };
        div()
            .w_full()
            .h(px(40.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .pl_3()
            .pr_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors().foreground)
                    .child(title),
            )
            .when_some(branch, |header, branch| {
                header.child(
                    div()
                        .min_w(px(0.0))
                        .max_w(px(140.0))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_size(px(12.0))
                        .text_color(colors().muted)
                        .child(
                            svg()
                                .path("chrome-icons/git-branch.svg")
                                .size(px(13.0))
                                .flex_none(),
                        )
                        .child(div().min_w(px(0.0)).truncate().child(branch)),
                )
            })
            .child(
                icon_button(
                    "git-panel-more",
                    "chrome-icons/ellipsis.svg",
                    true,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_panel_menu(PanelMenu::More, cx);
                    }),
                )
                .when(self.changes.menu == Some(PanelMenu::More), |button| {
                    button.bg(colors().selection)
                }),
            )
    }

    /// What the non-working-tree scopes compare, below the header.
    fn scope_meta(&self) -> impl IntoElement {
        let loading = match self.mode {
            GitPanelMode::Branch => self.branch_refreshing,
            GitPanelMode::LatestTurn => self.turn_refreshing && !self.turn_settled,
            GitPanelMode::History => self.history_refreshing || self.commit_refreshing,
            GitPanelMode::Worktree => false,
        };
        let (_, meta) = self.header_meta(loading);
        div()
            .w_full()
            .flex_none()
            .px_3()
            .pb_2()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .border_b_1()
            .border_color(colors().border_subtle)
            .children(meta)
    }

    fn toggle_panel_menu(&mut self, menu: PanelMenu, cx: &mut Context<Self>) {
        self.changes.menu = (self.changes.menu != Some(menu)).then_some(menu);
        self.mode_menu_open = false;
        cx.notify();
    }

    fn has_worktree_changes(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.changes.is_empty())
    }

    fn commit_box(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.changes.focus.is_focused(window);
        let busy = self.changes.busy;
        let can_commit = busy.is_none()
            && self.has_worktree_changes()
            && !self.changes.message.trim().is_empty();
        let message = self.changes.message.clone();
        let lines: Vec<String> = message.split('\n').map(str::to_owned).collect();
        let line_count = lines.len();

        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .pb_3()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(self.commit_input(focused, message, lines, line_count, cx))
                    .child(
                        div().absolute().top(px(4.0)).right(px(4.0)).child(
                            icon_button(
                                "git-generate-message",
                                "chrome-icons/sparkles.svg",
                                busy.is_none(),
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.generate_commit_message(cx);
                                }),
                            )
                            .when(self.changes.generating, |button| button.opacity(0.5)),
                        ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(30.0))
                    .flex()
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .bg(if can_commit {
                        colors().accent
                    } else {
                        surface_tint(colors().elevated, colors().sidebar)
                    })
                    .text_color(if can_commit {
                        colors().background
                    } else {
                        colors().muted
                    })
                    .child(
                        div()
                            .id("git-commit")
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(6.0))
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .when(can_commit, |button| {
                                button.cursor_pointer().hover(|button| button.opacity(0.9))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_commit(CommitAction::Commit, cx)
                            }))
                            .child(svg().path("chrome-icons/check.svg").size(px(14.0)))
                            .child(busy.unwrap_or("Commit")),
                    )
                    .child(
                        div()
                            .w(px(1.0))
                            .h_full()
                            .bg(colors().border_subtle)
                            .opacity(0.6),
                    )
                    .child(
                        div()
                            .id("git-commit-menu")
                            .w(px(30.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|button| button.opacity(0.85))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_panel_menu(PanelMenu::Commit, cx);
                            }))
                            .child(svg().path("chrome-icons/chevron-down.svg").size(px(12.0))),
                    ),
            )
            .child(
                div()
                    .id("git-create-pr")
                    .w_full()
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .text_size(px(12.5))
                    .text_color(colors().muted)
                    .cursor_pointer()
                    .hover(|button| {
                        button
                            .bg(surface_tint(colors().hover, colors().sidebar))
                            .text_color(colors().foreground)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(DiffViewEvent::RunInTerminal {
                            title: "Pull request".to_owned(),
                            command: "gh pr create".to_owned(),
                        });
                        this.changes.menu = None;
                    }))
                    .child(
                        svg()
                            .path("chrome-icons/git-pull-request.svg")
                            .size(px(14.0)),
                    )
                    .child("Create PR"),
            )
            .when_some(self.changes.feedback.clone(), |panel, (text, failed)| {
                panel.child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(16.0))
                        .text_color(if failed {
                            colors().danger
                        } else {
                            colors().subtle
                        })
                        .child(text),
                )
            })
    }

    fn commit_input(
        &self,
        focused: bool,
        message: String,
        lines: Vec<String>,
        line_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("git-commit-message")
            .track_focus(&self.changes.focus)
            .on_key_down(cx.listener(Self::on_commit_key_down))
            .w_full()
            .min_h(px(34.0))
            .max_h(px(140.0))
            .overflow_y_scroll()
            .pl_2()
            .pr(px(32.0))
            .py(px(7.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(if focused {
                colors().accent
            } else {
                colors().border_subtle
            })
            .bg(surface_tint(colors().elevated, colors().sidebar))
            .cursor_text()
            .text_size(px(12.5))
            .line_height(px(18.0))
            .on_click(cx.listener(|this, _, window, cx| {
                this.changes.focus.focus(window);
                this.changes.menu = None;
                cx.notify();
            }))
            .map(|input| {
                if message.is_empty() && !focused {
                    input.child(
                        div()
                            .text_color(colors().subtle)
                            .child("Message (⌘↩ to commit)"),
                    )
                } else {
                    input.children(lines.into_iter().enumerate().map(|(index, line)| {
                        let caret = focused && index + 1 == line_count;
                        div()
                            .min_h(px(18.0))
                            .text_color(colors().foreground)
                            .child(if caret { format!("{line}▏") } else { line })
                    }))
                }
            })
    }

    /// Asks the user's agent CLI for a message describing the pending commit.
    fn generate_commit_message(&mut self, cx: &mut Context<Self>) {
        if self.changes.busy.is_some() || self.changes.generating {
            return;
        }
        if !self.has_worktree_changes() {
            self.changes.feedback = Some(("No hay cambios para describir.".into(), true));
            cx.notify();
            return;
        }
        self.changes.generating = true;
        self.changes.feedback = Some(("Generando el mensaje con tu agente…".into(), false));
        cx.notify();
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        let task = cx.background_spawn(async move {
            let context = port.commit_message_context(&root)?;
            crate::infrastructure::commit_message::generate_commit_message(&root, &context)
        });
        self.changes._generate_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.changes.generating = false;
                match result {
                    Ok(message) => {
                        this.changes.message = message;
                        this.changes.feedback = None;
                    }
                    Err(error) => {
                        this.changes.feedback = Some((format!("{error:#}").into(), true));
                    }
                }
                cx.notify();
            });
        }));
    }

    fn on_commit_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outcome = apply_text_key(
            &mut self.changes.message,
            event.keystroke.key.as_str(),
            event.keystroke.key_char.as_deref(),
            &event.keystroke.modifiers,
            true,
            || cx.read_from_clipboard().and_then(|item| item.text()),
        );
        match outcome {
            TextKeyOutcome::Edited => {
                if self.changes.message.chars().count() > MAX_COMMIT_MESSAGE_CHARS {
                    self.changes.message = self
                        .changes
                        .message
                        .chars()
                        .take(MAX_COMMIT_MESSAGE_CHARS)
                        .collect();
                }
                self.changes.feedback = None;
            }
            TextKeyOutcome::Submit => self.run_commit(CommitAction::Commit, cx),
            TextKeyOutcome::Cancel => window.blur(),
            TextKeyOutcome::NextField | TextKeyOutcome::PreviousField => {}
            TextKeyOutcome::Unhandled => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn run_commit(&mut self, action: CommitAction, cx: &mut Context<Self>) {
        self.changes.menu = None;
        if self.changes.busy.is_some() {
            return;
        }
        let amend = action == CommitAction::Amend;
        let message = self.changes.message.trim().to_owned();
        if message.is_empty() && !amend {
            self.changes.feedback = Some(("Escribe un mensaje para el commit.".into(), true));
            cx.notify();
            return;
        }
        if !amend && !self.has_worktree_changes() {
            self.changes.feedback = Some(("No hay cambios para hacer commit.".into(), true));
            cx.notify();
            return;
        }
        let push = action == CommitAction::CommitAndPush;
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        self.changes.busy = Some(if push {
            "Commit y push…"
        } else {
            "Commit…"
        });
        self.changes.feedback = None;
        cx.notify();
        let task = cx.background_spawn(async move {
            let sha = port.commit(&root, &message, GitCommitOptions { amend })?;
            if push {
                port.sync(&root, GitSyncOperation::Push)
                    .map_err(|error| anyhow::anyhow!("commit {sha} creado, pero {error:#}"))?;
            }
            anyhow::Ok(sha)
        });
        self.changes._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.changes.busy = None;
                match result {
                    Ok(sha) => {
                        this.changes.message.clear();
                        this.changes.feedback = Some((
                            if push {
                                format!("Commit {sha} creado y publicado.")
                            } else {
                                format!("Commit {sha} creado.")
                            }
                            .into(),
                            false,
                        ));
                    }
                    Err(error) => {
                        this.changes.feedback = Some((format!("{error:#}").into(), true));
                    }
                }
                this.after_repository_write(cx);
            });
        }));
    }

    fn run_sync(&mut self, operation: GitSyncOperation, cx: &mut Context<Self>) {
        self.changes.menu = None;
        if self.changes.busy.is_some() {
            return;
        }
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        self.changes.busy = Some(match operation {
            GitSyncOperation::Push => "Push…",
            GitSyncOperation::Pull => "Pull…",
            GitSyncOperation::Fetch => "Fetch…",
        });
        self.changes.feedback = None;
        cx.notify();
        let task = cx.background_spawn(async move { port.sync(&root, operation) });
        self.changes._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.changes.busy = None;
                this.changes.feedback = Some(match result {
                    Ok(()) => (format!("{} completado.", operation.label()).into(), false),
                    Err(error) => (format!("{error:#}").into(), true),
                });
                this.after_repository_write(cx);
            });
        }));
    }

    /// Moves files into (`stage`) or out of the index.
    pub(super) fn stage_paths(&mut self, paths: Vec<String>, stage: bool, cx: &mut Context<Self>) {
        if paths.is_empty() || self.changes.busy.is_some() {
            return;
        }
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        let task = cx.background_spawn(async move {
            if stage {
                port.stage(&root, &paths)
            } else {
                port.unstage(&root, &paths)
            }
        });
        self.changes._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.changes.feedback = Some((format!("{error:#}").into(), true));
                }
                this.after_repository_write(cx);
            });
        }));
    }

    fn after_repository_write(&mut self, cx: &mut Context<Self>) {
        self.changes.graph_refreshed_at = None;
        self.refresh_visible_sources(false, cx);
        cx.emit(DiffViewEvent::Changed);
        cx.notify();
    }

    fn graph_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.changes.graph_open;
        let history = self.history.clone();
        let graph = self.history_graph.clone();
        let count = history.as_ref().map_or(0, |history| history.commits.len());
        let head = history.as_ref().map(|history| history.head.clone());
        let graph_width = Self::history_graph_width(graph.as_slice());
        let selected = self
            .selected_commit
            .as_ref()
            .map(|commit| commit.sha.clone());

        div()
            .w_full()
            .flex_none()
            .when(open, |section| section.h(relative(0.42)).min_h(px(140.0)))
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .id("git-graph-toggle")
                    .h(px(30.0))
                    .w_full()
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|row| row.bg(surface_tint(colors().hover, colors().sidebar)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.changes.graph_open = !this.changes.graph_open;
                        if this.changes.graph_open {
                            this.refresh_graph(true, cx);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(colors().muted)
                            .child("GRAPH"),
                    )
                    .child(
                        svg()
                            .path(if open {
                                "chrome-icons/chevron-down.svg"
                            } else {
                                "chrome-icons/chevron-up.svg"
                            })
                            .size(px(12.0))
                            .text_color(colors().muted),
                    ),
            )
            .when(open && count == 0, |section| {
                section.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(12.0))
                        .text_color(colors().subtle)
                        .child(if self.history_refreshing {
                            "Loading history…"
                        } else {
                            "No commits yet."
                        }),
                )
            })
            .when(open && count > 0, |section| {
                section.child(
                    uniform_list(
                        "git-graph-rows",
                        count,
                        cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                            let Some(history) = history.as_ref() else {
                                return Vec::new();
                            };
                            range
                                .filter_map(|index| {
                                    let commit = history.commits.get(index)?;
                                    let is_head =
                                        head.as_deref().is_some_and(|head| commit.sha == head);
                                    let is_selected =
                                        selected.as_deref() == Some(commit.sha.as_str());
                                    let open_commit = commit.clone();
                                    Some(
                                        graph_row(
                                            commit,
                                            graph.get(index),
                                            graph_width,
                                            is_head,
                                            is_selected,
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_commit_from_graph(open_commit.clone(), cx);
                                        }))
                                        .into_any_element(),
                                    )
                                })
                                .collect()
                        }),
                    )
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full(),
                )
            })
    }

    fn panel_menu(&self, menu: PanelMenu, cx: &mut Context<Self>) -> impl IntoElement {
        let items: Vec<(SharedString, bool, MenuAction)> = match menu {
            PanelMenu::Commit => [
                CommitAction::Commit,
                CommitAction::CommitAndPush,
                CommitAction::Amend,
            ]
            .into_iter()
            .map(|action| (action.label().into(), false, MenuAction::Commit(action)))
            .collect(),
            PanelMenu::More => GitPanelMode::ALL
                .into_iter()
                .map(|mode| {
                    (
                        mode.label().into(),
                        mode == self.mode,
                        MenuAction::Mode(mode),
                    )
                })
                .chain([
                    (
                        "Fetch".into(),
                        false,
                        MenuAction::Sync(GitSyncOperation::Fetch),
                    ),
                    (
                        "Pull".into(),
                        false,
                        MenuAction::Sync(GitSyncOperation::Pull),
                    ),
                    (
                        "Push".into(),
                        false,
                        MenuAction::Sync(GitSyncOperation::Push),
                    ),
                    ("Refresh".into(), false, MenuAction::Refresh),
                ])
                .collect(),
        };
        let top = match menu {
            PanelMenu::More => 38.0,
            PanelMenu::Commit => {
                // Header, message box (grows with its lines), gap, button.
                let lines = self.changes.message.split('\n').count().max(1) as f32;
                let message_height = (lines * 18.0 + 16.0).clamp(34.0, 140.0);
                40.0 + message_height + 8.0 + 30.0 + 4.0
            }
        };
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("git-panel-menu-dismiss")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.changes.menu = None;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .absolute()
                    .top(px(top))
                    .right(px(10.0))
                    .w(px(210.0))
                    .occlude()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(popover_surface())
                    .shadow_lg()
                    .p_1()
                    .flex()
                    .flex_col()
                    .children(items.into_iter().enumerate().map(
                        |(index, (label, selected, action))| {
                            let separator =
                                matches!(action, MenuAction::Sync(GitSyncOperation::Fetch));
                            div()
                                .id(SharedString::from(format!("git-panel-menu-{index}")))
                                .when(separator, |row| {
                                    row.mt_1().border_t_1().border_color(colors().border_subtle)
                                })
                                .h(px(30.0))
                                .px_3()
                                .rounded(px(7.0))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .text_size(px(12.5))
                                .text_color(colors().foreground)
                                .when(selected, |row| {
                                    row.font_weight(gpui::FontWeight::MEDIUM)
                                        .bg(surface_tint(colors().selection, colors().sidebar))
                                })
                                .hover(|row| row.bg(surface_tint(colors().hover, colors().sidebar)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.changes.menu = None;
                                    match action {
                                        MenuAction::Commit(action) => this.run_commit(action, cx),
                                        MenuAction::Mode(mode) => {
                                            this.return_to_worktree = false;
                                            this.set_mode(mode, cx);
                                        }
                                        MenuAction::Sync(operation) => this.run_sync(operation, cx),
                                        MenuAction::Refresh => {
                                            this.changes.graph_refreshed_at = None;
                                            this.refresh_now(cx);
                                        }
                                    }
                                    cx.notify();
                                }))
                                .child(label)
                        },
                    )),
            )
    }
}

#[derive(Debug, Clone, Copy)]
enum MenuAction {
    Commit(CommitAction),
    Mode(GitPanelMode),
    Sync(GitSyncOperation),
    Refresh,
}

fn graph_row(
    commit: &GitCommit,
    graph: Option<&crate::ui::git_graph::GitGraphRow>,
    graph_width: f32,
    is_head: bool,
    is_selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let branch = commit
        .refs
        .iter()
        .find(|name| !name.contains('/'))
        .or_else(|| commit.refs.first())
        .cloned();
    div()
        .id(SharedString::from(format!("git-graph-{}", commit.sha)))
        .h(px(GRAPH_ROW_HEIGHT))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .pl_2()
        .pr_3()
        .gap(px(6.0))
        .overflow_hidden()
        .cursor_pointer()
        .when(is_selected, |row| {
            row.bg(surface_tint(colors().selection, colors().sidebar))
        })
        .hover(|row| row.bg(surface_tint(colors().hover, colors().sidebar)))
        .child(DiffView::graph_column(
            graph,
            graph_width,
            is_head,
            GRAPH_ROW_HEIGHT,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(12.5))
                .when(is_head, |subject| {
                    subject.font_weight(gpui::FontWeight::SEMIBOLD)
                })
                .text_color(colors().foreground)
                .child(commit.subject.clone()),
        )
        .child(
            div()
                .flex_none()
                .max_w(px(64.0))
                .truncate()
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .child(commit.author.clone()),
        )
        .when_some(branch, |row, branch| {
            row.child(
                div()
                    .flex_none()
                    .max_w(px(96.0))
                    .h(px(18.0))
                    .px(px(6.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .bg(colors().accent)
                    .text_color(colors().background)
                    .text_size(px(10.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(
                        svg()
                            .path("chrome-icons/git-branch.svg")
                            .size(px(10.0))
                            .flex_none(),
                    )
                    .child(div().min_w(px(0.0)).truncate().child(branch)),
            )
        })
}

fn icon_button(
    id: &'static str,
    icon: &'static str,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(26.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|button| button.bg(colors().hover))
                .on_click(on_click)
        })
        .child(svg().path(icon).size(px(14.0)).text_color(if enabled {
            colors().muted
        } else {
            colors().subtle
        }))
}
