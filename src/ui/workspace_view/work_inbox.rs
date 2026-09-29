//! Native Inbox coordination. Layout and detail actions follow MonoCode’s Inbox.

mod detail;
mod layout;
mod review;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, ClipboardItem, Context, KeyDownEvent, SharedString, Task, Timer, Window, div,
    prelude::*, px,
};
use uuid::Uuid;

use crate::domain::work_items::{
    PrAction, WorkCheck, WorkDetail, WorkDiff, WorkFilter, WorkItem, WorkKind, WorkSource,
    WorkStatus,
};
use crate::infrastructure::work_items::{self, InboxProject, WorkItemsPage, WorkQuery};
use crate::ui::text_edit::{TextKeyOutcome, apply_text_key};
use crate::ui::theme::colors;

use super::navigation::section_button;
use super::{WorkspaceSection, WorkspaceView};

#[derive(Clone, Copy, PartialEq, Eq)]
enum InboxMenu {
    Filters,
    Connections,
    Projects,
    Agents,
    PrActions,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum DetailTab {
    #[default]
    Summary,
    Code,
    Checks,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CodeMode {
    #[default]
    Hunks,
    FullFile,
}

struct LoadState<T> {
    data: Option<T>,
    error: Option<String>,
    loading: bool,
    revision: u64,
    _task: Option<Task<()>>,
}

impl<T> Default for LoadState<T> {
    fn default() -> Self {
        Self {
            data: None,
            error: None,
            loading: false,
            revision: 0,
            _task: None,
        }
    }
}

impl<T> LoadState<T> {
    fn invalidate(&mut self) {
        self.data = None;
        self.error = None;
        self.loading = false;
        self.revision += 1;
        self._task = None;
    }

    fn apply(&mut self, revision: u64, result: anyhow::Result<T>) {
        if revision != self.revision {
            return;
        }
        self.loading = false;
        match result {
            Ok(data) => {
                self.data = Some(data);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }
}

#[derive(Default)]
struct ItemState {
    updated_at: u64,
    check_logs: HashMap<String, LoadState<String>>,
    expanded_checks: HashSet<String>,
    summary: LoadState<WorkDetail>,
    diff: LoadState<WorkDiff>,
    checks: LoadState<Vec<WorkCheck>>,
    review: Option<gpui::Entity<crate::ui::diff_view::DiffView>>,
    _review_subscription: Option<gpui::Subscription>,
    code_mode: CodeMode,
    checks_attention_only: bool,
    draft: String,
    reply: Option<(String, String)>,
    posting: bool,
    post_error: Option<String>,
    mutation_busy: bool,
    mutation_error: Option<String>,
    _post_task: Option<Task<()>>,
    _mutation_task: Option<Task<()>>,
}

#[derive(Clone)]
struct PrConfirmation {
    url: String,
    action: PrAction,
    head_oid: String,
}

#[derive(Default)]
struct SourceFeed {
    items: Vec<WorkItem>,
    connected: Option<bool>,
    error: Option<String>,
    loading: bool,
    truncated: bool,
    fetched: Option<Instant>,
    query: Option<WorkQuery>,
    generation: u64,
    _task: Option<Task<()>>,
}

impl SourceFeed {
    fn restore(&mut self, generation: u64, page: WorkItemsPage) -> bool {
        if generation != self.generation || self.fetched.is_some() {
            return false;
        }
        self.apply_page(page);
        // A disk snapshot is usable immediately but still needs a live refresh.
        // Keep loading set and do not start the refresh cooldown yet.
        true
    }

    fn accept(&mut self, generation: u64, result: anyhow::Result<WorkItemsPage>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.apply(result);
        true
    }

    fn apply(&mut self, result: anyhow::Result<WorkItemsPage>) {
        self.loading = false;
        self.fetched = Some(Instant::now());
        match result {
            Ok(page) => self.apply_page(page),
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    fn apply_page(&mut self, mut page: WorkItemsPage) {
        page.items.extend(
            self.items
                .iter()
                .filter(|item| {
                    page.failed_scopes.iter().any(|(repo, kind)| {
                        item.repository.eq_ignore_ascii_case(repo) && item.kind == *kind
                    })
                })
                .cloned(),
        );
        self.items = page.items;
        self.items
            .sort_by_key(|item| std::cmp::Reverse(item.updated_at));
        self.connected = Some(page.connected);
        self.truncated = page.truncated;
        self.error = page.warning;
    }
}

#[derive(Default)]
pub(super) struct WorkInbox {
    pub activity: bool,
    github: SourceFeed,
    linear: SourceFeed,
    filter: WorkFilter,
    search_editing: bool,
    selected: Option<String>,
    target_project: Option<Uuid>,
    agent: usize,
    action_error: Option<String>,
    menu: Option<InboxMenu>,
    menu_position: (f32, f32),
    detail_tab: DetailTab,
    details: HashMap<String, ItemState>,
    epoch: u64,
    discussion_open: bool,
    discussion_panes: HashMap<String, Uuid>,
    composer_open: bool,
    composer_note: String,
    composer_editing: bool,
    comment_editing: bool,
    confirmation: Option<PrConfirmation>,
    connecting: bool,
    connection_error: Option<String>,
    _connection_task: Option<Task<()>>,
    _poll_task: Option<Task<()>>,
}

impl WorkInbox {
    fn feed(&self, source: WorkSource) -> &SourceFeed {
        match source {
            WorkSource::GitHub => &self.github,
            WorkSource::Linear => &self.linear,
        }
    }

    fn feed_mut(&mut self, source: WorkSource) -> &mut SourceFeed {
        match source {
            WorkSource::GitHub => &mut self.github,
            WorkSource::Linear => &mut self.linear,
        }
    }
}

const AGENTS: [(&str, &str); 3] = [
    ("Claude", "claude"),
    ("Codex", "codex"),
    ("Gemini", "gemini"),
];

impl WorkspaceView {
    pub(super) fn scope_inbox_to_active_project(&mut self) {
        self.work_inbox.filter.project = if self.settings.inbox.source == WorkSource::GitHub {
            self.snapshot.selected_project_id.filter(|id| {
                self.snapshot
                    .projects
                    .iter()
                    .any(|project| project.id == *id)
            })
        } else {
            // Linear teams/projects are remote groups, not local repository IDs.
            None
        };
        self.settings.inbox.hidden_projects.clear();
    }

    fn inbox_project_selected(&self, id: Uuid) -> bool {
        self.work_inbox.filter.project.map_or_else(
            || !self.settings.inbox.hidden_projects.contains(&id),
            |project| project == id,
        )
    }

    fn toggle_inbox_project(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if let Some(selected) = self.work_inbox.filter.project.take() {
            self.settings.inbox.hidden_projects = self
                .snapshot
                .projects
                .iter()
                .filter(|project| project.id != selected)
                .map(|project| project.id)
                .collect();
        }
        crate::domain::work_items::toggle_value(&mut self.settings.inbox.hidden_projects, id);
        self.inbox_filters_changed(cx);
    }

    pub(super) fn sync_inbox_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self.work_inbox.selected.as_deref();
        let mut visible = self
            .work_inbox
            .feed(self.settings.inbox.source)
            .items
            .iter()
            .filter(|item| self.work_inbox.filter.matches(item, &self.settings.inbox));
        if visible
            .clone()
            .any(|item| Some(item.url.as_str()) == selected)
        {
            return;
        }
        if let Some(first) = visible.next().map(|item| item.url.clone()) {
            self.select_work_item(&first, cx);
        } else {
            self.work_inbox.selected = None;
            self.sync_terminal_surface_visibility(cx);
        }
    }

    fn inbox_filters_changed(&mut self, cx: &mut Context<Self>) {
        self.ensure_inbox_loaded(cx);
        self.sync_inbox_selection(cx);
        self.persist_settings(cx);
    }
    fn inbox_query(&self) -> WorkQuery {
        WorkQuery {
            projects: self
                .snapshot
                .projects
                .iter()
                .filter_map(|project| {
                    Some(InboxProject {
                        id: project.id,
                        root: project.directory()?.into(),
                    })
                })
                .collect(),
            assigned_to_me: self.settings.inbox.assigned_to_me,
            status: self.settings.inbox.query_status(),
        }
    }

    pub(super) fn ensure_inbox_loaded(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        self.refresh_work_source(WorkSource::GitHub, false, cx);
        self.refresh_work_source(WorkSource::Linear, false, cx);
        self.inbox_source_updated(self.settings.inbox.source, false, cx);
    }

    fn inbox_source_updated(
        &mut self,
        source: WorkSource,
        refresh_detail: bool,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_section == WorkspaceSection::Inbox
            && !self.work_inbox.activity
            && source == self.settings.inbox.source
        {
            self.sync_inbox_selection(cx);
            self.load_inbox_detail(refresh_detail, cx);
        }
        cx.notify();
    }

    pub(super) fn start_inbox_poll(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        // Start while the workspace opens, before the user visits Inbox.
        self.ensure_inbox_loaded(cx);
        self.work_inbox._poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(60)).await;
                if this
                    .update(cx, |this, cx| {
                        if this.workspace_section == WorkspaceSection::Inbox {
                            this.ensure_inbox_loaded(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn refresh_work_source(&mut self, source: WorkSource, force: bool, cx: &mut Context<Self>) {
        let query = self.inbox_query();
        let feed = self.work_inbox.feed_mut(source);
        if feed.query.as_ref() == Some(&query)
            && (feed.loading
                || (!force
                    && feed
                        .fetched
                        .is_some_and(|at| at.elapsed() < Duration::from_secs(60))))
        {
            return;
        }
        feed.generation += 1;
        let generation = feed.generation;
        if feed.query.as_ref() != Some(&query) {
            feed.items.clear();
            feed.error = None;
            feed.truncated = false;
            feed.fetched = None;
        }
        let restore_cache = feed.fetched.is_none();
        feed.query = Some(query.clone());
        feed.loading = true;
        feed._task = Some(cx.spawn(async move |this, cx| {
            let cache_query = query.clone();
            let (cache, cached) = cx
                .background_spawn(async move {
                    let cache = work_items::ListCache::new(source, &cache_query);
                    let cached = if restore_cache {
                        cache.as_ref().and_then(work_items::ListCache::load)
                    } else {
                        None
                    };
                    (cache, cached)
                })
                .await;
            if let Some(page) = cached {
                let _ = this.update(cx, |this, cx| {
                    if this.work_inbox.feed_mut(source).restore(generation, page) {
                        this.inbox_source_updated(source, false, cx);
                    }
                });
            }
            let result = cx
                .background_spawn(async move {
                    let result = work_items::list(source, &query);
                    if let (Some(cache), Ok(page)) = (cache, &result) {
                        cache.save(page);
                    }
                    result
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let feed = this.work_inbox.feed_mut(source);
                if feed.accept(generation, result) {
                    this.inbox_source_updated(source, true, cx);
                }
            });
        }));
        cx.notify();
    }

    fn select_work_source(&mut self, source: WorkSource, cx: &mut Context<Self>) {
        self.work_inbox.activity = false;
        self.settings.inbox.source = source;
        self.work_inbox.filter = WorkFilter::default();
        self.scope_inbox_to_active_project();
        self.work_inbox.selected = None;
        self.work_inbox.search_editing = false;
        self.work_inbox.action_error = None;
        self.work_inbox.menu = None;
        self.work_inbox.composer_open = false;
        self.work_inbox.confirmation = None;
        self.ensure_inbox_loaded(cx);
        self.sync_inbox_selection(cx);
        self.persist_settings(cx);
    }

    fn select_work_item(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(item) = self
            .work_inbox
            .feed(self.settings.inbox.source)
            .items
            .iter()
            .find(|item| item.url == url)
        else {
            return;
        };
        self.settings.inbox.mark_seen(item);
        let changed = self.work_inbox.selected.as_deref() != Some(url);
        if changed {
            self.work_inbox.target_project = item.project_id.or(self.snapshot.selected_project_id);
            let inbox = &mut self.work_inbox;
            inbox.selected = Some(url.into());
            inbox.search_editing = false;
            inbox.action_error = None;
            inbox.detail_tab = DetailTab::Summary;
            inbox.composer_open = false;
            inbox.composer_editing = false;
            inbox.composer_note.clear();
            inbox.menu = None;
            inbox.comment_editing = false;
            inbox.confirmation = None;
        }
        self.load_inbox_detail(false, cx);
        if changed {
            self.sync_terminal_surface_visibility(cx);
        }
        self.persist_settings(cx);
    }

    fn selected_work_item(&self) -> Option<&WorkItem> {
        self.work_inbox
            .feed(self.settings.inbox.source)
            .items
            .iter()
            .find(|item| Some(&item.url) == self.work_inbox.selected.as_ref())
    }

    fn open_inbox_composer(&mut self, cx: &mut Context<Self>) {
        self.work_inbox.composer_open = true;
        self.work_inbox.discussion_open = false;
        self.work_inbox.menu = None;
        self.work_inbox.search_editing = false;
        self.work_inbox.comment_editing = false;
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
    }

    fn start_work_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut item) = self.selected_work_item().cloned() else {
            return;
        };
        if let Some(detail) = self
            .work_inbox
            .details
            .get(&item.url)
            .and_then(|state| state.summary.data.as_ref())
        {
            item.body = detail.body.clone();
        }
        if !self.work_inbox.composer_note.trim().is_empty() {
            item.body.push_str("\n\nAdditional user instructions:\n");
            item.body.push_str(&self.work_inbox.composer_note);
        }
        let Some(project) = self.work_inbox.target_project else {
            self.work_inbox.action_error =
                Some("Choose a project with a folder to start the task.".into());
            cx.notify();
            return;
        };
        let (_, agent) = AGENTS[self.work_inbox.agent];
        if !self
            .snapshot
            .projects
            .iter()
            .any(|candidate| candidate.id == project && candidate.directory().is_some())
        {
            self.work_inbox.action_error =
                Some("The project is no longer available. Choose another project.".into());
            cx.notify();
            return;
        }
        let (command, prompt_path) = match work_items::prepare_launch(agent, &item) {
            Ok(launch) => launch,
            Err(error) => {
                self.work_inbox.action_error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        match self.run_in_new_tab(
            project,
            &format!("{} · {}", item.reference, item.title),
            &command,
            false,
            cx,
        ) {
            Ok((pane, true)) => {
                let live: HashSet<_> = self
                    .snapshot
                    .terminal_sessions()
                    .map(|session| session.id)
                    .collect();
                self.settings.inbox.linked_sessions.retain(|_, panes| {
                    panes.retain(|pane| live.contains(pane));
                    !panes.is_empty()
                });
                self.settings
                    .inbox
                    .linked_sessions
                    .entry(item.url)
                    .or_default()
                    .push(pane);
                self.work_inbox.composer_open = false;
                self.work_inbox.composer_editing = false;
                self.persist_settings(cx);
                self.work_inbox.action_error = None;
                self.open_pane(pane, window, cx);
            }
            Ok((_, false)) => {
                let _ = std::fs::remove_file(&prompt_path);
                self.work_inbox.action_error = Some(
                    "The tab opened, but the terminal could not start the agent. Check the terminal and try again."
                        .into(),
                );
            }
            Err(message) => {
                let _ = std::fs::remove_file(&prompt_path);
                self.work_inbox.action_error = Some(message.into());
            }
        }
        cx.notify();
    }

    pub(super) fn work_inbox_unread_count(&self) -> usize {
        self.work_inbox
            .github
            .items
            .iter()
            .chain(&self.work_inbox.linear.items)
            .filter(|item| item.unread(&self.settings.inbox.seen))
            .count()
    }

    pub(super) fn handle_inbox_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspace_section != WorkspaceSection::Inbox || self.work_inbox.activity {
            return false;
        }
        if matches!(event.keystroke.key.as_str(), "escape" | "esc")
            && self.work_inbox.menu.take().is_some()
        {
            cx.notify();
            return true;
        }
        let search = self.work_inbox.search_editing;
        let composer = self.work_inbox.composer_editing && self.work_inbox.composer_open;
        let comment = self.work_inbox.comment_editing;
        let buffer = if search {
            &mut self.work_inbox.filter.query
        } else if composer {
            &mut self.work_inbox.composer_note
        } else if comment {
            let Some(url) = self.work_inbox.selected.clone() else {
                return false;
            };
            let state = self.work_inbox.details.entry(url).or_default();
            if state.posting {
                return false;
            }
            &mut state.draft
        } else {
            return false;
        };
        let outcome = apply_text_key(
            buffer,
            &event.keystroke.key,
            event.keystroke.key_char.as_deref(),
            &event.keystroke.modifiers,
            !search,
            || cx.read_from_clipboard().and_then(|item| item.text()),
        );
        match outcome {
            TextKeyOutcome::Edited => {
                if search {
                    self.sync_inbox_selection(cx);
                    self.work_inbox.search_editing = true;
                }
            }
            TextKeyOutcome::Submit if comment => self.post_inbox_comment(cx),
            TextKeyOutcome::Submit if composer => self.start_work_item(window, cx),
            TextKeyOutcome::Submit
            | TextKeyOutcome::Cancel
            | TextKeyOutcome::NextField
            | TextKeyOutcome::PreviousField => {
                self.work_inbox.search_editing = false;
                self.work_inbox.composer_editing = false;
                self.work_inbox.comment_editing = false;
            }
            TextKeyOutcome::Unhandled => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn inbox_connection_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let connected = self
            .work_inbox
            .linear
            .connected
            .unwrap_or_else(|| !cfg!(test) && work_items::linear_connected());
        div()
            .px_4()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(div().text_size(px(12.0)).text_color(colors().muted).child(
                        if connected {
                            "Linear connected"
                        } else {
                            "Linear · Personal API key"
                        },
                    ))
                    .child(
                        section_button(
                            "linear-connect",
                            if self.work_inbox.connecting {
                                "Verifying…"
                            } else if connected {
                                "Change key from clipboard"
                            } else {
                                "Connect Linear from clipboard"
                            },
                            false,
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.connect_inbox_linear(false, cx)),
                        ),
                    )
                    .when(connected, |row| {
                        row.child(
                            section_button("linear-disconnect", "Disconnect", false).on_click(
                                cx.listener(|this, _, _, cx| this.connect_inbox_linear(true, cx)),
                            ),
                        )
                    })
                    .child(
                        section_button("linear-key-help", "Get API key ↗", false)
                            .on_click(|_, _, cx| cx.open_url("https://linear.app/settings/api")),
                    ),
            )
            .when_some(self.work_inbox.connection_error.as_ref(), |row, error| {
                row.child(message(error, true))
            })
            .into_any_element()
    }

    fn connect_inbox_linear(&mut self, disconnect: bool, cx: &mut Context<Self>) {
        if self.work_inbox.connecting {
            return;
        }
        if self
            .work_inbox
            .details
            .values()
            .any(|state| state.posting || state.mutation_busy)
        {
            self.work_inbox.connection_error =
                Some("Wait for the Inbox action to finish before changing the connection.".into());
            cx.notify();
            return;
        }
        let token = if disconnect {
            String::new()
        } else {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()
        };
        if !disconnect && token.trim().is_empty() {
            self.work_inbox.connection_error =
                Some("Copy your Linear personal API key and reconnect.".into());
            cx.notify();
            return;
        }
        self.work_inbox.connecting = true;
        self.work_inbox.connection_error = None;
        self.work_inbox._connection_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if disconnect {
                        work_items::disconnect_linear()
                    } else {
                        work_items::connect_linear(&token)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.work_inbox.connecting = false;
                match result {
                    Ok(()) => {
                        this.work_inbox.epoch += 1;
                        this.work_inbox.details.clear();
                        this.work_inbox.confirmation = None;
                        this.work_inbox.comment_editing = false;
                        let generation = this.work_inbox.linear.generation + 1;
                        this.work_inbox.linear = SourceFeed {
                            connected: Some(!disconnect),
                            generation,
                            ..Default::default()
                        };
                        if this.settings.inbox.source == WorkSource::Linear {
                            this.work_inbox.selected = None;
                        }
                        if !disconnect {
                            this.refresh_work_source(WorkSource::Linear, true, cx);
                        }
                    }
                    Err(error) => this.work_inbox.connection_error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

fn message(text: &str, error: bool) -> gpui::Div {
    div()
        .px_2()
        .py_1()
        .text_size(px(12.0))
        .text_color(if error {
            colors().warning
        } else {
            colors().muted
        })
        .child(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::super::tests::open_recording_workspace;
    use super::*;
    use crate::domain::work_items::fixture;

    #[gpui::test]
    fn inbox_reuses_the_workspace_diff_and_prepares_review_comments_without_execution(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::ui::diff_view::DiffViewEvent;
        let (root, _, inputs, window) = open_recording_workspace(cx, "inbox-shared-diff");
        let item = fixture();
        let review = window
            .update(cx, |view, _, cx| {
                view.work_inbox.github.items.push(item.clone());
                view.work_inbox.selected = Some(item.url.clone());
                view.work_inbox
                    .details
                    .entry(item.url.clone())
                    .or_default()
                    .diff
                    .data = Some(WorkDiff::default());
                view.sync_inbox_review(&item.url, cx);
                let review = view.work_inbox.details[&item.url].review.clone().unwrap();
                assert_ne!(review.entity_id(), view.diff_view.entity_id());
                view.sync_inbox_review(&item.url, cx);
                assert_eq!(
                    review.entity_id(),
                    view.work_inbox.details[&item.url]
                        .review
                        .as_ref()
                        .unwrap()
                        .entity_id()
                );
                view.set_inbox_code_mode(CodeMode::FullFile, cx);
                view.work_inbox.composer_note = "Existing instructions".into();
                review
            })
            .unwrap();
        let writes = inputs.lock().unwrap().len();
        review.update(cx, |_, cx| {
            cx.emit(DiffViewEvent::PreferencesChanged {
                split: true,
                wrap: true,
            });
            cx.emit(DiffViewEvent::SendReview {
                prompt: "main.rs:2: Please simplify this function.".into(),
                delivery_id: Uuid::new_v4(),
            });
        });
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(view.settings.diff_split && view.settings.diff_wrap);
                assert!(view.work_inbox.composer_open);
                assert!(
                    view.work_inbox
                        .composer_note
                        .starts_with("Existing instructions\n\n")
                );
                assert!(view.work_inbox.composer_note.contains(&item.url));
                assert!(
                    view.work_inbox
                        .composer_note
                        .contains("Please simplify this function.")
                );
                assert_eq!(inputs.lock().unwrap().len(), writes);
                view.work_inbox.selected = None;
                review.update(cx, |_, cx| {
                    cx.emit(DiffViewEvent::SendReview {
                        prompt: "Obsolete selection".into(),
                        delivery_id: Uuid::new_v4(),
                    });
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, window, _| {
                assert!(!view.work_inbox.composer_note.contains("Obsolete selection"));
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn work_inbox_cached_list_stays_visible_during_refresh_and_network_failure() {
        let mut feed = SourceFeed {
            generation: 1,
            loading: true,
            ..Default::default()
        };
        let page = || WorkItemsPage {
            items: vec![fixture()],
            connected: true,
            ..Default::default()
        };
        assert!(!feed.restore(0, page()));
        assert!(feed.items.is_empty());
        assert!(feed.restore(1, page()));
        assert_eq!(feed.items.len(), 1);
        assert!(feed.loading);
        assert!(feed.fetched.is_none());
        assert!(feed.accept(1, Err(anyhow::anyhow!("offline"))));
        assert_eq!(feed.items.len(), 1);
        assert!(!feed.loading);
        assert!(feed.error.is_some());
        assert!(!feed.restore(1, WorkItemsPage::default()));
        assert_eq!(feed.items.len(), 1);
        // Live data replaces the snapshot, including a now-empty inbox.
        assert!(feed.accept(
            1,
            Ok(WorkItemsPage {
                connected: true,
                ..Default::default()
            })
        ));
        assert!(feed.items.is_empty());
        assert!(feed.error.is_none());
    }

    #[gpui::test]
    fn entering_inbox_scopes_the_active_project_and_reselects_visible_open_work(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, snapshot, _, window) = open_recording_workspace(cx, "inbox-project-scope");
        let first_project = snapshot.selected_project_id.unwrap();
        let other_root = root.join("other");
        std::fs::create_dir_all(&other_root).unwrap();
        window
            .update(cx, |view, window, cx| {
                let second_project = view.snapshot.add_project(&other_root);
                view.snapshot.select_project(first_project);
                let mut first = fixture();
                first.project_id = Some(first_project);
                let mut second = first.clone();
                second.url = "https://github.com/other/app/issues/1".into();
                second.project_id = Some(second_project);
                let mut closed = first.clone();
                closed.url = "https://github.com/demo/app/issues/43".into();
                closed.status = WorkStatus::Closed;
                let mut merged = closed.clone();
                merged.url = "https://github.com/demo/app/pull/44".into();
                merged.kind = WorkKind::PullRequest;
                merged.status = WorkStatus::Merged;
                view.work_inbox.github.items = vec![
                    closed.clone(),
                    second.clone(),
                    first.clone(),
                    merged.clone(),
                ];
                view.work_inbox.selected = Some(second.url.clone());
                view.settings.inbox.hidden_projects = vec![first_project];
                view.select_section(WorkspaceSection::Inbox, window, cx);
                assert_eq!(view.work_inbox.filter.project, Some(first_project));
                assert_eq!(
                    view.work_inbox.selected.as_deref(),
                    Some(first.url.as_str())
                );
                assert!(
                    !view
                        .work_inbox
                        .filter
                        .matches(&second, &view.settings.inbox)
                );
                assert!(
                    !view
                        .work_inbox
                        .filter
                        .matches(&closed, &view.settings.inbox)
                );
                assert!(
                    !view
                        .work_inbox
                        .filter
                        .matches(&merged, &view.settings.inbox)
                );
                assert_eq!(view.inbox_query().status, Some(WorkStatus::Open));
                assert!(view.inbox_project_selected(first_project));
                assert!(!view.inbox_project_selected(second_project));

                // A manual broader selection survives refreshes and clicks while already in Inbox.
                view.toggle_inbox_project(second_project, cx);
                assert!(
                    view.work_inbox
                        .filter
                        .matches(&second, &view.settings.inbox)
                );
                assert!(view.work_inbox.filter.matches(&first, &view.settings.inbox));
                view.select_section(WorkspaceSection::Inbox, window, cx);
                assert!(view.work_inbox.filter.project.is_none());

                view.select_section(WorkspaceSection::Workspace, window, cx);
                view.snapshot.select_project(second_project);
                view.select_section(WorkspaceSection::Inbox, window, cx);
                assert_eq!(view.work_inbox.filter.project, Some(second_project));
                assert_eq!(
                    view.work_inbox.selected.as_deref(),
                    Some(second.url.as_str())
                );

                view.select_work_source(WorkSource::Linear, cx);
                assert!(view.work_inbox.filter.project.is_none());
                view.select_work_source(WorkSource::GitHub, cx);
                assert_eq!(view.work_inbox.filter.project, Some(second_project));
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn work_inbox_prefetch_does_not_select_or_mark_tasks_read_until_visible(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, _, _, window) = open_recording_workspace(cx, "inbox-prefetch");
        window
            .update(cx, |view, window, cx| {
                let item = fixture();
                view.work_inbox.github.apply(Ok(WorkItemsPage {
                    items: vec![item.clone()],
                    connected: true,
                    ..Default::default()
                }));
                view.inbox_source_updated(WorkSource::GitHub, true, cx);
                assert!(view.work_inbox.selected.is_none());
                assert!(item.unread(&view.settings.inbox.seen));
                assert!(view.work_inbox.details.is_empty());
                view.workspace_section = WorkspaceSection::Inbox;
                view.work_inbox.activity = true;
                view.inbox_source_updated(WorkSource::GitHub, false, cx);
                assert!(view.work_inbox.selected.is_none());
                assert!(item.unread(&view.settings.inbox.seen));
                view.work_inbox.activity = false;
                view.inbox_source_updated(WorkSource::GitHub, false, cx);
                assert_eq!(view.work_inbox.selected.as_deref(), Some(item.url.as_str()));
                assert!(!item.unread(&view.settings.inbox.seen));
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn work_inbox_sources_fail_independently_and_ignore_stale_results() {
        let mut inbox = WorkInbox::default();
        inbox.github.apply(Ok(WorkItemsPage {
            items: vec![fixture()],
            connected: true,
            ..Default::default()
        }));
        inbox
            .linear
            .apply(Err(anyhow::anyhow!("Linear unavailable")));
        assert_eq!(inbox.github.items.len(), 1);
        assert!(inbox.github.error.is_none());
        inbox
            .github
            .apply(Err(anyhow::anyhow!("Network unavailable")));
        assert_eq!(inbox.github.items.len(), 1);
        assert!(inbox.github.error.is_some());
        inbox.github.apply(Ok(WorkItemsPage {
            connected: true,
            warning: Some("PR query unavailable".into()),
            failed_scopes: vec![("demo/app".into(), WorkKind::Issue)],
            ..Default::default()
        }));
        assert_eq!(inbox.github.items.len(), 1);
        assert!(inbox.github.error.is_some());
        inbox.github.generation = 2;
        assert!(!inbox.github.accept(1, Ok(WorkItemsPage::default())));
        assert_eq!(inbox.github.items.len(), 1);
        assert!(inbox.github.accept(2, Ok(WorkItemsPage::default())));
        assert!(inbox.github.items.is_empty());
        assert!(inbox.github.error.is_none());
    }

    #[gpui::test]
    fn work_inbox_prepares_a_draft_then_starts_a_linked_session_in_its_project(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, snapshot, inputs, window) = open_recording_workspace(cx, "work-inbox-start");
        let original = snapshot.selected_project_id.unwrap();
        let other_root = root.join("other");
        std::fs::create_dir_all(&other_root).unwrap();
        window
            .update(cx, |view, window, cx| {
                let other = view.snapshot.add_project(&other_root);
                view.snapshot.select_project(original);
                let mut item = fixture();
                item.project_id = Some(other);
                view.work_inbox.github.items.push(item.clone());
                view.select_section(WorkspaceSection::Inbox, window, cx);
                view.select_work_item(&item.url, cx);
                assert_eq!(view.work_inbox.target_project, Some(other));
                assert!(!item.unread(&view.settings.inbox.seen));
                let before = view.snapshot.terminal_sessions().count();
                let writes = inputs.lock().unwrap().len();
                view.open_inbox_composer(cx);
                assert_eq!(view.snapshot.terminal_sessions().count(), before);
                assert_eq!(inputs.lock().unwrap().len(), writes);
                view.work_inbox.composer_note = "Conserva los cambios del usuario".into();
                view.start_work_item(window, cx);
                assert_eq!(view.snapshot.selected_project_id, Some(other));
                assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
                assert_eq!(view.snapshot.terminal_sessions().count(), before + 1);
                let pane = view.snapshot.selected_session().unwrap().id;
                let sent = inputs.lock().unwrap();
                let (target, bytes) = sent.last().unwrap();
                assert_eq!(*target, pane);
                let command = String::from_utf8(bytes.clone()).unwrap();
                assert!(command.starts_with("/bin/zsh -lc "));
                assert!(command.ends_with('\r'));
                assert!(!command.contains('\n'));
                // The recording port does not run commands: clean its prompt file.
                let name = command
                    .split("vibra-inbox-")
                    .nth(1)
                    .unwrap()
                    .split(".txt")
                    .next()
                    .unwrap();
                let path = std::env::temp_dir().join(format!("vibra-inbox-{name}.txt"));
                assert!(
                    std::fs::read_to_string(&path)
                        .unwrap()
                        .contains("Conserva los cambios del usuario")
                );
                std::fs::remove_file(path).unwrap();
                drop(sent);
                assert_eq!(view.settings.inbox.linked_sessions[&item.url], vec![pane]);
                let saved = serde_json::to_string(&view.settings.inbox).unwrap();
                let restored: crate::domain::work_items::InboxPreferences =
                    serde_json::from_str(&saved).unwrap();
                assert_eq!(restored.linked_sessions[&item.url], vec![pane]);
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn details_discard_stale_loads_and_preserve_data_on_refresh_error() {
        let mut load = LoadState::<String> {
            revision: 1,
            loading: true,
            ..Default::default()
        };
        load.invalidate();
        load.apply(1, Ok("old response".into()));
        assert!(load.data.is_none());
        load.apply(2, Ok("current response".into()));
        load.revision = 3;
        load.apply(3, Err(anyhow::anyhow!("offline")));
        assert_eq!(load.data.as_deref(), Some("current response"));
        assert!(load.error.is_some());
    }

    #[gpui::test]
    fn inbox_discussion_keeps_workspace_selection_and_isolates_terminal_visibility(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, snapshot, inputs, window) = open_recording_workspace(cx, "inbox-discussion");
        window
            .update(cx, |view, window, cx| {
                let mut item = fixture();
                item.project_id = snapshot.selected_project_id;
                view.work_inbox.github.items.push(item.clone());
                view.select_section(WorkspaceSection::Inbox, window, cx);
                view.select_work_item(&item.url, cx);
                let selected = view.snapshot.selected_session().unwrap().id;
                view.open_inbox_discussion(window, cx);
                let pane = view.visible_inbox_terminal().unwrap();
                assert_ne!(pane, selected);
                assert_eq!(view.snapshot.selected_session().unwrap().id, selected);
                assert_eq!(view.workspace_section, WorkspaceSection::Inbox);
                assert_eq!(view.visible_terminal_ids(cx), HashSet::from([pane]));
                let sent = inputs.lock().unwrap();
                let command = String::from_utf8(sent.last().unwrap().1.clone()).unwrap();
                let name = command
                    .split("vibra-inbox-")
                    .nth(1)
                    .unwrap()
                    .split(".txt")
                    .next()
                    .unwrap();
                let path = std::env::temp_dir().join(format!("vibra-inbox-{name}.txt"));
                let prompt = std::fs::read_to_string(&path).unwrap();
                assert!(prompt.contains(&item.url));
                assert!(prompt.contains("read-only remote queries"));
                std::fs::remove_file(path).unwrap();
                let writes = sent.len();
                drop(sent);
                view.open_inbox_discussion(window, cx);
                assert_eq!(inputs.lock().unwrap().len(), writes);
                view.work_inbox.discussion_open = false;
                assert!(view.visible_terminal_ids(cx).is_empty());
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn task_drafts_and_pr_confirmation_stay_with_their_original_item(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, _, inputs, window) = open_recording_workspace(cx, "inbox-targets");
        window
            .update(cx, |view, window, cx| {
                let first = fixture();
                let mut second = fixture();
                second.url = "https://github.com/demo/app/pull/43".into();
                second.title = "Second task".into();
                second.kind = WorkKind::PullRequest;
                view.work_inbox.github.items = vec![first.clone(), second.clone()];
                view.select_section(WorkspaceSection::Inbox, window, cx);
                view.select_work_item(&first.url, cx);
                view.work_inbox
                    .details
                    .entry(first.url.clone())
                    .or_default()
                    .draft = "Unsent comment".into();
                view.select_work_item(&second.url, cx);
                view.work_inbox
                    .details
                    .entry(second.url.clone())
                    .or_default()
                    .summary
                    .data = Some(WorkDetail {
                    head_oid: "original-head".into(),
                    ..Default::default()
                });
                let writes = inputs.lock().unwrap().len();
                view.propose_inbox_pr_action(&second.url, PrAction::Squash);
                view.work_inbox
                    .details
                    .get_mut(&second.url)
                    .unwrap()
                    .summary
                    .data
                    .as_mut()
                    .unwrap()
                    .head_oid = "new-head".into();
                assert_eq!(
                    view.work_inbox.confirmation.as_ref().unwrap().head_oid,
                    "original-head"
                );
                assert_eq!(inputs.lock().unwrap().len(), writes);
                view.select_work_item(&first.url, cx);
                assert!(view.work_inbox.confirmation.is_none());
                assert_eq!(view.work_inbox.details[&first.url].draft, "Unsent comment");
                view.work_inbox.search_editing = true;
                view.work_inbox.filter.query = "Second".into();
                let event = gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke {
                        key_char: Some("x".into()),
                        ..gpui::Keystroke::parse("x").unwrap()
                    },
                    is_held: false,
                };
                assert!(view.handle_inbox_key(&event, window, cx));
                assert!(view.work_inbox.selected.is_none());
                assert!(view.work_inbox.search_editing);
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn work_inbox_missing_target_never_falls_back_to_another_project(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, snapshot, inputs, window) = open_recording_workspace(cx, "work-inbox-missing");
        window
            .update(cx, |view, window, cx| {
                let mut item = fixture();
                item.project_id = Some(Uuid::new_v4());
                view.work_inbox.github.items.push(item.clone());
                view.select_work_item(&item.url, cx);
                let before = view.snapshot.terminal_sessions().count();
                let writes = inputs.lock().unwrap().len();
                view.start_work_item(window, cx);
                assert_eq!(view.snapshot.terminal_sessions().count(), before);
                assert_eq!(inputs.lock().unwrap().len(), writes);
                assert_eq!(
                    view.snapshot.selected_project_id,
                    snapshot.selected_project_id
                );
                assert!(view.work_inbox.action_error.is_some());
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
