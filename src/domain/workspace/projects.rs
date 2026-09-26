use std::path::Path;

use uuid::Uuid;

use super::types::*;

impl ProjectSnapshot {
    pub(super) fn new(id: Uuid, name: String, root_path: String) -> Self {
        Self {
            id,
            name,
            root_path,
            collapsed: false,
            sessions: Vec::new(),
            selected_session_id: None,
            visible_session_ids: None,
            split_axis: None,
            tabs: None,
            selected_tab_id: None,
            workspaces: Some(Vec::new()),
            selected_workspace_id: None,
        }
    }

    pub fn directory(&self) -> Option<&str> {
        (!self.root_path.is_empty()).then_some(self.root_path.as_str())
    }
}

impl WorkspaceSnapshot {
    /// Register a folder independently of its sessions. Reopening it selects it.
    pub fn add_project(&mut self, root: &Path) -> Uuid {
        let root_path = root.to_string_lossy().into_owned();
        let id = if let Some(project) = self.projects.iter().find(|p| p.root_path == root_path) {
            project.id
        } else {
            let id = Uuid::new_v4();
            let name = root
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("Terminal")
                .to_owned();
            self.projects
                .push(ProjectSnapshot::new(id, name, root_path));
            id
        };
        self.select_project(id);
        self.normalize();
        id
    }

    pub fn select_project(&mut self, project_id: Uuid) -> bool {
        let Some(project) = self.projects.iter_mut().find(|p| p.id == project_id) else {
            return false;
        };
        project.collapsed = false;
        self.selected_project_id = Some(project_id);
        true
    }

    pub fn create_workspace_in_project(&mut self, project_id: Uuid) -> Option<Uuid> {
        let project = self.projects.iter_mut().find(|p| p.id == project_id)?;
        let root = project.directory()?.to_owned();
        let tab = TabSnapshot::with_session(SessionSnapshot::new(root));
        let workspace_id = Uuid::new_v4();
        project
            .workspaces
            .get_or_insert_default()
            .push(TerminalWorkspaceSnapshot {
                id: workspace_id,
                name: project.name.clone(),
                title_source: Some(WorkspaceTitleSource::Automatic),
                selected_tab_id: Some(tab.id),
                tabs: vec![tab],
            });
        project.selected_workspace_id = Some(workspace_id);
        project.collapsed = false;
        self.selected_project_id = Some(project_id);
        self.normalize();
        Some(workspace_id)
    }

    /// Opens a new terminal tab in the project's session, creating the
    /// session when the project has none. With `focus` the project and the
    /// new tab are selected; otherwise the user's current selection stays.
    pub fn open_tab_in_project(&mut self, project_id: Uuid, focus: bool) -> Option<(Uuid, Uuid)> {
        let project_index = self.projects.iter().position(|p| p.id == project_id)?;
        let root = self.projects[project_index].directory()?.to_owned();
        let has_session = self.projects[project_index]
            .workspaces
            .as_ref()
            .is_some_and(|workspaces| !workspaces.is_empty());
        if !has_session {
            let previous_project = self.selected_project_id;
            let workspace_id = self.create_workspace_in_project(project_id)?;
            let project = &self.projects[project_index];
            let workspace = project
                .workspaces
                .as_ref()?
                .iter()
                .find(|workspace| workspace.id == workspace_id)?;
            let tab = workspace.tabs.first()?;
            let ids = (tab.id, tab.selected_session_id?);
            if !focus {
                self.selected_project_id = previous_project;
            }
            return Some(ids);
        }
        let project = &mut self.projects[project_index];
        let workspace_id = project
            .selected_workspace_id
            .or_else(|| project.workspaces.as_ref()?.first().map(|item| item.id))?;
        let workspace = project
            .workspaces
            .as_mut()?
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)?;
        let tab = TabSnapshot::with_session(SessionSnapshot::new(root));
        let ids = (tab.id, tab.selected_session_id?);
        workspace.tabs.push(tab);
        if focus {
            workspace.selected_tab_id = Some(ids.0);
            project.selected_workspace_id = Some(workspace_id);
            project.collapsed = false;
            self.selected_project_id = Some(project_id);
        }
        self.normalize();
        Some(ids)
    }

    /// Projects used to hold several sessions, each with its own tabs. The
    /// app now shows one row of tabs per project, so earlier sessions are
    /// merged into the selected one instead of staying out of reach.
    pub fn consolidate_project_sessions(&mut self) -> bool {
        let mut changed = false;
        for project in &mut self.projects {
            let Some(workspaces) = project.workspaces.as_mut() else {
                continue;
            };
            if workspaces.len() < 2 {
                continue;
            }
            let primary = project
                .selected_workspace_id
                .and_then(|id| workspaces.iter().position(|workspace| workspace.id == id))
                .unwrap_or(0);
            let mut merged = workspaces.remove(primary);
            for workspace in workspaces.drain(..) {
                merged.tabs.extend(workspace.tabs);
            }
            merged.name = project.name.clone();
            merged.title_source = Some(WorkspaceTitleSource::Automatic);
            project.selected_workspace_id = Some(merged.id);
            workspaces.push(merged);
            changed = true;
        }
        if changed {
            self.normalize();
        }
        changed
    }

    pub fn rename_project(&mut self, project_id: Uuid, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > super::MAX_NAME_CHARS {
            return false;
        }
        let Some(project) = self.projects.iter_mut().find(|p| p.id == project_id) else {
            return false;
        };
        project.name = name.to_owned();
        true
    }

    /// Associating a folder never rewrites the cwd of an existing terminal.
    pub fn set_project_directory(&mut self, project_id: Uuid, root: &Path) -> bool {
        let Some(project) = self.projects.iter_mut().find(|p| p.id == project_id) else {
            return false;
        };
        project.root_path = root.to_string_lossy().into_owned();
        true
    }

    /// Removes only app state; no folder or repository is deleted.
    pub fn remove_project(&mut self, project_id: Uuid) -> bool {
        let Some(index) = self.projects.iter().position(|p| p.id == project_id) else {
            return false;
        };
        self.projects.remove(index);
        self.normalize();
        true
    }

    pub fn move_project(&mut self, project_id: Uuid, before_id: Option<Uuid>) -> bool {
        let Some(from) = self.projects.iter().position(|p| p.id == project_id) else {
            return false;
        };
        let to = match before_id {
            Some(id) => match self.projects.iter().position(|p| p.id == id) {
                Some(index) => index,
                None => return false,
            },
            None => self.projects.len(),
        };
        if from == to || from + 1 == to {
            return false;
        }
        let project = self.projects.remove(from);
        self.projects
            .insert(if from < to { to - 1 } else { to }, project);
        self.normalize();
        true
    }
}
