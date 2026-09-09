use std::collections::{HashMap, HashSet};
use std::path::Path;

use uuid::Uuid;

use super::types::*;

impl super::WorkspaceSnapshot {
    pub fn create_workspace(&mut self, root: &Path) {
        let selected_workspace_id = self.selected_workspace().map(|workspace| workspace.id);
        let inherited_space_id = selected_workspace_id.and_then(|selected_workspace_id| {
            self.sidebar_items.iter().find_map(|item| match item {
                SidebarItemSnapshot::Space {
                    id, workspace_ids, ..
                } if workspace_ids.contains(&selected_workspace_id) => Some(*id),
                _ => None,
            })
        });
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
        let new_workspace_id = workspace.id;

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
        if let Some(space_id) = inherited_space_id {
            self.move_workspace_to_space(new_workspace_id, space_id);
        }
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

    pub fn create_terminal_tab_with_options(
        &mut self,
        focus_new: bool,
        working_directory: Option<String>,
    ) -> Option<(Uuid, Uuid)> {
        let (project_index, workspace_index) = self.selected_workspace_indices()?;
        let working_directory = working_directory.unwrap_or_else(|| {
            self.selected_session()
                .map(|session| session.working_directory.clone())
                .unwrap_or_else(|| self.projects[project_index].root_path.clone())
        });
        let project = &mut self.projects[project_index];
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
    /// remains selected.
    pub fn split_selected_terminal_with_focus(
        &mut self,
        direction: PaneSplitDirection,
        focus_new: bool,
    ) -> Option<Uuid> {
        let (project_index, workspace_index, tab_index, session_index) =
            self.selected_session_indices()?;
        let project = &mut self.projects[project_index];
        let tab =
            &mut project.workspaces.as_mut().expect("normalized")[workspace_index].tabs[tab_index];
        let selected_id = tab.sessions[session_index].id;
        let working_directory = tab.sessions[session_index].working_directory.clone();
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
        for project in &mut self.projects {
            for workspace in project.workspaces.iter_mut().flatten() {
                let Some(tab) = workspace
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.sessions.iter().any(|session| session.id == session_id))
                else {
                    continue;
                };
                tab.selected_session_id = Some(session_id);
                workspace.selected_tab_id = Some(tab.id);
                project.selected_workspace_id = Some(workspace.id);
                self.selected_project_id = Some(project.id);
                project.normalize();
                return true;
            }
        }
        false
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

    pub fn swap_tab_terminals(&mut self, first: Uuid, second: Uuid) -> bool {
        if first == second {
            return false;
        }
        let Some((project_index, workspace_index, tab_index, _)) = self.selected_session_indices()
        else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let tab =
            &mut project.workspaces.as_mut().expect("normalized")[workspace_index].tabs[tab_index];
        if !tab.layout.swap_terminals(first, second) {
            return false;
        }
        tab.selected_session_id = Some(first);
        tab.zoomed_session_id = None;
        project.normalize();
        true
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

    /// Moves a workspace so it sits before `before_workspace_id`, or at the
    /// end of the sidebar when `before_workspace_id` is `None`.
    pub fn move_workspace(
        &mut self,
        workspace_id: Uuid,
        before_workspace_id: Option<Uuid>,
    ) -> bool {
        if before_workspace_id == Some(workspace_id) {
            return false;
        }
        let Some(from_order) = self
            .workspace_order
            .iter()
            .position(|id| *id == workspace_id)
        else {
            return false;
        };
        let to_order = match before_workspace_id {
            Some(target_id) => {
                let Some(index) = self.workspace_order.iter().position(|id| *id == target_id)
                else {
                    return false;
                };
                index
            }
            None => self.workspace_order.len(),
        };
        let space_for = |workspace_id: Uuid| {
            self.sidebar_items.iter().find_map(|item| match item {
                SidebarItemSnapshot::Space {
                    id, workspace_ids, ..
                } if workspace_ids.contains(&workspace_id) => Some(*id),
                _ => None,
            })
        };
        let source_space = space_for(workspace_id);
        let target_space = before_workspace_id.and_then(space_for);
        if from_order == to_order
            || (from_order + 1 == to_order && source_space == target_space)
            || (before_workspace_id.is_none()
                && from_order + 1 == self.workspace_order.len()
                && source_space.is_none())
        {
            return false;
        }
        let Some(source_item_index) = self.sidebar_items.iter().position(|item| match item {
            SidebarItemSnapshot::Workspace { workspace_id: id } => *id == workspace_id,
            SidebarItemSnapshot::Space { workspace_ids, .. } => {
                workspace_ids.contains(&workspace_id)
            }
            SidebarItemSnapshot::Spacer { .. } => false,
        }) else {
            return false;
        };

        match &mut self.sidebar_items[source_item_index] {
            SidebarItemSnapshot::Workspace { .. } => {
                self.sidebar_items.remove(source_item_index);
            }
            SidebarItemSnapshot::Space { workspace_ids, .. } => {
                workspace_ids.retain(|id| *id != workspace_id);
            }
            SidebarItemSnapshot::Spacer { .. } => unreachable!(),
        }

        let mut inserted = false;
        if let Some(target_id) = before_workspace_id {
            for item in &mut self.sidebar_items {
                match item {
                    SidebarItemSnapshot::Workspace { workspace_id: id } if *id == target_id => {
                        break;
                    }
                    SidebarItemSnapshot::Space { workspace_ids, .. } => {
                        if let Some(index) = workspace_ids.iter().position(|id| *id == target_id) {
                            workspace_ids.insert(index, workspace_id);
                            inserted = true;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if !inserted {
                let Some(index) = self.sidebar_items.iter().position(|item| {
                    matches!(item, SidebarItemSnapshot::Workspace { workspace_id: id } if *id == target_id)
                }) else {
                    self.normalize();
                    return false;
                };
                self.sidebar_items
                    .insert(index, SidebarItemSnapshot::Workspace { workspace_id });
            }
        } else {
            self.sidebar_items
                .push(SidebarItemSnapshot::Workspace { workspace_id });
        }
        self.sidebar_items.retain(|item| {
            !matches!(item, SidebarItemSnapshot::Space { workspace_ids, .. } if workspace_ids.is_empty())
        });
        self.workspace_order = self
            .sidebar_items
            .iter()
            .flat_map(|item| match item {
                SidebarItemSnapshot::Workspace { workspace_id } => vec![*workspace_id],
                SidebarItemSnapshot::Space { workspace_ids, .. } => workspace_ids.clone(),
                SidebarItemSnapshot::Spacer { .. } => Vec::new(),
            })
            .collect();
        true
    }

    /// Moves a workspace immediately before or after a target while preserving
    /// the target's space membership. This powers directional sidebar drops.
    pub fn move_workspace_relative(
        &mut self,
        workspace_id: Uuid,
        target_id: Uuid,
        place_after: bool,
    ) -> bool {
        if workspace_id == target_id {
            return false;
        }
        let original = self.sidebar_items.clone();
        let source_exists = self.sidebar_items.iter().any(|item| match item {
            SidebarItemSnapshot::Workspace { workspace_id: id } => *id == workspace_id,
            SidebarItemSnapshot::Space { workspace_ids, .. } => {
                workspace_ids.contains(&workspace_id)
            }
            SidebarItemSnapshot::Spacer { .. } => false,
        });
        let target_exists = self.sidebar_items.iter().any(|item| match item {
            SidebarItemSnapshot::Workspace { workspace_id: id } => *id == target_id,
            SidebarItemSnapshot::Space { workspace_ids, .. } => workspace_ids.contains(&target_id),
            SidebarItemSnapshot::Spacer { .. } => false,
        });
        if !source_exists || !target_exists {
            return false;
        }

        if let Some(index) = self.sidebar_items.iter().position(
            |item| matches!(item, SidebarItemSnapshot::Workspace { workspace_id: id } if *id == workspace_id),
        ) {
            self.sidebar_items.remove(index);
        } else {
            for item in &mut self.sidebar_items {
                if let SidebarItemSnapshot::Space { workspace_ids, .. } = item {
                    workspace_ids.retain(|id| *id != workspace_id);
                }
            }
        }

        if let Some(index) = self.sidebar_items.iter().position(
            |item| matches!(item, SidebarItemSnapshot::Workspace { workspace_id: id } if *id == target_id),
        ) {
            self.sidebar_items.insert(
                index + usize::from(place_after),
                SidebarItemSnapshot::Workspace { workspace_id },
            );
        } else {
            let Some(workspace_ids) = self.sidebar_items.iter_mut().find_map(|item| match item {
                SidebarItemSnapshot::Space { workspace_ids, .. }
                    if workspace_ids.contains(&target_id) =>
                {
                    Some(workspace_ids)
                }
                _ => None,
            }) else {
                self.sidebar_items = original;
                return false;
            };
            let target_index = workspace_ids
                .iter()
                .position(|id| *id == target_id)
                .expect("target membership checked");
            workspace_ids.insert(target_index + usize::from(place_after), workspace_id);
        }
        self.normalize();
        self.sidebar_items != original
    }

    pub fn create_sidebar_space(&mut self, workspace_id: Uuid, name: &str) -> Option<Uuid> {
        let item_index = self.sidebar_items.iter().position(|item| match item {
            SidebarItemSnapshot::Workspace { workspace_id: id } => *id == workspace_id,
            SidebarItemSnapshot::Space { workspace_ids, .. } => {
                workspace_ids.contains(&workspace_id)
            }
            SidebarItemSnapshot::Spacer { .. } => false,
        })?;
        let insert_at = match &mut self.sidebar_items[item_index] {
            SidebarItemSnapshot::Workspace { .. } => {
                self.sidebar_items.remove(item_index);
                item_index
            }
            SidebarItemSnapshot::Space { workspace_ids, .. } => {
                workspace_ids.retain(|id| *id != workspace_id);
                item_index + 1
            }
            SidebarItemSnapshot::Spacer { .. } => unreachable!(),
        };
        let id = Uuid::new_v4();
        self.sidebar_items.insert(
            insert_at,
            SidebarItemSnapshot::Space {
                id,
                name: name.trim().to_owned(),
                collapsed: false,
                workspace_ids: vec![workspace_id],
            },
        );
        self.normalize();
        Some(id)
    }

    pub fn create_empty_sidebar_space(&mut self, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.sidebar_items.push(SidebarItemSnapshot::Space {
            id,
            name: name.trim().to_owned(),
            collapsed: false,
            workspace_ids: Vec::new(),
        });
        self.normalize();
        id
    }

    pub fn move_workspace_to_space(&mut self, workspace_id: Uuid, space_id: Uuid) -> bool {
        let source_space_id = self.sidebar_items.iter().find_map(|item| match item {
            SidebarItemSnapshot::Space {
                id, workspace_ids, ..
            } if workspace_ids.contains(&workspace_id) => Some(*id),
            _ => None,
        });
        if source_space_id == Some(space_id) {
            return false;
        }
        let workspace_exists = self.sidebar_items.iter().any(|item| match item {
            SidebarItemSnapshot::Workspace { workspace_id: id } => *id == workspace_id,
            SidebarItemSnapshot::Space { workspace_ids, .. } => {
                workspace_ids.contains(&workspace_id)
            }
            SidebarItemSnapshot::Spacer { .. } => false,
        });
        let target_exists = self
            .sidebar_items
            .iter()
            .any(|item| matches!(item, SidebarItemSnapshot::Space { id, .. } if *id == space_id));
        if !workspace_exists || !target_exists {
            return false;
        }
        if let Some(index) = self.sidebar_items.iter().position(
            |item| matches!(item, SidebarItemSnapshot::Workspace { workspace_id: id } if *id == workspace_id),
        ) {
            self.sidebar_items.remove(index);
        }
        for item in &mut self.sidebar_items {
            if let SidebarItemSnapshot::Space { workspace_ids, .. } = item {
                workspace_ids.retain(|id| *id != workspace_id);
            }
        }
        let Some(SidebarItemSnapshot::Space {
            workspace_ids,
            collapsed,
            ..
        }) = self
            .sidebar_items
            .iter_mut()
            .find(|item| matches!(item, SidebarItemSnapshot::Space { id, .. } if *id == space_id))
        else {
            self.normalize();
            return false;
        };
        workspace_ids.push(workspace_id);
        *collapsed = false;
        self.normalize();
        true
    }

    pub fn rename_sidebar_space(&mut self, space_id: Uuid, name: &str) -> bool {
        let Some(SidebarItemSnapshot::Space { name: current, .. }) = self
            .sidebar_items
            .iter_mut()
            .find(|item| matches!(item, SidebarItemSnapshot::Space { id, .. } if *id == space_id))
        else {
            return false;
        };
        *current = name.trim().to_owned();
        true
    }

    pub fn toggle_sidebar_space(&mut self, space_id: Uuid) -> bool {
        let Some(SidebarItemSnapshot::Space { collapsed, .. }) = self
            .sidebar_items
            .iter_mut()
            .find(|item| matches!(item, SidebarItemSnapshot::Space { id, .. } if *id == space_id))
        else {
            return false;
        };
        *collapsed = !*collapsed;
        true
    }

    /// Removes only the group; its sessions return to the ungrouped sidebar.
    pub fn remove_sidebar_space(&mut self, space_id: Uuid) -> bool {
        let Some(index) = self.sidebar_items.iter().position(
            |item| matches!(item, SidebarItemSnapshot::Space { id, .. } if *id == space_id),
        ) else {
            return false;
        };
        let SidebarItemSnapshot::Space { workspace_ids, .. } = self.sidebar_items.remove(index)
        else {
            unreachable!();
        };
        for (offset, workspace_id) in workspace_ids.into_iter().enumerate() {
            self.sidebar_items.insert(
                index + offset,
                SidebarItemSnapshot::Workspace { workspace_id },
            );
        }
        self.normalize();
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

    /// Selects a tab by Ghostty-style number: `1..=8` go to that index (or the
    /// last tab if there aren't enough), and `9` always goes to the last tab.
    pub fn select_tab_number(&mut self, number: usize) -> bool {
        if number == 0 {
            return false;
        }
        let Some((project_index, workspace_index)) = self.selected_workspace_indices() else {
            return false;
        };
        let workspace = &self.projects[project_index]
            .workspaces
            .as_ref()
            .expect("normalized")[workspace_index];
        if workspace.tabs.is_empty() {
            return false;
        }
        let index = if number >= 9 {
            workspace.tabs.len() - 1
        } else {
            number.saturating_sub(1).min(workspace.tabs.len() - 1)
        };
        let tab_id = workspace.tabs[index].id;
        if workspace.selected_tab_id == Some(tab_id) {
            return false;
        }
        self.select_tab(tab_id)
    }

    /// Moves `tab_id` so it sits before `before_tab_id`, or at the end when
    /// `before_tab_id` is `None`.
    pub fn move_tab(&mut self, tab_id: Uuid, before_tab_id: Option<Uuid>) -> bool {
        if before_tab_id == Some(tab_id) {
            return false;
        }
        let Some((project_index, workspace_index)) = self.selected_workspace_indices() else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let workspace = &mut project.workspaces.as_mut().expect("normalized")[workspace_index];
        let Some(from) = workspace.tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        let to = match before_tab_id {
            Some(target_id) => {
                let Some(index) = workspace.tabs.iter().position(|tab| tab.id == target_id) else {
                    return false;
                };
                index
            }
            None => workspace.tabs.len(),
        };
        if from == to || from + 1 == to {
            return false;
        }
        let tab = workspace.tabs.remove(from);
        let insert_at = if from < to { to - 1 } else { to };
        workspace.tabs.insert(insert_at, tab);
        workspace.selected_tab_id = Some(tab_id);
        project.normalize();
        true
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
        let mut entries: Vec<_> = self
            .projects
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
            .collect();
        let positions: HashMap<_, _> = self
            .workspace_order
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index))
            .collect();
        entries.sort_by_key(|entry| {
            positions
                .get(&entry.workspace_id)
                .copied()
                .unwrap_or(usize::MAX)
        });
        entries
    }

    pub fn sidebar_entries(&self) -> Vec<SidebarEntry> {
        let entries: HashMap<_, _> = self
            .workspace_entries()
            .into_iter()
            .map(|entry| (entry.workspace_id, entry))
            .collect();
        self.sidebar_items
            .iter()
            .flat_map(|item| match item {
                SidebarItemSnapshot::Workspace { workspace_id } => entries
                    .get(workspace_id)
                    .cloned()
                    .map(|entry| SidebarEntry::Workspace {
                        entry,
                        space_id: None,
                    })
                    .into_iter()
                    .collect(),
                SidebarItemSnapshot::Space {
                    id,
                    name,
                    collapsed,
                    workspace_ids,
                } => {
                    let mut result = vec![SidebarEntry::Space {
                        id: *id,
                        name: name.clone(),
                        collapsed: *collapsed,
                        workspace_count: workspace_ids.len(),
                    }];
                    if !collapsed {
                        result.extend(workspace_ids.iter().filter_map(|workspace_id| {
                            entries.get(workspace_id).cloned().map(|entry| {
                                SidebarEntry::Workspace {
                                    entry,
                                    space_id: Some(*id),
                                }
                            })
                        }));
                    }
                    result
                }
                SidebarItemSnapshot::Spacer { .. } => Vec::new(),
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

    /// Sessions that should be painted (selected tab, or the zoomed pane).
    pub fn painted_session_ids(&self) -> HashSet<Uuid> {
        let Some(tab) = self.selected_tab() else {
            return HashSet::new();
        };
        if let Some(zoomed) = tab.zoomed_session_id {
            return HashSet::from([zoomed]);
        }
        tab.sessions.iter().map(|session| session.id).collect()
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

    pub fn update_agent_task_title(&mut self, session_id: Uuid, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        let Some(session) = self
            .projects
            .iter_mut()
            .flat_map(|project| project.workspaces.iter_mut().flatten())
            .flat_map(|workspace| &mut workspace.tabs)
            .flat_map(|tab| &mut tab.sessions)
            .find(|session| session.id == session_id)
        else {
            return false;
        };
        if session.agent_task_title.as_deref() == Some(title) {
            return false;
        }
        session.agent_task_title = Some(title.chars().take(80).collect());
        true
    }

    pub fn update_session_title(&mut self, session_id: Uuid, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        for project in &mut self.projects {
            let Some(session) = project
                .workspaces
                .iter_mut()
                .flatten()
                .flat_map(|workspace| &mut workspace.tabs)
                .flat_map(|tab| &mut tab.sessions)
                .find(|session| session.id == session_id)
            else {
                continue;
            };
            if session.title == title {
                return false;
            }
            session.title = title.to_owned();
            project.normalize();
            return true;
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
            for workspace in project.workspaces.iter_mut().flatten() {
                let Some(session) = workspace
                    .tabs
                    .iter_mut()
                    .flat_map(|tab| &mut tab.sessions)
                    .find(|session| session.id == session_id)
                else {
                    continue;
                };
                let path_changed = session.working_directory != path;
                if path_changed {
                    session.working_directory = path.clone();
                }

                // Automatic workspace titles track the primary session cwd.
                let is_primary = workspace
                    .primary_session()
                    .is_some_and(|session| session.id == session_id);
                let auto_title = workspace.title_source != Some(WorkspaceTitleSource::Manual);
                let name_changed = is_primary && auto_title && workspace.name != directory_name;
                if name_changed {
                    workspace.name = directory_name;
                    workspace.title_source = Some(WorkspaceTitleSource::Automatic);
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

impl SessionSnapshot {
    pub fn new(working_directory: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: "Terminal".to_owned(),
            agent_task_title: None,
            working_directory,
        }
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
}
