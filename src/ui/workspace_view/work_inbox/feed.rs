use std::collections::HashSet;
use std::time::Duration;

use gpui::{Context, Timer, prelude::*};
use uuid::Uuid;

use crate::domain::work_items::{WorkFilter, WorkItem, WorkSource};
use crate::infrastructure::work_items::{self, InboxProject, WorkQuery};

use super::{DetailTab, WorkspaceSection, WorkspaceView};

impl WorkspaceView {
    pub(crate) fn scope_inbox_to_active_project(&mut self) {
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

    pub(crate) fn inbox_project_selected(&self, id: Uuid) -> bool {
        self.work_inbox.filter.project.map_or_else(
            || !self.settings.inbox.hidden_projects.contains(&id),
            |project| project == id,
        )
    }

    pub(crate) fn toggle_inbox_project(&mut self, id: Uuid, cx: &mut Context<Self>) {
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

    pub(crate) fn sync_inbox_selection(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn inbox_filters_changed(&mut self, cx: &mut Context<Self>) {
        self.ensure_inbox_loaded(cx);
        self.sync_inbox_selection(cx);
        self.persist_settings(cx);
    }
    pub(crate) fn inbox_query(&self) -> WorkQuery {
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

    pub(crate) fn ensure_inbox_loaded(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        self.refresh_work_source(WorkSource::GitHub, false, cx);
        self.refresh_work_source(WorkSource::Linear, false, cx);
        self.inbox_source_updated(self.settings.inbox.source, false, cx);
    }

    pub(crate) fn prune_inbox_details(&mut self) {
        let mut keep: HashSet<&str> = self
            .work_inbox
            .github
            .items
            .iter()
            .chain(self.work_inbox.linear.items.iter())
            .map(|item| item.url.as_str())
            .collect();
        if let Some(selected) = self.work_inbox.selected.as_deref() {
            keep.insert(selected);
        }
        keep.extend(self.work_inbox.discussion_panes.keys().map(String::as_str));
        self.work_inbox.details.retain(|url, state| {
            keep.contains(url.as_str()) || state.posting || state.mutation_busy
        });
    }

    pub(crate) fn inbox_source_updated(
        &mut self,
        source: WorkSource,
        refresh_detail: bool,
        cx: &mut Context<Self>,
    ) {
        self.prune_inbox_details();
        if self.workspace_section == WorkspaceSection::Inbox
            && !self.work_inbox.activity
            && source == self.settings.inbox.source
        {
            self.sync_inbox_selection(cx);
            self.load_inbox_detail(refresh_detail, cx);
        }
        cx.notify();
    }

    pub(crate) fn start_inbox_poll(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn refresh_work_source(
        &mut self,
        source: WorkSource,
        force: bool,
        cx: &mut Context<Self>,
    ) {
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

    pub(crate) fn select_work_source(&mut self, source: WorkSource, cx: &mut Context<Self>) {
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

    pub(crate) fn select_work_item(&mut self, url: &str, cx: &mut Context<Self>) {
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

    pub(crate) fn selected_work_item(&self) -> Option<&WorkItem> {
        self.work_inbox
            .feed(self.settings.inbox.source)
            .items
            .iter()
            .find(|item| Some(&item.url) == self.work_inbox.selected.as_ref())
    }

    pub(crate) fn work_inbox_unread_count(&self) -> usize {
        self.work_inbox
            .github
            .items
            .iter()
            .chain(&self.work_inbox.linear.items)
            .filter(|item| item.unread(&self.settings.inbox.seen))
            .count()
    }
}
