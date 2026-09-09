use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CURRENT_WORKSPACE_SCHEMA_VERSION: u32 = 6;
pub const DEFAULT_PANE_SPLIT_RATIO: u16 = 5_000;
pub(crate) const MIN_PANE_SPLIT_RATIO: u16 = 1_000;
pub(crate) const MAX_PANE_SPLIT_RATIO: u16 = 9_000;

const fn default_pane_split_ratio() -> u16 {
    DEFAULT_PANE_SPLIT_RATIO
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    /// Zero identifies snapshots written before explicit schema versioning.
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub projects: Vec<ProjectSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_project_id: Option<Uuid>,
    /// Stable visual order for the workspace entries in the sessions sidebar.
    ///
    /// Workspaces remain grouped by project for their data model, so this is kept
    /// separately to allow a user to reorder entries across projects as well.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_order: Vec<Uuid>,
    /// Complete visual order for the sessions sidebar, including user-created
    /// spaces. `workspace_order` remains as a backwards-compatible mirror.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidebar_items: Vec<SidebarItemSnapshot>,
}

impl Default for WorkspaceSnapshot {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_WORKSPACE_SCHEMA_VERSION,
            projects: Vec::new(),
            selected_project_id: None,
            workspace_order: Vec::new(),
            sidebar_items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SidebarItemSnapshot {
    Workspace {
        workspace_id: Uuid,
    },
    Space {
        id: Uuid,
        name: String,
        #[serde(default)]
        collapsed: bool,
        #[serde(default)]
        workspace_ids: Vec<Uuid>,
    },
    /// Temporary schema-5 representation, migrated by `normalize`.
    Spacer {
        id: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSnapshot {
    pub id: Uuid,
    pub name: String,
    pub root_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_session_ids: Option<Vec<Uuid>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_axis: Option<WorkspaceSplitAxis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tabs: Option<Vec<TabSnapshot>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_tab_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces: Option<Vec<TerminalWorkspaceSnapshot>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_workspace_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWorkspaceSnapshot {
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_source: Option<WorkspaceTitleSource>,
    #[serde(default)]
    pub tabs: Vec<TabSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_tab_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceTitleSource {
    Automatic,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabSnapshot {
    pub id: Uuid,
    #[serde(default)]
    pub sessions: Vec<SessionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoomed_session_id: Option<Uuid>,
    pub layout: PaneLayoutSnapshot,
}

/// The externally-tagged representation deliberately matches Swift's synthesized
/// `Codable` payload, including the `_0` field of the single-value case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaneLayoutSnapshot {
    Terminal {
        #[serde(rename = "_0")]
        id: Uuid,
    },
    Split {
        axis: WorkspaceSplitAxis,
        #[serde(default = "default_pane_split_ratio")]
        ratio: u16,
        first: Box<PaneLayoutSnapshot>,
        second: Box<PaneLayoutSnapshot>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceSplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneSplitDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocusDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneResizeDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneBranch {
    First,
    Second,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub id: Uuid,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_task_title: Option<String>,
    pub working_directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub project_id: Uuid,
    pub workspace_id: Uuid,
    pub project_name: String,
    pub workspace_name: String,
    /// `true` when the user renamed the tab; automatic titles follow the live cwd.
    pub title_is_manual: bool,
    /// Working directory of the selected (or first) session in this workspace.
    pub working_directory: String,
    pub session_count: usize,
    pub is_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarEntry {
    Workspace {
        entry: WorkspaceEntry,
        space_id: Option<Uuid>,
    },
    Space {
        id: Uuid,
        name: String,
        collapsed: bool,
        workspace_count: usize,
    },
}
