use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CURRENT_WORKSPACE_SCHEMA_VERSION: u32 = 3;
pub const DEFAULT_PANE_SPLIT_RATIO: u16 = 5_000;
const MIN_PANE_SPLIT_RATIO: u16 = 1_000;
const MAX_PANE_SPLIT_RATIO: u16 = 9_000;

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
}

impl Default for WorkspaceSnapshot {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_WORKSPACE_SCHEMA_VERSION,
            projects: Vec::new(),
            selected_project_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSnapshot {
    pub id: Uuid,
    pub name: String,
    pub root_path: String,
    #[serde(default)]
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

impl WorkspaceSnapshot {
    pub fn normalize(&mut self) {
        for project in &mut self.projects {
            project.normalize();
        }
        self.projects.retain(|project| {
            project
                .workspaces
                .as_ref()
                .is_some_and(|workspaces| !workspaces.is_empty())
        });

        if self.projects.is_empty() {
            self.selected_project_id = None;
        } else if !self
            .projects
            .iter()
            .any(|project| Some(project.id) == self.selected_project_id)
        {
            self.selected_project_id = Some(self.projects[0].id);
        }

        // Normalization performs the legacy-to-canonical conversions above, so a
        // successfully normalized snapshot is safe to persist as the current schema.
        self.schema_version = CURRENT_WORKSPACE_SCHEMA_VERSION;
    }

    pub fn create_workspace(&mut self, root: &Path) {
        let root_path = root.to_string_lossy().into_owned();
        let directory_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Terminal")
            .to_owned();

        let session = SessionSnapshot::new(root_path.clone());
        let tab = TabSnapshot::with_session(session);
        let workspace = TerminalWorkspaceSnapshot {
            id: Uuid::new_v4(),
            name: directory_name.clone(),
            title_source: Some(WorkspaceTitleSource::Automatic),
            selected_tab_id: Some(tab.id),
            tabs: vec![tab],
        };

        if let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.root_path == root_path)
        {
            let workspace_id = workspace.id;
            project.workspaces.get_or_insert_default().push(workspace);
            project.selected_workspace_id = Some(workspace_id);
            self.selected_project_id = Some(project.id);
        } else {
            let project_id = Uuid::new_v4();
            let workspace_id = workspace.id;
            self.projects.push(ProjectSnapshot {
                id: project_id,
                name: directory_name,
                root_path,
                sessions: Vec::new(),
                selected_session_id: None,
                visible_session_ids: None,
                split_axis: None,
                tabs: None,
                selected_tab_id: None,
                workspaces: Some(vec![workspace]),
                selected_workspace_id: Some(workspace_id),
            });
            self.selected_project_id = Some(project_id);
        }

        self.normalize();
    }

    /// Relocates workspaces created from an unsafe launcher fallback (typically `/`).
    /// Only exact path matches are changed, so intentionally configured subdirectories
    /// and sessions that have moved elsewhere are preserved.
    pub fn relocate_root(&mut self, from: &Path, to: &Path) -> bool {
        if from == to || !to.is_dir() {
            return false;
        }
        let directory_name = to
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Terminal")
            .to_owned();
        let from = from.to_string_lossy();
        let to = to.to_string_lossy().into_owned();
        let mut changed = false;

        for project in &mut self.projects {
            if project.root_path != from {
                continue;
            }
            project.root_path.clone_from(&to);
            if matches!(project.name.as_str(), "Terminal" | "/") {
                project.name.clone_from(&directory_name);
            }
            if let Some(workspaces) = project.workspaces.as_mut() {
                for workspace in workspaces {
                    if workspace.title_source != Some(WorkspaceTitleSource::Manual)
                        && matches!(workspace.name.as_str(), "Terminal" | "/")
                    {
                        workspace.name.clone_from(&directory_name);
                    }
                    for tab in &mut workspace.tabs {
                        for session in &mut tab.sessions {
                            if session.working_directory == from {
                                session.working_directory.clone_from(&to);
                            }
                        }
                    }
                }
            }
            for session in &mut project.sessions {
                if session.working_directory == from {
                    session.working_directory.clone_from(&to);
                }
            }
            if let Some(tabs) = project.tabs.as_mut() {
                for tab in tabs {
                    for session in &mut tab.sessions {
                        if session.working_directory == from {
                            session.working_directory.clone_from(&to);
                        }
                    }
                }
            }
            project.normalize();
            changed = true;
        }

        changed
    }

    pub fn create_terminal_tab(&mut self) -> bool {
        self.create_terminal_tab_with_focus(true).is_some()
    }

    /// Creates a terminal tab in the selected workspace.
    /// When `focus_new` is false the previous selected tab stays selected.
    pub fn create_terminal_tab_with_focus(&mut self, focus_new: bool) -> Option<(Uuid, Uuid)> {
        self.create_terminal_tab_with_options(focus_new, None)
    }

    pub fn create_terminal_tab_with_options(
        &mut self,
        focus_new: bool,
        working_directory: Option<String>,
    ) -> Option<(Uuid, Uuid)> {
        let (project_index, workspace_index) = self.selected_workspace_indices()?;
        let project = &mut self.projects[project_index];
        let working_directory = working_directory.unwrap_or_else(|| project.root_path.clone());
        let tab = TabSnapshot::with_session(SessionSnapshot::new(working_directory));
        let tab_id = tab.id;
        let session_id = tab.selected_session_id?;
        let workspace = &mut project.workspaces.as_mut().expect("normalized")[workspace_index];
        workspace.tabs.push(tab);
        if focus_new {
            workspace.selected_tab_id = Some(tab_id);
        }
        project.normalize();
        Some((tab_id, session_id))
    }

    pub fn split_selected_terminal(&mut self, direction: PaneSplitDirection) -> Option<Uuid> {
        self.split_selected_terminal_with_focus(direction, true)
    }

    /// Splits the selected terminal. When `focus_new` is false the original pane
    /// remains selected so automation can spawn siblings without stealing focus.
    pub fn split_selected_terminal_with_focus(
        &mut self,
        direction: PaneSplitDirection,
        focus_new: bool,
    ) -> Option<Uuid> {
        self.split_selected_terminal_with_options(direction, focus_new, None)
    }

    pub fn split_selected_terminal_with_options(
        &mut self,
        direction: PaneSplitDirection,
        focus_new: bool,
        working_directory: Option<String>,
    ) -> Option<Uuid> {
        let (project_index, workspace_index, tab_index, session_index) =
            self.selected_session_indices()?;
        let project = &mut self.projects[project_index];
        let tab =
            &mut project.workspaces.as_mut().expect("normalized")[workspace_index].tabs[tab_index];
        let selected_id = tab.sessions[session_index].id;
        let working_directory = working_directory
            .unwrap_or_else(|| tab.sessions[session_index].working_directory.clone());
        let session = SessionSnapshot::new(working_directory);
        let session_id = session.id;
        let (axis, insert_first) = match direction {
            PaneSplitDirection::Left => (WorkspaceSplitAxis::Horizontal, true),
            PaneSplitDirection::Right => (WorkspaceSplitAxis::Horizontal, false),
            PaneSplitDirection::Up => (WorkspaceSplitAxis::Vertical, true),
            PaneSplitDirection::Down => (WorkspaceSplitAxis::Vertical, false),
        };
        if !tab
            .layout
            .split_terminal(selected_id, session_id, axis, insert_first)
        {
            return None;
        }
        tab.sessions.push(session);
        if focus_new {
            tab.selected_session_id = Some(session_id);
        }
        tab.zoomed_session_id = None;
        project.normalize();
        Some(session_id)
    }

    pub fn select_terminal(&mut self, session_id: Uuid) -> bool {
        let Some((project_index, workspace_index)) = self.selected_workspace_indices() else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let workspace = &mut project.workspaces.as_mut().expect("normalized")[workspace_index];
        let Some(tab) = workspace
            .tabs
            .iter_mut()
            .find(|tab| Some(tab.id) == workspace.selected_tab_id)
        else {
            return false;
        };
        if !tab.layout.contains_terminal(session_id) {
            return false;
        }
        if tab.selected_session_id == Some(session_id) {
            return false;
        }
        tab.selected_session_id = Some(session_id);
        project.normalize();
        true
    }

    pub fn select_terminal_global(&mut self, session_id: Uuid) -> bool {
        let found = self
            .projects
            .iter()
            .enumerate()
            .find_map(|(project_index, project)| {
                project
                    .workspaces
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .find_map(|(workspace_index, workspace)| {
                        workspace
                            .tabs
                            .iter()
                            .enumerate()
                            .find_map(|(tab_index, tab)| {
                                tab.sessions
                                    .iter()
                                    .any(|session| session.id == session_id)
                                    .then_some((project_index, workspace_index, tab_index))
                            })
                    })
            });
        let Some((project_index, workspace_index, tab_index)) = found else {
            return false;
        };
        let project_id = self.projects[project_index].id;
        let project = &mut self.projects[project_index];
        let workspace = &mut project.workspaces.as_mut().expect("normalized")[workspace_index];
        project.selected_workspace_id = Some(workspace.id);
        workspace.selected_tab_id = Some(workspace.tabs[tab_index].id);
        workspace.tabs[tab_index].selected_session_id = Some(session_id);
        self.selected_project_id = Some(project_id);
        project.normalize();
        true
    }

    pub fn focus_terminal(&mut self, direction: PaneFocusDirection) -> bool {
        let Some(tab) = self.selected_tab() else {
            return false;
        };
        let Some(selected_id) = tab.selected_session_id else {
            return false;
        };
        let Some(next_id) = tab.layout.adjacent_terminal(selected_id, direction) else {
            return false;
        };
        self.select_terminal(next_id)
    }

    pub fn cycle_terminal(&mut self, offset: isize) -> bool {
        if offset == 0 {
            return false;
        }
        let Some(tab) = self.selected_tab() else {
            return false;
        };
        let ids = tab.layout.terminal_ids();
        let Some(selected_id) = tab.selected_session_id else {
            return false;
        };
        let Some(current) = ids.iter().position(|id| *id == selected_id) else {
            return false;
        };
        let next = (current as isize + offset).rem_euclid(ids.len() as isize) as usize;
        self.select_terminal(ids[next])
    }

    pub fn resize_selected_pane(&mut self, direction: PaneResizeDirection) -> bool {
        let Some((project_index, workspace_index, tab_index, _)) = self.selected_session_indices()
        else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let tab =
            &mut project.workspaces.as_mut().expect("normalized")[workspace_index].tabs[tab_index];
        let selected_id = tab
            .selected_session_id
            .expect("selected session index exists");
        let (axis, delta) = match direction {
            PaneResizeDirection::Left => (WorkspaceSplitAxis::Horizontal, -500),
            PaneResizeDirection::Right => (WorkspaceSplitAxis::Horizontal, 500),
            PaneResizeDirection::Up => (WorkspaceSplitAxis::Vertical, -500),
            PaneResizeDirection::Down => (WorkspaceSplitAxis::Vertical, 500),
        };
        let changed = tab.layout.move_nearest_divider(selected_id, axis, delta);
        if changed {
            project.normalize();
        }
        changed
    }

    pub fn set_selected_split_ratio(&mut self, path: &[PaneBranch], ratio: u16) -> bool {
        let Some((project_index, workspace_index, tab_index, _)) = self.selected_session_indices()
        else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let tab =
            &mut project.workspaces.as_mut().expect("normalized")[workspace_index].tabs[tab_index];
        let changed = tab.layout.set_split_ratio(path, ratio);
        if changed {
            project.normalize();
        }
        changed
    }

    pub fn equalize_selected_panes(&mut self) -> bool {
        let Some((project_index, workspace_index, tab_index, _)) = self.selected_session_indices()
        else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let tab =
            &mut project.workspaces.as_mut().expect("normalized")[workspace_index].tabs[tab_index];
        let changed = tab.layout.equalize();
        if changed {
            project.normalize();
        }
        changed
    }

    pub fn toggle_selected_pane_zoom(&mut self) -> bool {
        let Some((project_index, workspace_index, tab_index, _)) = self.selected_session_indices()
        else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let tab =
            &mut project.workspaces.as_mut().expect("normalized")[workspace_index].tabs[tab_index];
        let selected_id = tab
            .selected_session_id
            .expect("selected session index exists");
        tab.zoomed_session_id = (tab.zoomed_session_id != Some(selected_id)).then_some(selected_id);
        project.normalize();
        true
    }

    pub fn close_tab(&mut self, tab_id: Uuid) -> bool {
        let Some((project_index, workspace_index)) = self.selected_workspace_indices() else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let workspaces = project.workspaces.as_mut().expect("normalized");
        let workspace = &mut workspaces[workspace_index];
        let Some(tab_index) = workspace.tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        workspace.tabs.remove(tab_index);
        if workspace.tabs.is_empty() {
            workspaces.remove(workspace_index);
        }
        if workspaces.is_empty() {
            self.projects.remove(project_index);
        }
        self.normalize();
        true
    }

    pub fn close_selected_terminal(&mut self) -> bool {
        let Some((_, _, _, session_index)) = self.selected_session_indices() else {
            return false;
        };
        let Some(session_id) = self
            .selected_tab()
            .and_then(|tab| tab.sessions.get(session_index).map(|session| session.id))
        else {
            return false;
        };
        self.close_terminal(session_id)
    }

    pub fn close_terminal(&mut self, session_id: Uuid) -> bool {
        if !self.select_terminal_global(session_id) {
            return false;
        }
        let Some((project_index, workspace_index, tab_index, session_index)) =
            self.selected_session_indices()
        else {
            return false;
        };

        let project = &mut self.projects[project_index];
        let workspaces = project.workspaces.as_mut().expect("normalized");
        let workspace = &mut workspaces[workspace_index];
        let tab = &mut workspace.tabs[tab_index];
        if tab.sessions.get(session_index).map(|s| s.id) != Some(session_id) {
            return false;
        }
        let old_order = tab.layout.terminal_ids();
        let removed_id = tab.sessions.remove(session_index).id;

        if tab.sessions.is_empty() {
            workspace.tabs.remove(tab_index);
        } else {
            tab.layout = tab
                .layout
                .removing_terminal(removed_id)
                .unwrap_or_else(|| PaneLayoutSnapshot::terminal(tab.sessions[0].id));
            let remaining_order = tab.layout.terminal_ids();
            let removed_index = old_order
                .iter()
                .position(|id| *id == removed_id)
                .unwrap_or(0);
            tab.selected_session_id = remaining_order
                .get(removed_index.min(remaining_order.len() - 1))
                .copied();
            if tab.zoomed_session_id == Some(removed_id) {
                tab.zoomed_session_id = None;
            }
        }

        if workspace.tabs.is_empty() {
            workspaces.remove(workspace_index);
        }
        if workspaces.is_empty() {
            self.projects.remove(project_index);
        }
        self.normalize();
        true
    }

    pub fn rename_workspace(&mut self, project_id: Uuid, workspace_id: Uuid, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        else {
            return false;
        };
        let Some(workspace) = project
            .workspaces
            .as_mut()
            .and_then(|workspaces| workspaces.iter_mut().find(|item| item.id == workspace_id))
        else {
            return false;
        };
        if workspace.name == name && workspace.title_source == Some(WorkspaceTitleSource::Manual) {
            return false;
        }
        workspace.name = name.to_owned();
        workspace.title_source = Some(WorkspaceTitleSource::Manual);
        true
    }

    pub fn close_workspace(&mut self, project_id: Uuid, workspace_id: Uuid) -> bool {
        let Some(project_index) = self
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            return false;
        };
        let Some(workspace_index) =
            self.projects[project_index]
                .workspaces
                .as_ref()
                .and_then(|workspaces| {
                    workspaces
                        .iter()
                        .position(|workspace| workspace.id == workspace_id)
                })
        else {
            return false;
        };
        let workspaces = self.projects[project_index]
            .workspaces
            .as_mut()
            .expect("checked above");
        workspaces.remove(workspace_index);
        if workspaces.is_empty() {
            self.projects.remove(project_index);
        }
        self.normalize();
        true
    }

    pub fn select_workspace(&mut self, project_id: Uuid, workspace_id: Uuid) -> bool {
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        else {
            return false;
        };
        let exists = project
            .workspaces
            .as_ref()
            .is_some_and(|workspaces| workspaces.iter().any(|item| item.id == workspace_id));
        if !exists {
            return false;
        }
        project.selected_workspace_id = Some(workspace_id);
        self.selected_project_id = Some(project_id);
        true
    }

    pub fn select_tab(&mut self, tab_id: Uuid) -> bool {
        let Some((project_index, workspace_index)) = self.selected_workspace_indices() else {
            return false;
        };
        let workspace = &mut self.projects[project_index]
            .workspaces
            .as_mut()
            .expect("normalized")[workspace_index];
        if workspace.tabs.iter().any(|tab| tab.id == tab_id) {
            workspace.selected_tab_id = Some(tab_id);
            true
        } else {
            false
        }
    }

    pub fn cycle_workspace(&mut self, offset: isize) -> bool {
        let entries = self.workspace_entries();
        if entries.is_empty() || offset == 0 {
            return false;
        }
        let current = entries
            .iter()
            .position(|entry| entry.is_selected)
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(entries.len() as isize) as usize;
        self.select_workspace(entries[next].project_id, entries[next].workspace_id)
    }

    pub fn workspace_entries(&self) -> Vec<WorkspaceEntry> {
        self.projects
            .iter()
            .flat_map(|project| {
                project
                    .workspaces
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(move |workspace| WorkspaceEntry {
                        project_id: project.id,
                        workspace_id: workspace.id,
                        project_name: project.name.clone(),
                        workspace_name: workspace.name.clone(),
                        title_is_manual: workspace.title_source
                            == Some(WorkspaceTitleSource::Manual),
                        working_directory: workspace
                            .primary_working_directory()
                            .unwrap_or_else(|| project.root_path.clone()),
                        session_count: workspace.tabs.iter().map(|tab| tab.sessions.len()).sum(),
                        is_selected: self.selected_project_id == Some(project.id)
                            && project.selected_workspace_id == Some(workspace.id),
                    })
            })
            .collect()
    }

    pub fn selected_workspace(&self) -> Option<&TerminalWorkspaceSnapshot> {
        let project = self
            .projects
            .iter()
            .find(|project| Some(project.id) == self.selected_project_id)?;
        let workspace_id = project.selected_workspace_id?;
        project
            .workspaces
            .as_ref()?
            .iter()
            .find(|workspace| workspace.id == workspace_id)
    }

    pub fn selected_project(&self) -> Option<&ProjectSnapshot> {
        self.projects
            .iter()
            .find(|project| Some(project.id) == self.selected_project_id)
    }

    pub fn selected_tab(&self) -> Option<&TabSnapshot> {
        let workspace = self.selected_workspace()?;
        workspace
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == workspace.selected_tab_id)
    }

    pub fn selected_session(&self) -> Option<&SessionSnapshot> {
        let tab = self.selected_tab()?;
        tab.sessions
            .iter()
            .find(|session| Some(session.id) == tab.selected_session_id)
    }

    pub fn terminal_sessions(&self) -> Vec<SessionSnapshot> {
        self.projects
            .iter()
            .flat_map(|project| project.workspaces.as_deref().unwrap_or_default())
            .flat_map(|workspace| &workspace.tabs)
            .flat_map(|tab| tab.sessions.iter().cloned())
            .collect()
    }

    pub fn update_session_title(&mut self, session_id: Uuid, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        for project in &mut self.projects {
            let Some(workspaces) = project.workspaces.as_mut() else {
                continue;
            };
            for workspace in workspaces {
                for tab in &mut workspace.tabs {
                    if let Some(session) = tab
                        .sessions
                        .iter_mut()
                        .find(|session| session.id == session_id)
                    {
                        if session.title == title {
                            return false;
                        }
                        session.title = title.to_owned();
                        project.normalize();
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn update_session_working_directory(&mut self, session_id: Uuid, path: &Path) -> bool {
        let directory_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Terminal")
            .to_owned();
        let path = path.to_string_lossy().into_owned();

        for project in &mut self.projects {
            let Some(workspaces) = project.workspaces.as_mut() else {
                continue;
            };
            for workspace in workspaces {
                let mut found = false;
                let mut path_changed = false;
                for tab in &mut workspace.tabs {
                    if let Some(session) = tab
                        .sessions
                        .iter_mut()
                        .find(|session| session.id == session_id)
                    {
                        found = true;
                        if session.working_directory != path {
                            session.working_directory = path.clone();
                            path_changed = true;
                        }
                        break;
                    }
                }
                if !found {
                    continue;
                }

                // cmux-style: automatic workspace titles track the primary session cwd.
                let is_primary = workspace
                    .primary_session()
                    .is_some_and(|session| session.id == session_id);
                let auto_title = workspace.title_source != Some(WorkspaceTitleSource::Manual);
                let mut name_changed = false;
                if is_primary && auto_title && workspace.name != directory_name {
                    workspace.name = directory_name;
                    workspace.title_source = Some(WorkspaceTitleSource::Automatic);
                    name_changed = true;
                }

                if path_changed || name_changed {
                    project.normalize();
                    return true;
                }
                return false;
            }
        }
        false
    }

    fn selected_workspace_indices(&self) -> Option<(usize, usize)> {
        let project_index = self
            .projects
            .iter()
            .position(|project| Some(project.id) == self.selected_project_id)?;
        let project = &self.projects[project_index];
        let workspace_index = project
            .workspaces
            .as_ref()?
            .iter()
            .position(|workspace| Some(workspace.id) == project.selected_workspace_id)?;
        Some((project_index, workspace_index))
    }

    fn selected_session_indices(&self) -> Option<(usize, usize, usize, usize)> {
        let (project_index, workspace_index) = self.selected_workspace_indices()?;
        let workspace = &self.projects[project_index].workspaces.as_ref()?[workspace_index];
        let tab_index = workspace
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == workspace.selected_tab_id)?;
        let tab = &workspace.tabs[tab_index];
        let session_index = tab
            .sessions
            .iter()
            .position(|session| Some(session.id) == tab.selected_session_id)?;
        Some((project_index, workspace_index, tab_index, session_index))
    }
}

impl ProjectSnapshot {
    pub fn normalize(&mut self) {
        if self
            .workspaces
            .as_ref()
            .is_none_or(|workspaces| workspaces.is_empty())
        {
            let mut migrated_tabs = self.tabs.clone().unwrap_or_default();
            if migrated_tabs.is_empty() {
                migrated_tabs = self.migrate_legacy_tabs();
            }
            if !migrated_tabs.is_empty() {
                let selected_tab_id = migrated_tabs
                    .iter()
                    .find(|tab| Some(tab.id) == self.selected_tab_id)
                    .or_else(|| {
                        migrated_tabs.iter().find(|tab| {
                            tab.sessions
                                .iter()
                                .any(|session| Some(session.id) == self.selected_session_id)
                        })
                    })
                    .map(|tab| tab.id)
                    .or_else(|| migrated_tabs.first().map(|tab| tab.id));
                self.workspaces = Some(vec![TerminalWorkspaceSnapshot {
                    id: Uuid::new_v4(),
                    name: self.name.clone(),
                    title_source: None,
                    tabs: migrated_tabs,
                    selected_tab_id,
                }]);
            }
        }

        let workspaces = self.workspaces.get_or_insert_default();
        for workspace in workspaces.iter_mut() {
            workspace.normalize();
        }
        workspaces.retain(|workspace| !workspace.tabs.is_empty());

        if workspaces.is_empty() {
            self.selected_workspace_id = None;
            self.tabs = Some(Vec::new());
            self.selected_tab_id = None;
            self.sessions.clear();
            self.selected_session_id = None;
            self.visible_session_ids = Some(Vec::new());
            self.split_axis = None;
            return;
        }

        if !workspaces
            .iter()
            .any(|workspace| Some(workspace.id) == self.selected_workspace_id)
        {
            self.selected_workspace_id = workspaces.first().map(|workspace| workspace.id);
        }

        let selected_workspace = workspaces
            .iter()
            .find(|workspace| Some(workspace.id) == self.selected_workspace_id)
            .unwrap_or(&workspaces[0]);
        let selected_tab = selected_workspace
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == selected_workspace.selected_tab_id)
            .unwrap_or(&selected_workspace.tabs[0]);

        self.sessions = workspaces
            .iter()
            .flat_map(|workspace| workspace.tabs.iter())
            .flat_map(|tab| tab.sessions.iter().cloned())
            .collect();
        self.selected_session_id = selected_tab.selected_session_id;
        self.visible_session_ids = Some(selected_tab.layout.terminal_ids());
        self.split_axis = selected_tab.layout.root_axis();
        self.tabs = Some(selected_workspace.tabs.clone());
        self.selected_tab_id = selected_workspace.selected_tab_id;
    }

    fn migrate_legacy_tabs(&self) -> Vec<TabSnapshot> {
        if self.sessions.is_empty() {
            return Vec::new();
        }
        let visible_ids = self
            .visible_session_ids
            .clone()
            .unwrap_or_else(|| self.selected_session_id.into_iter().collect());
        let visible_sessions: Vec<_> = self
            .sessions
            .iter()
            .filter(|session| visible_ids.contains(&session.id))
            .cloned()
            .collect();
        let mut inserted_group = false;
        let mut tabs = Vec::new();

        for session in &self.sessions {
            if visible_ids.contains(&session.id) {
                if inserted_group {
                    continue;
                }
                inserted_group = true;
                let layouts = visible_sessions
                    .iter()
                    .map(|session| PaneLayoutSnapshot::terminal(session.id))
                    .collect();
                tabs.push(TabSnapshot {
                    id: Uuid::new_v4(),
                    sessions: visible_sessions.clone(),
                    selected_session_id: self
                        .selected_session_id
                        .filter(|id| visible_ids.contains(id))
                        .or_else(|| visible_sessions.first().map(|session| session.id)),
                    zoomed_session_id: None,
                    layout: PaneLayoutSnapshot::joining(
                        layouts,
                        self.split_axis.unwrap_or(WorkspaceSplitAxis::Horizontal),
                    ),
                });
            } else {
                tabs.push(TabSnapshot::with_session(session.clone()));
            }
        }
        tabs
    }
}

impl TerminalWorkspaceSnapshot {
    pub fn normalize(&mut self) {
        for tab in &mut self.tabs {
            tab.normalize();
        }
        self.tabs.retain(|tab| !tab.sessions.is_empty());
        if self.tabs.is_empty() {
            self.selected_tab_id = None;
        } else if !self
            .tabs
            .iter()
            .any(|tab| Some(tab.id) == self.selected_tab_id)
        {
            self.selected_tab_id = self.tabs.first().map(|tab| tab.id);
        }
    }
}

impl TerminalWorkspaceSnapshot {
    /// Working directory of the selected tab's selected session (or first available).
    pub fn primary_working_directory(&self) -> Option<String> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.selected_tab_id)
            .or_else(|| self.tabs.first())?;
        tab.sessions
            .iter()
            .find(|session| Some(session.id) == tab.selected_session_id)
            .or_else(|| tab.sessions.first())
            .map(|session| session.working_directory.clone())
    }

    /// Session that drives sidebar path/branch metadata for this workspace.
    pub fn primary_session(&self) -> Option<&SessionSnapshot> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.selected_tab_id)
            .or_else(|| self.tabs.first())?;
        tab.sessions
            .iter()
            .find(|session| Some(session.id) == tab.selected_session_id)
            .or_else(|| tab.sessions.first())
    }
}

impl TabSnapshot {
    pub fn with_session(session: SessionSnapshot) -> Self {
        let session_id = session.id;
        Self {
            id: Uuid::new_v4(),
            sessions: vec![session],
            selected_session_id: Some(session_id),
            zoomed_session_id: None,
            layout: PaneLayoutSnapshot::terminal(session_id),
        }
    }

    pub fn normalize(&mut self) {
        if self.sessions.is_empty() {
            self.selected_session_id = None;
            self.zoomed_session_id = None;
            return;
        }
        let session_ids: Vec<_> = self.sessions.iter().map(|session| session.id).collect();
        let layout_ids = self.layout.terminal_ids();
        if layout_ids.len() != session_ids.len()
            || !session_ids.iter().all(|id| layout_ids.contains(id))
        {
            self.layout = PaneLayoutSnapshot::joining(
                session_ids
                    .iter()
                    .copied()
                    .map(PaneLayoutSnapshot::terminal)
                    .collect(),
                WorkspaceSplitAxis::Horizontal,
            );
        }
        if !session_ids
            .iter()
            .any(|id| Some(*id) == self.selected_session_id)
        {
            self.selected_session_id = session_ids.first().copied();
        }
        if self
            .zoomed_session_id
            .is_some_and(|id| !session_ids.contains(&id))
        {
            self.zoomed_session_id = None;
        }
        self.layout.normalize();
    }
}

#[derive(Debug, Clone, Copy)]
struct PaneRect {
    id: Uuid,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl PaneRect {
    fn center_x(self) -> f32 {
        self.x + self.width / 2.0
    }

    fn center_y(self) -> f32 {
        self.y + self.height / 2.0
    }
}

impl PaneLayoutSnapshot {
    pub fn terminal(id: Uuid) -> Self {
        Self::Terminal { id }
    }

    pub fn terminal_ids(&self) -> Vec<Uuid> {
        match self {
            Self::Terminal { id } => vec![*id],
            Self::Split { first, second, .. } => {
                let mut ids = first.terminal_ids();
                ids.extend(second.terminal_ids());
                ids
            }
        }
    }

    pub fn root_axis(&self) -> Option<WorkspaceSplitAxis> {
        match self {
            Self::Terminal { .. } => None,
            Self::Split { axis, .. } => Some(*axis),
        }
    }

    pub fn contains_terminal(&self, terminal_id: Uuid) -> bool {
        match self {
            Self::Terminal { id } => *id == terminal_id,
            Self::Split { first, second, .. } => {
                first.contains_terminal(terminal_id) || second.contains_terminal(terminal_id)
            }
        }
    }

    pub fn split_terminal(
        &mut self,
        terminal_id: Uuid,
        new_terminal_id: Uuid,
        axis: WorkspaceSplitAxis,
        insert_first: bool,
    ) -> bool {
        if let Self::Terminal { id } = self {
            if *id != terminal_id {
                return false;
            }
            let existing = Self::terminal(*id);
            let inserted = Self::terminal(new_terminal_id);
            let (first, second) = if insert_first {
                (inserted, existing)
            } else {
                (existing, inserted)
            };
            *self = Self::Split {
                axis,
                ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(first),
                second: Box::new(second),
            };
            return true;
        }

        match self {
            Self::Split { first, second, .. } => {
                first.split_terminal(terminal_id, new_terminal_id, axis, insert_first)
                    || second.split_terminal(terminal_id, new_terminal_id, axis, insert_first)
            }
            Self::Terminal { .. } => false,
        }
    }

    pub fn adjacent_terminal(
        &self,
        terminal_id: Uuid,
        direction: PaneFocusDirection,
    ) -> Option<Uuid> {
        let mut rects = Vec::new();
        self.collect_rects(0.0, 0.0, 1.0, 1.0, &mut rects);
        let current = *rects.iter().find(|rect| rect.id == terminal_id)?;
        rects
            .into_iter()
            .filter(|candidate| candidate.id != terminal_id)
            .filter_map(|candidate| {
                directional_score(current, candidate, direction).map(|score| (candidate.id, score))
            })
            .min_by(|(_, left), (_, right)| left.total_cmp(right))
            .map(|(id, _)| id)
    }

    pub fn move_nearest_divider(
        &mut self,
        terminal_id: Uuid,
        axis: WorkspaceSplitAxis,
        delta: i16,
    ) -> bool {
        let Self::Split {
            axis: split_axis,
            ratio,
            first,
            second,
        } = self
        else {
            return false;
        };
        let child = if first.contains_terminal(terminal_id) {
            first
        } else if second.contains_terminal(terminal_id) {
            second
        } else {
            return false;
        };
        if child.move_nearest_divider(terminal_id, axis, delta) {
            return true;
        }
        if *split_axis != axis {
            return false;
        }
        let adjusted = (*ratio as i32 + i32::from(delta))
            .clamp(MIN_PANE_SPLIT_RATIO as i32, MAX_PANE_SPLIT_RATIO as i32)
            as u16;
        if adjusted == *ratio {
            return false;
        }
        *ratio = adjusted;
        true
    }

    pub fn set_split_ratio(&mut self, path: &[PaneBranch], ratio: u16) -> bool {
        let Self::Split {
            ratio: current,
            first,
            second,
            ..
        } = self
        else {
            return false;
        };
        let Some((branch, remaining)) = path.split_first() else {
            let ratio = ratio.clamp(MIN_PANE_SPLIT_RATIO, MAX_PANE_SPLIT_RATIO);
            if *current == ratio {
                return false;
            }
            *current = ratio;
            return true;
        };
        match branch {
            PaneBranch::First => first.set_split_ratio(remaining, ratio),
            PaneBranch::Second => second.set_split_ratio(remaining, ratio),
        }
    }

    pub fn equalize(&mut self) -> bool {
        match self {
            Self::Terminal { .. } => false,
            Self::Split {
                ratio,
                first,
                second,
                ..
            } => {
                let changed = *ratio != DEFAULT_PANE_SPLIT_RATIO;
                *ratio = DEFAULT_PANE_SPLIT_RATIO;
                first.equalize() | second.equalize() | changed
            }
        }
    }

    fn normalize(&mut self) {
        if let Self::Split {
            ratio,
            first,
            second,
            ..
        } = self
        {
            *ratio = (*ratio).clamp(MIN_PANE_SPLIT_RATIO, MAX_PANE_SPLIT_RATIO);
            first.normalize();
            second.normalize();
        }
    }

    fn collect_rects(&self, x: f32, y: f32, width: f32, height: f32, output: &mut Vec<PaneRect>) {
        match self {
            Self::Terminal { id } => output.push(PaneRect {
                id: *id,
                x,
                y,
                width,
                height,
            }),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let fraction = f32::from(*ratio) / 10_000.0;
                match axis {
                    WorkspaceSplitAxis::Horizontal => {
                        let first_width = width * fraction;
                        first.collect_rects(x, y, first_width, height, output);
                        second.collect_rects(
                            x + first_width,
                            y,
                            width - first_width,
                            height,
                            output,
                        );
                    }
                    WorkspaceSplitAxis::Vertical => {
                        let first_height = height * fraction;
                        first.collect_rects(x, y, width, first_height, output);
                        second.collect_rects(
                            x,
                            y + first_height,
                            width,
                            height - first_height,
                            output,
                        );
                    }
                }
            }
        }
    }

    pub fn removing_terminal(&self, terminal_id: Uuid) -> Option<Self> {
        match self {
            Self::Terminal { id } => (*id != terminal_id).then_some(self.clone()),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => match (
                first.removing_terminal(terminal_id),
                second.removing_terminal(terminal_id),
            ) {
                (None, None) => None,
                (None, Some(remaining)) | (Some(remaining), None) => Some(remaining),
                (Some(first), Some(second)) => Some(Self::Split {
                    axis: *axis,
                    ratio: *ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
            },
        }
    }

    pub fn joining(mut layouts: Vec<Self>, axis: WorkspaceSplitAxis) -> Self {
        assert!(
            !layouts.is_empty(),
            "a pane layout needs at least one terminal"
        );
        let first = layouts.remove(0);
        layouts
            .into_iter()
            .fold(first, |first, second| Self::Split {
                axis,
                ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(first),
                second: Box::new(second),
            })
    }
}

fn directional_score(
    current: PaneRect,
    candidate: PaneRect,
    direction: PaneFocusDirection,
) -> Option<f32> {
    let dx = candidate.center_x() - current.center_x();
    let dy = candidate.center_y() - current.center_y();
    let (primary, orthogonal, overlaps) = match direction {
        PaneFocusDirection::Left if dx < 0.0 => (
            -dx,
            dy.abs(),
            ranges_overlap(current.y, current.height, candidate.y, candidate.height),
        ),
        PaneFocusDirection::Right if dx > 0.0 => (
            dx,
            dy.abs(),
            ranges_overlap(current.y, current.height, candidate.y, candidate.height),
        ),
        PaneFocusDirection::Up if dy < 0.0 => (
            -dy,
            dx.abs(),
            ranges_overlap(current.x, current.width, candidate.x, candidate.width),
        ),
        PaneFocusDirection::Down if dy > 0.0 => (
            dy,
            dx.abs(),
            ranges_overlap(current.x, current.width, candidate.x, candidate.width),
        ),
        _ => return None,
    };
    Some(primary + orthogonal * 0.25 + if overlaps { 0.0 } else { 2.0 })
}

fn ranges_overlap(start: f32, length: f32, other_start: f32, other_length: f32) -> bool {
    start < other_start + other_length && other_start < start + length
}

impl SessionSnapshot {
    pub fn new(working_directory: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: "Terminal".to_owned(),
            working_directory,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(value: &str) -> Uuid {
        Uuid::parse_str(value).unwrap()
    }

    #[test]
    fn terminal_layout_matches_swift_codable_shape() {
        let layout = PaneLayoutSnapshot::terminal(uuid("AAB81C01-C781-4381-90EF-44F986F9DC74"));
        let encoded = serde_json::to_value(&layout).unwrap();

        assert_eq!(
            encoded,
            serde_json::json!({
                "terminal": { "_0": "aab81c01-c781-4381-90ef-44f986f9dc74" }
            })
        );
        assert_eq!(
            serde_json::from_value::<PaneLayoutSnapshot>(encoded).unwrap(),
            layout
        );
    }

    #[test]
    fn split_layout_collapses_after_removing_a_terminal() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let layout = PaneLayoutSnapshot::joining(
            vec![
                PaneLayoutSnapshot::terminal(first_id),
                PaneLayoutSnapshot::terminal(second_id),
            ],
            WorkspaceSplitAxis::Horizontal,
        );

        assert_eq!(
            layout.removing_terminal(first_id),
            Some(PaneLayoutSnapshot::terminal(second_id))
        );
    }

    #[test]
    fn legacy_split_layouts_receive_an_equal_ratio() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let encoded = serde_json::json!({
            "split": {
                "axis": "horizontal",
                "first": { "terminal": { "_0": first_id } },
                "second": { "terminal": { "_0": second_id } }
            }
        });

        let layout: PaneLayoutSnapshot = serde_json::from_value(encoded).unwrap();

        assert!(matches!(
            layout,
            PaneLayoutSnapshot::Split {
                ratio: DEFAULT_PANE_SPLIT_RATIO,
                ..
            }
        ));
    }

    #[test]
    fn split_without_focus_keeps_the_caller_selected() {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/vibra-nofocus"));
        let first_id = snapshot.selected_session().unwrap().id;
        let sibling = snapshot
            .split_selected_terminal_with_focus(PaneSplitDirection::Right, false)
            .unwrap();
        assert_ne!(first_id, sibling);
        assert_eq!(snapshot.selected_session().unwrap().id, first_id);
        assert_eq!(
            snapshot.selected_tab().unwrap().layout.terminal_ids(),
            vec![first_id, sibling]
        );
    }

    #[test]
    fn create_terminal_tab_without_focus_keeps_previous_tab() {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/vibra-tab"));
        let original_tab = snapshot.selected_tab().unwrap().id;
        let (tab_id, session_id) = snapshot.create_terminal_tab_with_focus(false).unwrap();
        assert_ne!(tab_id, original_tab);
        assert_eq!(snapshot.selected_tab().unwrap().id, original_tab);
        let created = snapshot
            .selected_workspace()
            .unwrap()
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .unwrap();
        assert_eq!(created.selected_session_id, Some(session_id));
    }

    #[test]
    fn pane_operations_preserve_geometry_selection_and_zoom() {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/vibra-panes"));
        let first_id = snapshot.selected_session().unwrap().id;
        let right_id = snapshot
            .split_selected_terminal(PaneSplitDirection::Right)
            .unwrap();

        assert_eq!(
            snapshot.selected_tab().unwrap().layout.terminal_ids(),
            vec![first_id, right_id]
        );
        assert!(snapshot.focus_terminal(PaneFocusDirection::Left));
        assert_eq!(snapshot.selected_session().unwrap().id, first_id);
        assert!(snapshot.focus_terminal(PaneFocusDirection::Right));
        assert_eq!(snapshot.selected_session().unwrap().id, right_id);

        let down_id = snapshot
            .split_selected_terminal(PaneSplitDirection::Down)
            .unwrap();
        assert!(snapshot.focus_terminal(PaneFocusDirection::Up));
        assert_eq!(snapshot.selected_session().unwrap().id, right_id);
        assert!(snapshot.focus_terminal(PaneFocusDirection::Down));
        assert_eq!(snapshot.selected_session().unwrap().id, down_id);
        assert!(snapshot.resize_selected_pane(PaneResizeDirection::Down));
        assert!(snapshot.equalize_selected_panes());

        assert!(snapshot.toggle_selected_pane_zoom());
        assert_eq!(
            snapshot.selected_tab().unwrap().zoomed_session_id,
            Some(down_id)
        );
        assert!(snapshot.toggle_selected_pane_zoom());
        assert_eq!(snapshot.selected_tab().unwrap().zoomed_session_id, None);
    }

    #[test]
    fn split_ratios_are_addressed_by_tree_path_and_clamped() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let third_id = Uuid::new_v4();
        let mut layout = PaneLayoutSnapshot::joining(
            vec![
                PaneLayoutSnapshot::terminal(first_id),
                PaneLayoutSnapshot::terminal(second_id),
            ],
            WorkspaceSplitAxis::Horizontal,
        );
        assert!(layout.split_terminal(second_id, third_id, WorkspaceSplitAxis::Vertical, false,));

        assert!(layout.set_split_ratio(&[PaneBranch::Second], u16::MAX));

        let PaneLayoutSnapshot::Split { second, .. } = layout else {
            panic!("expected root split")
        };
        assert!(matches!(
            *second,
            PaneLayoutSnapshot::Split {
                ratio: MAX_PANE_SPLIT_RATIO,
                ..
            }
        ));
    }

    #[test]
    fn closing_a_tab_removes_every_pane_in_it() {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/vibra-close-tab"));
        snapshot
            .split_selected_terminal(PaneSplitDirection::Right)
            .unwrap();
        let tab_id = snapshot.selected_tab().unwrap().id;

        assert!(snapshot.close_tab(tab_id));

        assert!(snapshot.projects.is_empty());
        assert!(snapshot.terminal_sessions().is_empty());
    }

    #[test]
    fn normalization_repairs_stale_selection() {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/vibra-gpui-test"));
        snapshot.selected_project_id = Some(Uuid::new_v4());

        snapshot.normalize();

        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.selected_project_id, Some(snapshot.projects[0].id));
        assert!(snapshot.selected_session().is_some());
    }

    #[test]
    fn legacy_sessions_migrate_without_data_loss() {
        let session = SessionSnapshot::new("/tmp/vibra-legacy".into());
        let session_id = session.id;
        let mut snapshot = WorkspaceSnapshot {
            schema_version: 0,
            projects: vec![ProjectSnapshot {
                id: Uuid::new_v4(),
                name: "Legacy".into(),
                root_path: "/tmp/vibra-legacy".into(),
                sessions: vec![session],
                selected_session_id: Some(session_id),
                visible_session_ids: Some(vec![session_id]),
                split_axis: None,
                tabs: None,
                selected_tab_id: None,
                workspaces: None,
                selected_workspace_id: None,
            }],
            selected_project_id: None,
        };

        snapshot.normalize();

        assert_eq!(snapshot.projects[0].sessions.len(), 1);
        assert_eq!(snapshot.selected_session().unwrap().id, session_id);
    }

    #[test]
    fn workspace_entries_surface_the_selected_session_working_directory() {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/vibra-sidebar"));
        let session_id = snapshot.selected_session().unwrap().id;
        assert!(
            snapshot.update_session_working_directory(
                session_id,
                Path::new("/tmp/vibra-sidebar/nested")
            )
        );

        let entries = snapshot.workspace_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].working_directory, "/tmp/vibra-sidebar/nested");
        // Automatic titles follow the primary session basename after `cd`.
        assert_eq!(entries[0].workspace_name, "nested");
        assert_eq!(
            snapshot.selected_workspace().unwrap().title_source,
            Some(WorkspaceTitleSource::Automatic)
        );
    }

    #[test]
    fn manual_workspace_title_survives_working_directory_changes() {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/vibra-manual-title"));
        let project_id = snapshot.selected_project_id.unwrap();
        let workspace_id = snapshot.selected_workspace().unwrap().id;
        assert!(snapshot.rename_workspace(project_id, workspace_id, "Mi sesión"));
        let session_id = snapshot.selected_session().unwrap().id;

        assert!(snapshot.update_session_working_directory(
            session_id,
            Path::new("/tmp/vibra-manual-title/deep")
        ));
        assert_eq!(snapshot.selected_workspace().unwrap().name, "Mi sesión");
        assert_eq!(
            snapshot.selected_workspace().unwrap().title_source,
            Some(WorkspaceTitleSource::Manual)
        );
    }

    #[test]
    fn terminal_title_updates_the_canonical_and_legacy_views() {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/vibra-title"));
        let session_id = snapshot.selected_session().unwrap().id;

        assert!(snapshot.update_session_title(session_id, "zsh — tests"));
        assert_eq!(snapshot.selected_session().unwrap().title, "zsh — tests");
        assert_eq!(snapshot.projects[0].sessions[0].title, "zsh — tests");
        assert!(!snapshot.update_session_title(session_id, "zsh — tests"));
    }

    #[test]
    fn accidental_root_workspace_relocates_every_session() {
        let target = std::env::temp_dir().join(format!("VibraDev-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&target).unwrap();
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/"));
        snapshot.create_terminal_tab();

        assert!(snapshot.relocate_root(Path::new("/"), &target));

        let project = &snapshot.projects[0];
        assert_eq!(project.root_path, target.to_string_lossy());
        assert_eq!(project.name, target.file_name().unwrap().to_string_lossy());
        assert!(
            snapshot
                .terminal_sessions()
                .iter()
                .all(|session| { session.working_directory == target.to_string_lossy() })
        );
        assert!(!snapshot.relocate_root(Path::new("/"), &target));
        std::fs::remove_dir_all(target).unwrap();
    }
}
