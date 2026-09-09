use std::collections::HashSet;

use uuid::Uuid;

use super::types::*;

impl super::WorkspaceSnapshot {
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

        let workspace_ids: Vec<_> = self
            .projects
            .iter()
            .flat_map(|project| project.workspaces.as_deref().unwrap_or_default())
            .map(|workspace| workspace.id)
            .collect();
        let valid_ids: HashSet<_> = workspace_ids.iter().copied().collect();
        let mut seen_ids = HashSet::new();
        self.workspace_order
            .retain(|id| valid_ids.contains(id) && seen_ids.insert(*id));
        let ordered_ids: HashSet<_> = self.workspace_order.iter().copied().collect();
        self.workspace_order.extend(
            workspace_ids
                .iter()
                .copied()
                .filter(|id| !ordered_ids.contains(id)),
        );

        if self.sidebar_items.is_empty() {
            self.sidebar_items = self
                .workspace_order
                .iter()
                .copied()
                .map(|workspace_id| SidebarItemSnapshot::Workspace { workspace_id })
                .collect();
        }

        // Schema 5 stored bare separators. Treat the first workspace following
        // one as the initial member of a named space so preview data still loads.
        if self
            .sidebar_items
            .iter()
            .any(|item| matches!(item, SidebarItemSnapshot::Spacer { .. }))
        {
            let previous = std::mem::take(&mut self.sidebar_items);
            let mut items = Vec::with_capacity(previous.len());
            let mut iter = previous.into_iter().peekable();
            while let Some(item) = iter.next() {
                match item {
                    SidebarItemSnapshot::Spacer { id } => {
                        let workspace_ids = match iter.peek() {
                            Some(SidebarItemSnapshot::Workspace { .. }) => match iter.next() {
                                Some(SidebarItemSnapshot::Workspace { workspace_id }) => {
                                    vec![workspace_id]
                                }
                                _ => unreachable!(),
                            },
                            _ => Vec::new(),
                        };
                        items.push(SidebarItemSnapshot::Space {
                            id,
                            name: "Espacio".into(),
                            collapsed: false,
                            workspace_ids,
                        });
                    }
                    item => items.push(item),
                }
            }
            self.sidebar_items = items;
        }
        let mut seen_workspaces = HashSet::new();
        let mut seen_spaces = HashSet::new();
        let previous = std::mem::take(&mut self.sidebar_items);
        self.sidebar_items = previous
            .into_iter()
            .filter_map(|item| match item {
                SidebarItemSnapshot::Workspace { workspace_id } => {
                    (valid_ids.contains(&workspace_id) && seen_workspaces.insert(workspace_id))
                        .then_some(SidebarItemSnapshot::Workspace { workspace_id })
                }
                SidebarItemSnapshot::Space {
                    id,
                    mut name,
                    collapsed,
                    mut workspace_ids,
                } => {
                    name = name.trim().to_owned();
                    if name.is_empty() {
                        name = "Espacio".into();
                    }
                    workspace_ids.retain(|workspace_id| {
                        valid_ids.contains(workspace_id) && seen_workspaces.insert(*workspace_id)
                    });
                    seen_spaces
                        .insert(id)
                        .then_some(SidebarItemSnapshot::Space {
                            id,
                            name,
                            collapsed,
                            workspace_ids,
                        })
                }
                SidebarItemSnapshot::Spacer { .. } => None,
            })
            .collect();
        self.sidebar_items.extend(
            workspace_ids
                .iter()
                .filter(|id| !seen_workspaces.contains(id))
                .map(|workspace_id| SidebarItemSnapshot::Workspace {
                    workspace_id: *workspace_id,
                }),
        );
        self.workspace_order = self
            .sidebar_items
            .iter()
            .flat_map(|item| match item {
                SidebarItemSnapshot::Workspace { workspace_id } => vec![*workspace_id],
                SidebarItemSnapshot::Space { workspace_ids, .. } => workspace_ids.clone(),
                SidebarItemSnapshot::Spacer { .. } => Vec::new(),
            })
            .collect();

        // Normalization performs the legacy-to-canonical conversions above, so a
        // successfully normalized snapshot is safe to persist as the current schema.
        self.schema_version = CURRENT_WORKSPACE_SCHEMA_VERSION;
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
