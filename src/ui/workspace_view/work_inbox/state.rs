use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gpui::Task;
use uuid::Uuid;

use crate::domain::work_items::{
    PrAction, WorkCheck, WorkDetail, WorkDiff, WorkFilter, WorkItem, WorkSource,
};
use crate::infrastructure::work_items::{WorkItemsPage, WorkQuery};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum InboxMenu {
    Filters,
    Connections,
    Projects,
    Agents,
    PrActions,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum DetailTab {
    #[default]
    Summary,
    Code,
    Checks,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum CodeMode {
    #[default]
    Hunks,
    FullFile,
}

pub(super) struct LoadState<T> {
    pub(super) data: Option<T>,
    pub(super) error: Option<String>,
    pub(super) loading: bool,
    pub(super) revision: u64,
    pub(super) _task: Option<Task<()>>,
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
    pub(super) fn invalidate(&mut self) {
        self.data = None;
        self.error = None;
        self.loading = false;
        self.revision += 1;
        self._task = None;
    }

    pub(super) fn apply(&mut self, revision: u64, result: anyhow::Result<T>) {
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
pub(super) struct ItemState {
    pub(super) updated_at: u64,
    pub(super) check_logs: HashMap<String, LoadState<String>>,
    pub(super) expanded_checks: HashSet<String>,
    pub(super) summary: LoadState<WorkDetail>,
    pub(super) diff: LoadState<WorkDiff>,
    pub(super) checks: LoadState<Vec<WorkCheck>>,
    pub(super) review: Option<gpui::Entity<crate::ui::diff_view::DiffView>>,
    pub(super) _review_subscription: Option<gpui::Subscription>,
    pub(super) code_mode: CodeMode,
    pub(super) checks_attention_only: bool,
    pub(super) draft: String,
    pub(super) reply: Option<(String, String)>,
    pub(super) posting: bool,
    pub(super) post_error: Option<String>,
    pub(super) mutation_busy: bool,
    pub(super) mutation_error: Option<String>,
    pub(super) _post_task: Option<Task<()>>,
    pub(super) _mutation_task: Option<Task<()>>,
}

#[derive(Clone)]
pub(super) struct PrConfirmation {
    pub(super) url: String,
    pub(super) action: PrAction,
    pub(super) head_oid: String,
}

#[derive(Default)]
pub(super) struct SourceFeed {
    pub(super) items: Vec<WorkItem>,
    pub(super) connected: Option<bool>,
    pub(super) error: Option<String>,
    pub(super) loading: bool,
    pub(super) truncated: bool,
    pub(super) fetched: Option<Instant>,
    pub(super) query: Option<WorkQuery>,
    pub(super) generation: u64,
    pub(super) _task: Option<Task<()>>,
}

impl SourceFeed {
    pub(super) fn restore(&mut self, generation: u64, page: WorkItemsPage) -> bool {
        if generation != self.generation || self.fetched.is_some() {
            return false;
        }
        self.apply_page(page);
        // A disk snapshot is usable immediately but still needs a live refresh.
        // Keep loading set and do not start the refresh cooldown yet.
        true
    }

    pub(super) fn accept(
        &mut self,
        generation: u64,
        result: anyhow::Result<WorkItemsPage>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.apply(result);
        true
    }

    pub(super) fn apply(&mut self, result: anyhow::Result<WorkItemsPage>) {
        self.loading = false;
        self.fetched = Some(Instant::now());
        match result {
            Ok(page) => self.apply_page(page),
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    pub(super) fn apply_page(&mut self, mut page: WorkItemsPage) {
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
pub(crate) struct WorkInbox {
    pub activity: bool,
    pub(super) github: SourceFeed,
    pub(super) linear: SourceFeed,
    pub(super) filter: WorkFilter,
    pub(super) search_editing: bool,
    pub(super) selected: Option<String>,
    pub(super) target_project: Option<Uuid>,
    pub(super) agent: usize,
    pub(super) action_error: Option<String>,
    pub(super) menu: Option<InboxMenu>,
    pub(super) menu_position: (f32, f32),
    pub(super) detail_tab: DetailTab,
    pub(super) details: HashMap<String, ItemState>,
    pub(super) epoch: u64,
    pub(super) discussion_open: bool,
    pub(super) discussion_panes: HashMap<String, Uuid>,
    pub(super) composer_open: bool,
    pub(super) composer_note: String,
    pub(super) composer_editing: bool,
    pub(super) comment_editing: bool,
    pub(super) confirmation: Option<PrConfirmation>,
    pub(super) connecting: bool,
    pub(super) connection_error: Option<String>,
    pub(super) _connection_task: Option<Task<()>>,
    pub(super) _poll_task: Option<Task<()>>,
}

impl WorkInbox {
    pub(in crate::ui::workspace_view) fn forget_closed_discussion_panes(
        &mut self,
        live_ids: &HashSet<Uuid>,
    ) {
        self.discussion_panes
            .retain(|_, pane| live_ids.contains(pane));
    }

    pub(super) fn feed(&self, source: WorkSource) -> &SourceFeed {
        match source {
            WorkSource::GitHub => &self.github,
            WorkSource::Linear => &self.linear,
        }
    }

    pub(super) fn feed_mut(&mut self, source: WorkSource) -> &mut SourceFeed {
        match source {
            WorkSource::GitHub => &mut self.github,
            WorkSource::Linear => &mut self.linear,
        }
    }
}

pub(super) const AGENTS: [(&str, &str); 3] = [
    ("Claude", "claude"),
    ("Codex", "codex"),
    ("Gemini", "gemini"),
];
