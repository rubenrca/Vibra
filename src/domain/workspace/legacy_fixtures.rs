//! Constructors and operations for pre-redesign fixtures. They deliberately
//! allow multiple workspaces per project to exercise migration and restoration;
//! runtime code can only open tabs through `open_tab_in_project`.

use super::*;
use std::path::Path;
use uuid::Uuid;

impl WorkspaceSnapshot {
    pub fn create_workspace(&mut self, root: &Path) {
        let project_id = self.add_project(root);
        self.create_workspace_in_project(project_id);
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

    /// Sessions are merged into one per project in the app; kept for
    /// fixtures that build older, multi-session snapshots.
    pub fn rename_workspace(&mut self, project_id: Uuid, workspace_id: Uuid, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > super::MAX_NAME_CHARS {
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

    /// Sessions are merged into one per project in the app; kept for
    /// fixtures that build older, multi-session snapshots.
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
        self.normalize();
        true
    }

    /// Sessions are merged into one per project in the app; kept for
    /// fixtures that build older, multi-session snapshots.
    pub fn cycle_workspace(&mut self, offset: isize) -> bool {
        let entries: Vec<_> = self
            .projects
            .iter()
            .flat_map(|project| {
                project.workspaces.iter().flatten().map(|workspace| {
                    (
                        project.id,
                        workspace.id,
                        self.selected_project_id == Some(project.id)
                            && project.selected_workspace_id == Some(workspace.id),
                    )
                })
            })
            .collect();
        if entries.is_empty() || offset == 0 {
            return false;
        }
        let current = entries
            .iter()
            .position(|(_, _, selected)| *selected)
            .unwrap_or(0);
        let next = (current + offset.rem_euclid(entries.len() as isize) as usize) % entries.len();
        let (project_id, workspace_id, _) = entries[next];
        self.select_workspace(project_id, workspace_id)
    }
}
