use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::types::*;

impl ProjectSnapshot {
    pub(super) fn new(id: Uuid, name: String, root_path: String) -> Self {
        Self {
            id,
            name,
            name_source: Some(WorkspaceTitleSource::Automatic),
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

    pub fn terminal_sessions(&self) -> impl Iterator<Item = &SessionSnapshot> {
        self.workspaces
            .iter()
            .flatten()
            .flat_map(|workspace| &workspace.tabs)
            .flat_map(|tab| &tab.sessions)
    }

    /// The terminal the space follows: the one selected in its current tab.
    pub fn selected_session(&self) -> Option<&SessionSnapshot> {
        let workspaces = self.workspaces.as_ref()?;
        let workspace = workspaces
            .iter()
            .find(|workspace| Some(workspace.id) == self.selected_workspace_id)
            .or(workspaces.first())?;
        let tab = workspace
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == workspace.selected_tab_id)?;
        tab.sessions
            .iter()
            .find(|session| Some(session.id) == tab.selected_session_id)
    }

    /// Moves the space to `root`, renaming it `name` unless the user named it.
    fn follow_location(&mut self, root: &Path, name: String) -> bool {
        let mut changed = false;
        if self.name_source.is_none() {
            // Saved before spaces: a name other than its folder's was chosen.
            let folder = Path::new(&self.root_path)
                .file_name()
                .and_then(|folder| folder.to_str());
            self.name_source = Some(if folder.is_none_or(|folder| folder == self.name) {
                WorkspaceTitleSource::Automatic
            } else {
                WorkspaceTitleSource::Manual
            });
            changed = true;
        }
        let root = root.to_string_lossy();
        if self.root_path != root {
            self.root_path = root.into_owned();
            changed = true;
        }
        if self.name_source == Some(WorkspaceTitleSource::Automatic) && self.name != name {
            self.name = name;
            changed = true;
        }
        changed
    }
}

impl WorkspaceSnapshot {
    /// A new space in `root`, selected. Unlike `add_project` it never reuses
    /// one: several spaces may be in the same folder.
    pub fn add_space(&mut self, root: &Path, name: String) -> Uuid {
        let id = Uuid::new_v4();
        self.projects.push(ProjectSnapshot::new(
            id,
            name,
            root.to_string_lossy().into_owned(),
        ));
        self.select_project(id);
        self.normalize();
        id
    }

    /// Spaces follow their selected terminal: `locate` maps its cwd to the
    /// space's folder and that folder's name, or `None` when the cwd is gone.
    /// Returns whether a space moved or was renamed.
    pub fn follow_space_locations(
        &mut self,
        mut locate: impl FnMut(&Path) -> Option<(PathBuf, String)>,
    ) -> bool {
        let mut changed = false;
        for project in &mut self.projects {
            let Some(cwd) = project
                .selected_session()
                .map(|session| PathBuf::from(&session.working_directory))
            else {
                continue;
            };
            if let Some((root, name)) = locate(&cwd) {
                changed |= project.follow_location(&root, name);
            }
        }
        changed
    }

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
                .unwrap_or(super::DEFAULT_CONTAINER_NAME)
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
        let Some(project) = self.project_mut(project_id) else {
            return false;
        };
        project.collapsed = false;
        self.selected_project_id = Some(project_id);
        true
    }

    /// The only runtime path for creating tabs. The serialized workspace is
    /// just their container; it is created once and reused until its last tab
    /// closes. Background opens preserve the user's selected project and tab.
    pub fn open_tab_in_project(&mut self, project_id: Uuid, focus: bool) -> Option<(Uuid, Uuid)> {
        let root = self.project(project_id)?.directory()?.to_owned();
        self.open_tab_in_directory(project_id, root, focus)
    }

    /// Like `open_tab_in_project`, starting wherever the caller says: the
    /// home folder for a space whose folder is gone.
    pub fn open_tab_in_directory(
        &mut self,
        project_id: Uuid,
        directory: String,
        focus: bool,
    ) -> Option<(Uuid, Uuid)> {
        let project = self.project_mut(project_id)?;
        let tab = TabSnapshot::with_session(SessionSnapshot::new(directory));
        let ids = (tab.id, tab.selected_session_id?);
        let workspaces = project.workspaces.get_or_insert_default();
        if workspaces.is_empty() {
            workspaces.push(TerminalWorkspaceSnapshot {
                id: Uuid::new_v4(),
                name: project.name.clone(),
                title_source: Some(WorkspaceTitleSource::Automatic),
                tabs: Vec::new(),
                selected_tab_id: None,
                review_tab_index: None,
            });
        }
        let index = workspaces
            .iter()
            .position(|workspace| Some(workspace.id) == project.selected_workspace_id)
            .unwrap_or(0);
        let workspace = &mut workspaces[index];
        workspace.tabs.push(tab);
        if focus {
            workspace.selected_tab_id = Some(ids.0);
            project.selected_workspace_id = Some(workspace.id);
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
        let Some(name) = super::parse_user_name(name) else {
            return false;
        };
        let Some(project) = self.project_mut(project_id) else {
            return false;
        };
        project.name = name.to_owned();
        project.name_source = Some(WorkspaceTitleSource::Manual);
        true
    }

    /// Gives a space back its folder's `name`, which it follows from then on.
    pub fn name_space_after_folder(&mut self, project_id: Uuid, name: String) -> bool {
        let Some(project) = self.project_mut(project_id) else {
            return false;
        };
        project.name = name;
        project.name_source = Some(WorkspaceTitleSource::Automatic);
        true
    }

    /// Associating a folder never rewrites the cwd of an existing terminal.
    /// Spaces find their folder by following their terminal; tests point one.
    #[cfg(test)]
    pub fn set_project_directory(&mut self, project_id: Uuid, root: &Path) -> bool {
        let Some(project) = self.project_mut(project_id) else {
            return false;
        };
        project.root_path = root.to_string_lossy().into_owned();
        true
    }

    /// Spaces live while they have terminals: removes every other one but
    /// `keep`, and returns the removed IDs.
    pub fn remove_empty_spaces(&mut self, keep: Option<Uuid>) -> Vec<Uuid> {
        let removed: Vec<_> = self
            .projects
            .iter()
            .filter(|project| {
                Some(project.id) != keep && project.terminal_sessions().next().is_none()
            })
            .map(|project| project.id)
            .collect();
        if !removed.is_empty() {
            self.projects
                .retain(|project| !removed.contains(&project.id));
            self.normalize();
        }
        removed
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
        let Some(insert_at) = super::relocate_index(from, to) else {
            return false;
        };
        let project = self.projects.remove(from);
        self.projects.insert(insert_at, project);
        self.normalize();
        true
    }
}
