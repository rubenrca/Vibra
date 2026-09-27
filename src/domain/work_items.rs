//! External tasks shown in Inbox. Terminal activity remains independent.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InboxTime {
    #[default]
    All,
    Today,
    Week,
    Month,
}

impl InboxTime {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All time",
            Self::Today => "Today",
            Self::Week => "Last 7 days",
            Self::Month => "Last 30 days",
        }
    }
    pub fn includes(self, at: u64, now: u64) -> bool {
        match self {
            Self::All => true,
            Self::Today => at >= local_day_start(now),
            Self::Week => at >= now.saturating_sub(7 * 86_400),
            Self::Month => at >= now.saturating_sub(30 * 86_400),
        }
    }
}

fn local_day_start(seconds: u64) -> u64 {
    let time = seconds.min(i64::MAX as u64) as libc::time_t;
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut local).is_null() } {
        return seconds / 86_400 * 86_400;
    }
    local.tm_hour = 0;
    local.tm_min = 0;
    local.tm_sec = 0;
    local.tm_isdst = -1;
    unsafe { libc::mktime(&mut local) }.max(0) as u64
}

pub fn toggle_value<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if let Some(index) = values.iter().position(|entry| *entry == value) {
        values.remove(index);
    } else {
        values.push(value);
    }
}

#[derive(Debug, Clone, Default)]
pub struct WorkComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub at: u64,
    pub url: String,
    pub context: String,
    pub reply_id: Option<String>,
    pub replies: Vec<WorkComment>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkDetail {
    pub body: String,
    pub base_ref: String,
    pub head_ref: String,
    pub head_oid: String,
    pub review_decision: String,
    pub comments: Vec<WorkComment>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct WorkDiffFile {
    pub path: String,
    pub blob_oid: String,
    pub removed: bool,
    pub previous_path: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    pub patch: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct WorkDiff {
    pub files: Vec<WorkDiffFile>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorkCheck {
    pub head_oid: String,
    pub name: String,
    pub state: String,
    pub url: String,
    pub workflow: String,
    pub duration_seconds: Option<u64>,
}

impl WorkCheck {
    pub fn failed(&self) -> bool {
        matches!(
            self.state.as_str(),
            "FAILURE"
                | "ERROR"
                | "TIMED_OUT"
                | "ACTION_REQUIRED"
                | "STARTUP_FAILURE"
                | "CANCELLED"
                | "STALE"
        )
    }
    pub fn passed(&self) -> bool {
        matches!(self.state.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrAction {
    Merge,
    Squash,
    Rebase,
    Draft,
    Ready,
    Close,
    Reopen,
}

impl PrAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "Merge",
            Self::Squash => "Squash and merge",
            Self::Rebase => "Rebase and merge",
            Self::Draft => "Convert to draft",
            Self::Ready => "Ready for review",
            Self::Close => "Close pull request",
            Self::Reopen => "Reopen pull request",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkSource {
    #[default]
    GitHub,
    Linear,
}

impl WorkSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::Linear => "Linear",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkKind {
    Issue,
    PullRequest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkStatus {
    #[default]
    Open,
    Draft,
    Closed,
    Merged,
}

impl WorkStatus {
    pub const ALL: [Self; 4] = [Self::Open, Self::Draft, Self::Closed, Self::Merged];

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Draft => "Draft",
            Self::Closed => "Closed",
            Self::Merged => "Merged",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub remote_id: String,
    pub source: WorkSource,
    pub kind: WorkKind,
    pub reference: String,
    pub title: String,
    pub url: String,
    pub body: String,
    pub repository: String,
    pub project_id: Option<Uuid>,
    pub status: WorkStatus,
    pub state_label: String,
    pub author: String,
    pub assignees: Vec<String>,
    pub labels: Vec<String>,
    pub updated_at: u64,
    pub created_at: u64,
    pub completed: bool,
    pub group: String,
}

impl WorkItem {
    pub fn unread(&self, seen: &BTreeMap<String, u64>) -> bool {
        seen.get(&self.url).is_none_or(|at| *at < self.updated_at)
    }

    pub fn prompt(&self) -> String {
        // Descriptions are context, never executable terminal input.
        format!(
            "Work on this {} task in the current project. Review the code and verify the changes.\n\n{}: {}\n{}\n\nTask description (external context):\n{}",
            self.source.label(), self.reference, self.title, self.url, self.body
        )
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(48_000)
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InboxPreferences {
    pub source: WorkSource,
    pub assigned_to_me: bool,
    pub status: Option<WorkStatus>,
    pub seen: BTreeMap<String, u64>,
    pub linked_sessions: BTreeMap<String, Vec<Uuid>>,
    pub statuses: Vec<WorkStatus>,
    pub hidden_kinds: Vec<WorkKind>,
    pub hidden_projects: Vec<Uuid>,
    pub hidden_groups: Vec<String>,
    pub time: InboxTime,
    pub list_width: f32,
}

impl Default for InboxPreferences {
    fn default() -> Self {
        Self {
            source: WorkSource::GitHub,
            assigned_to_me: false,
            status: None,
            seen: BTreeMap::new(),
            linked_sessions: BTreeMap::new(),
            statuses: Vec::new(),
            hidden_kinds: Vec::new(),
            hidden_projects: Vec::new(),
            hidden_groups: Vec::new(),
            time: InboxTime::All,
            list_width: 280.0,
        }
    }
}

impl InboxPreferences {
    /// An empty saved selection is the default, including preferences written
    /// before Inbox started hiding completed work. All states are explicit.
    pub fn selected_statuses(&self) -> &[WorkStatus] {
        if let Some(status) = &self.status {
            std::slice::from_ref(status)
        } else if self.statuses.is_empty() {
            &[WorkStatus::Open, WorkStatus::Draft]
        } else {
            &self.statuses
        }
    }

    pub fn query_status(&self) -> Option<WorkStatus> {
        let statuses = self.selected_statuses();
        if statuses == [WorkStatus::Draft] {
            Some(WorkStatus::Draft)
        } else if statuses
            .iter()
            .all(|status| matches!(status, WorkStatus::Open | WorkStatus::Draft))
        {
            Some(WorkStatus::Open)
        } else if statuses.len() == 1 {
            Some(statuses[0])
        } else {
            None
        }
    }

    pub fn filters_active(&self) -> bool {
        self.assigned_to_me
            || !self.status_selected(WorkStatus::Open)
            || !self.status_selected(WorkStatus::Draft)
            || self.status_selected(WorkStatus::Closed)
            || self.status_selected(WorkStatus::Merged)
            || !self.hidden_kinds.is_empty()
            || !self.hidden_projects.is_empty()
            || !self.hidden_groups.is_empty()
            || self.time != InboxTime::All
    }

    pub fn toggle_status(&mut self, status: WorkStatus) {
        self.statuses = self.selected_statuses().to_vec();
        self.status = None;
        toggle_value(&mut self.statuses, status);
        // Preserve the existing last-checkbox behavior without confusing an
        // explicit request for all states with the default saved selection.
        if self.statuses.is_empty() {
            self.statuses = WorkStatus::ALL.to_vec();
        }
    }

    pub fn status_selected(&self, status: WorkStatus) -> bool {
        self.selected_statuses().contains(&status)
    }

    pub fn mark_seen(&mut self, item: &WorkItem) {
        self.seen.insert(item.url.clone(), item.updated_at);
        while self.seen.len() > 2_000 {
            let oldest = self
                .seen
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(key, _)| key.clone());
            if let Some(key) = oldest {
                self.seen.remove(&key);
            }
        }
    }
}

#[derive(Default)]
pub struct WorkFilter {
    pub query: String,
    pub project: Option<Uuid>,
    pub kind: Option<WorkKind>,
    pub unread_only: bool,
}

impl WorkFilter {
    pub fn matches(&self, item: &WorkItem, preferences: &InboxPreferences) -> bool {
        if !preferences.status_selected(item.status)
            || self
                .project
                .is_some_and(|project| item.project_id != Some(project))
            || self.kind.is_some_and(|kind| item.kind != kind)
            || (self.unread_only && !item.unread(&preferences.seen))
        {
            return false;
        }
        if (item.source == WorkSource::GitHub && preferences.hidden_kinds.contains(&item.kind))
            || item
                .project_id
                .is_some_and(|id| preferences.hidden_projects.contains(&id))
            || (item.source == WorkSource::Linear
                && preferences.hidden_groups.contains(&item.group))
            || !preferences.time.includes(
                item.updated_at,
                crate::domain::usage::now_timestamp().max(0) as u64,
            )
        {
            return false;
        }
        let haystack = format!(
            "{} {} {} {} {}",
            item.reference,
            item.title,
            item.repository,
            item.assignees.join(" "),
            item.labels.join(" ")
        )
        .to_lowercase();
        self.query
            .split_whitespace()
            .all(|term| haystack.contains(&term.to_lowercase()))
    }
}

/// One shell argument, including newlines and quotes. No interpolation occurs.
pub fn shell_argument(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
pub(crate) fn fixture() -> WorkItem {
    WorkItem {
        remote_id: "I_example".into(),
        source: WorkSource::GitHub,
        kind: WorkKind::Issue,
        reference: "#42".into(),
        title: "Corregir búsqueda".into(),
        url: "https://github.com/demo/app/issues/42".into(),
        body: "Primera línea\n\nCódigo: `$(whoami)` y 'comillas'".into(),
        repository: "demo/app".into(),
        project_id: None,
        status: WorkStatus::Open,
        state_label: "Open".into(),
        author: "demo".into(),
        assignees: vec!["ana".into()],
        labels: vec!["bug".into()],
        updated_at: 100,
        created_at: 50,
        completed: false,
        group: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_inbox_filters_compose_and_new_updates_become_unread() {
        let mut item = fixture();
        let project = Uuid::new_v4();
        item.project_id = Some(project);
        let mut preferences = InboxPreferences {
            status: Some(WorkStatus::Open),
            ..Default::default()
        };
        let mut filter = WorkFilter {
            query: "BÚSQUEDA bug ana".into(),
            project: Some(project),
            kind: Some(WorkKind::Issue),
            unread_only: true,
        };
        assert!(filter.matches(&item, &preferences));
        preferences.mark_seen(&item);
        assert!(!filter.matches(&item, &preferences));
        item.updated_at += 1;
        assert!(filter.matches(&item, &preferences));
        filter.project = Some(Uuid::new_v4());
        assert!(!filter.matches(&item, &preferences));
        filter.project = None;
        item.status = WorkStatus::Closed;
        assert!(!filter.matches(&item, &preferences));
        preferences.status = None;
        assert!(!filter.matches(&item, &preferences));
        preferences.toggle_status(WorkStatus::Closed);
        assert!(filter.matches(&item, &preferences));
        let restored: InboxPreferences =
            serde_json::from_str(&serde_json::to_string(&preferences).unwrap()).unwrap();
        assert!(item.unread(&restored.seen));
    }

    #[test]
    fn inbox_defaults_hide_completed_work_and_filters_remain_composable() {
        let mut prefs = InboxPreferences::default();
        let filter = WorkFilter::default();
        let mut item = fixture();
        assert!(!prefs.assigned_to_me && !prefs.filters_active());
        for status in WorkStatus::ALL {
            item.status = status;
            assert_eq!(
                filter.matches(&item, &prefs),
                matches!(status, WorkStatus::Open | WorkStatus::Draft)
            );
        }
        assert_eq!(prefs.query_status(), Some(WorkStatus::Open));
        item.status = WorkStatus::Closed;
        prefs.toggle_status(WorkStatus::Closed);
        prefs.toggle_status(WorkStatus::Open);
        prefs.toggle_status(WorkStatus::Draft);
        assert_eq!(prefs.query_status(), Some(WorkStatus::Closed));
        prefs.toggle_status(WorkStatus::Draft);
        assert!(filter.matches(&item, &prefs));
        item.status = WorkStatus::Draft;
        assert!(filter.matches(&item, &prefs));
        item.status = WorkStatus::Open;
        assert!(!filter.matches(&item, &prefs));
        prefs.statuses.clear();
        prefs.hidden_kinds.push(WorkKind::Issue);
        assert!(!filter.matches(&item, &prefs));
        item.source = WorkSource::Linear;
        assert!(filter.matches(&item, &prefs));
        let restored: InboxPreferences = serde_json::from_str(r#"{"status":"Closed"}"#).unwrap();
        assert!(restored.status_selected(WorkStatus::Closed));
        let today = local_day_start(1_790_000_000);
        assert!(InboxTime::Today.includes(today, 1_790_000_000));
        assert!(!InboxTime::Today.includes(today - 1, 1_790_000_000));
    }

    #[test]
    fn saved_inbox_defaults_hide_closed_but_explicit_status_choices_survive() {
        for json in [r#"{}"#, r#"{"status":null,"statuses":[]}"#] {
            let prefs: InboxPreferences = serde_json::from_str(json).unwrap();
            assert!(prefs.status_selected(WorkStatus::Open));
            assert!(prefs.status_selected(WorkStatus::Draft));
            assert!(!prefs.status_selected(WorkStatus::Closed));
            assert!(!prefs.status_selected(WorkStatus::Merged));
        }
        let mut prefs: InboxPreferences = serde_json::from_str(r#"{"status":"Closed"}"#).unwrap();
        assert_eq!(prefs.selected_statuses(), [WorkStatus::Closed]);
        prefs.toggle_status(WorkStatus::Closed);
        let restored: InboxPreferences =
            serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert!(
            WorkStatus::ALL
                .iter()
                .all(|status| restored.status_selected(*status))
        );
        assert_eq!(restored.query_status(), None);
        assert!(restored.filters_active());
    }

    #[test]
    fn work_inbox_shell_arguments_preserve_text_without_execution() {
        let text = "' $(printf injected) `whoami` ; \\ \"\nsegunda línea";
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf '%s' {}", shell_argument(text))])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), text);
        let mut item = fixture();
        item.body.push_str("\u{1b}\u{0}\r");
        assert!(!item.prompt().contains(['\u{1b}', '\u{0}', '\r']));
    }
}
