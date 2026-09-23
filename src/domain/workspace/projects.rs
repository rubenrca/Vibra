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

    pub fn toggle_project(&mut self, project_id: Uuid) -> bool {
        let Some(project) = self.projects.iter_mut().find(|p| p.id == project_id) else {
            return false;
        };
        project.collapsed = !project.collapsed;
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

    fn workspace_location(&self, id: Uuid) -> Option<(usize, usize)> {
        self.projects.iter().enumerate().find_map(|(pi, project)| {
            project
                .workspaces
                .as_ref()?
                .iter()
                .position(|w| w.id == id)
                .map(|wi| (pi, wi))
        })
    }

    /// Moving a session changes ownership, never its terminals or files.
    pub fn move_workspace_to_project(&mut self, workspace_id: Uuid, project_id: Uuid) -> bool {
        let Some((source, index)) = self.workspace_location(workspace_id) else {
            return false;
        };
        let Some(target) = self.projects.iter().position(|p| p.id == project_id) else {
            return false;
        };
        if source == target {
            return false;
        }
        let selected = self
            .selected_workspace()
            .is_some_and(|w| w.id == workspace_id);
        let workspace = self.projects[source]
            .workspaces
            .as_mut()
            .expect("normalized")
            .remove(index);
        self.projects[target]
            .workspaces
            .get_or_insert_default()
            .push(workspace);
        self.projects[target].collapsed = false;
        if selected {
            self.projects[target].selected_workspace_id = Some(workspace_id);
            self.selected_project_id = Some(project_id);
        }
        self.normalize();
        true
    }

    /// Reorders within a project; a target in another project also transfers ownership.
    pub fn move_workspace(&mut self, workspace_id: Uuid, before_id: Option<Uuid>) -> bool {
        match before_id {
            Some(target) => self.move_workspace_relative(workspace_id, target, false),
            None => {
                let Some((pi, _)) = self.workspace_location(workspace_id) else {
                    return false;
                };
                let Some(last) = self.projects[pi]
                    .workspaces
                    .as_ref()
                    .and_then(|ws| ws.last())
                else {
                    return false;
                };
                self.move_workspace_relative(workspace_id, last.id, true)
            }
        }
    }

    pub fn move_workspace_relative(
        &mut self,
        workspace_id: Uuid,
        target_id: Uuid,
        after: bool,
    ) -> bool {
        if workspace_id == target_id {
            return false;
        }
        let Some((source, from)) = self.workspace_location(workspace_id) else {
            return false;
        };
        let Some((target, target_index)) = self.workspace_location(target_id) else {
            return false;
        };
        let to = target_index + usize::from(after);
        if source == target && (from == to || from + 1 == to) {
            return false;
        }
        let selected = self
            .selected_workspace()
            .is_some_and(|w| w.id == workspace_id);
        let workspace = self.projects[source]
            .workspaces
            .as_mut()
            .expect("normalized")
            .remove(from);
        let insert_at = if source == target && from < to {
            to - 1
        } else {
            to
        };
        self.projects[target]
            .workspaces
            .as_mut()
            .expect("normalized")
            .insert(insert_at, workspace);
        self.projects[target].collapsed = false;
        if selected {
            self.projects[target].selected_workspace_id = Some(workspace_id);
            self.selected_project_id = Some(self.projects[target].id);
        }
        self.normalize();
        true
    }
}
