use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use super::types::*;

impl super::WorkspaceSnapshot {
    pub fn normalize(&mut self) {
        if self.schema_version < 7 {
            for project in &mut self.projects {
                project.normalize();
            }
            self.migrate_sidebar_projects();
        }
        for project in &mut self.projects {
            project.normalize();
        }
        if !self
            .projects
            .iter()
            .any(|p| Some(p.id) == self.selected_project_id)
        {
            self.selected_project_id = self.projects.first().map(|p| p.id);
        }
        self.workspace_order = self
            .projects
            .iter()
            .flat_map(|p| p.workspaces.iter().flatten().map(|w| w.id))
            .collect();
        self.sidebar_items.clear();
        self.schema_version = CURRENT_WORKSPACE_SCHEMA_VERSION;
    }

    /// Keep named spaces intact. A mixed-folder or empty space requires an explicit
    /// folder association; we must not guess from a terminal's changing cwd.
    fn migrate_sidebar_projects(&mut self) {
        let selected_workspace = self.selected_workspace().map(|w| w.id);
        let original = std::mem::take(&mut self.projects);
        let mut pending = HashMap::new();
        let mut owners = HashMap::new();
        let mut order = self.workspace_order.clone();
        let mut originally_empty = Vec::new();
        for mut project in original {
            let workspaces = project.workspaces.take().unwrap_or_default();
            if workspaces.is_empty() {
                originally_empty.push(project.id);
            }
            for workspace in workspaces {
                order.push(workspace.id);
                pending.insert(workspace.id, (project.id, workspace));
            }
            project.workspaces = Some(Vec::new());
            owners.insert(project.id, project);
        }
        let mut items = std::mem::take(&mut self.sidebar_items)
            .into_iter()
            .peekable();
        let mut migrated = Vec::new();
        while let Some(item) = items.next() {
            let item = match item {
                SidebarItemSnapshot::Spacer { id } => {
                    let workspace_ids =
                        if matches!(items.peek(), Some(SidebarItemSnapshot::Workspace { .. })) {
                            match items.next().unwrap() {
                                SidebarItemSnapshot::Workspace { workspace_id } => {
                                    vec![workspace_id]
                                }
                                _ => unreachable!(),
                            }
                        } else {
                            Vec::new()
                        };
                    SidebarItemSnapshot::Space {
                        id,
                        name: "Espacio".into(),
                        collapsed: false,
                        workspace_ids,
                    }
                }
                item => item,
            };
            migrated.push(item);
        }
        // Includes sessions missing from legacy ordering; duplicates are consumed once.
        migrated.extend(
            order
                .into_iter()
                .map(|workspace_id| SidebarItemSnapshot::Workspace { workspace_id }),
        );
        let mut seen_spaces = HashSet::new();
        for item in migrated {
            match item {
                SidebarItemSnapshot::Workspace { workspace_id } => {
                    let Some((owner, workspace)) = pending.remove(&workspace_id) else {
                        continue;
                    };
                    let index =
                        if let Some(index) = self.projects.iter().position(|p| p.id == owner) {
                            index
                        } else {
                            self.projects.push(owners[&owner].clone());
                            self.projects.len() - 1
                        };
                    self.projects[index]
                        .workspaces
                        .as_mut()
                        .unwrap()
                        .push(workspace);
                }
                SidebarItemSnapshot::Space {
                    id,
                    name,
                    collapsed,
                    workspace_ids,
                } => {
                    if !seen_spaces.insert(id) {
                        continue;
                    }
                    let workspaces: Vec<_> = workspace_ids
                        .into_iter()
                        .filter_map(|id| pending.remove(&id))
                        .collect();
                    let roots: HashSet<_> = workspaces
                        .iter()
                        .map(|(owner, _)| owners[owner].root_path.clone())
                        .collect();
                    let root = if roots.len() == 1 {
                        roots.into_iter().next().unwrap()
                    } else {
                        String::new()
                    };
                    let id = if owners.contains_key(&id) {
                        Uuid::new_v4()
                    } else {
                        id
                    };
                    let name = if name.trim().is_empty() {
                        "Espacio".into()
                    } else {
                        name.trim().to_owned()
                    };
                    let mut project = ProjectSnapshot::new(id, name, root);
                    project.collapsed = collapsed;
                    project.workspaces = Some(
                        workspaces
                            .into_iter()
                            .map(|(_, workspace)| workspace)
                            .collect(),
                    );
                    self.projects.push(project);
                }
                SidebarItemSnapshot::Spacer { .. } => unreachable!(),
            }
        }
        for id in originally_empty {
            if !self.projects.iter().any(|p| p.id == id) {
                self.projects.push(owners[&id].clone());
            }
        }
        if let Some(workspace_id) = selected_workspace
            && let Some(project) = self
                .projects
                .iter_mut()
                .find(|p| p.workspaces.iter().flatten().any(|w| w.id == workspace_id))
        {
            project.selected_workspace_id = Some(workspace_id);
            self.selected_project_id = Some(project.id);
        }
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
            self.tabs = None;
            self.selected_tab_id = None;
            self.sessions.clear();
            self.selected_session_id = None;
            self.visible_session_ids = None;
            self.split_axis = None;
            return;
        }

        if !workspaces
            .iter()
            .any(|workspace| Some(workspace.id) == self.selected_workspace_id)
        {
            self.selected_workspace_id = workspaces.first().map(|workspace| workspace.id);
        }

        // Schema 4 lives entirely on `workspaces`. Do not mirror Swift-era
        // sessions/tabs copies — they doubled persisted JSON and clone cost.
        self.sessions.clear();
        self.selected_session_id = None;
        self.visible_session_ids = None;
        self.split_axis = None;
        self.tabs = None;
        self.selected_tab_id = None;
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

impl TabSnapshot {
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
