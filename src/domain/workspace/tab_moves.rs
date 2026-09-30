//! Moving existing sessions between tabs preserves their identities and live PTYs.

use uuid::Uuid;

use super::*;

impl TerminalWorkspaceSnapshot {
    pub fn tab_order(&self, review_open: bool) -> Vec<WorkspaceTabId> {
        let mut order: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| WorkspaceTabId::Terminal(tab.id))
            .collect();
        if review_open {
            order.insert(
                self.review_tab_index
                    .unwrap_or(order.len())
                    .min(order.len()),
                WorkspaceTabId::Review,
            );
        }
        order
    }

    pub(super) fn remove_tab(&mut self, index: usize) -> TabSnapshot {
        if let Some(review_index) = &mut self.review_tab_index
            && index < *review_index
        {
            *review_index -= 1;
        }
        self.tabs.remove(index)
    }
}

impl WorkspaceSnapshot {
    /// Reorder the visible strip, including the review, without changing focus.
    pub fn move_workspace_tab(
        &mut self,
        source: WorkspaceTabId,
        before: Option<WorkspaceTabId>,
        review_open: bool,
    ) -> bool {
        let Some((project_index, workspace_index)) = self.selected_workspace_indices() else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let workspace = &mut project.workspaces.as_mut().expect("normalized")[workspace_index];
        let mut order = workspace.tab_order(review_open);
        let Some(from) = order.iter().position(|id| *id == source) else {
            return false;
        };
        let to = match before {
            Some(id) => match order.iter().position(|item| *item == id) {
                Some(index) => index,
                None => return false,
            },
            None => order.len(),
        };
        let Some(insert_at) = super::relocate_index(from, to) else {
            return false;
        };
        order.remove(from);
        order.insert(insert_at, source);
        if review_open {
            workspace.review_tab_index = order.iter().position(|id| *id == WorkspaceTabId::Review);
        }
        workspace.tabs.sort_by_key(|tab| {
            order
                .iter()
                .position(|id| *id == WorkspaceTabId::Terminal(tab.id))
        });
        project.normalize();
        true
    }

    /// Move the complete source layout into a target pane, retaining nested splits.
    pub fn merge_tab_into_pane(
        &mut self,
        source_id: Uuid,
        target_session: Uuid,
        direction: PaneSplitDirection,
    ) -> bool {
        let Some((project_index, workspace_index)) = self.selected_workspace_indices() else {
            return false;
        };
        let project = &mut self.projects[project_index];
        let workspace = &mut project.workspaces.as_mut().expect("normalized")[workspace_index];
        let Some(source_index) = workspace.tabs.iter().position(|tab| tab.id == source_id) else {
            return false;
        };
        let Some(target_index) = workspace
            .tabs
            .iter()
            .position(|tab| tab.layout.contains_terminal(target_session))
        else {
            return false;
        };
        if source_index == target_index {
            return false;
        }
        let source = &workspace.tabs[source_index];
        if source.sessions.is_empty() {
            return false;
        }
        let target_id = workspace.tabs[target_index].id;
        let source = workspace.remove_tab(source_index);
        let target = workspace
            .tabs
            .iter_mut()
            .find(|tab| tab.id == target_id)
            .expect("target retained");
        target
            .layout
            .split_with_layout(target_session, &source.layout, direction);
        target.selected_session_id = source
            .selected_session_id
            .or_else(|| source.sessions.first().map(|session| session.id));
        target.zoomed_session_id = None;
        target.sessions.extend(source.sessions);
        workspace.selected_tab_id = Some(target_id);
        project.normalize();
        true
    }

    /// Detach a pane without closing its session. Single-pane tabs are reused.
    pub fn detach_terminal_to_tab(&mut self, session_id: Uuid) -> Option<Uuid> {
        let (project_index, workspace_index) = self.selected_workspace_indices()?;
        let project = &mut self.projects[project_index];
        let workspace = &mut project.workspaces.as_mut().expect("normalized")[workspace_index];
        let tab = workspace
            .tabs
            .iter_mut()
            .find(|tab| tab.layout.contains_terminal(session_id))?;
        let session_index = tab
            .sessions
            .iter()
            .position(|session| session.id == session_id)?;
        if tab.sessions.len() == 1 {
            workspace.selected_tab_id = Some(tab.id);
            return Some(tab.id);
        }
        let layout = tab.layout.removing_terminal(session_id)?;
        let session = tab.sessions.remove(session_index);
        tab.layout = layout;
        if tab.selected_session_id == Some(session_id) {
            tab.selected_session_id = tab.layout.terminal_ids().first().copied();
        }
        tab.zoomed_session_id = None;
        let detached = TabSnapshot::with_session(session);
        let tab_id = detached.id;
        workspace.tabs.push(detached);
        workspace.selected_tab_id = Some(tab_id);
        project.normalize();
        Some(tab_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn workspace() -> WorkspaceSnapshot {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/tab-moves"));
        snapshot
    }

    #[test]
    fn review_reorders_with_terminal_tabs_and_survives_serialization() {
        use WorkspaceTabId::{Review, Terminal};
        let mut snapshot = workspace();
        let first = snapshot.selected_tab().unwrap().id;
        let project = snapshot.selected_project_id.unwrap();
        let (second, _) = snapshot.open_tab_in_project(project, true).unwrap();
        assert!(snapshot.move_workspace_tab(Review, Some(Terminal(first)), true));
        assert_eq!(
            snapshot.selected_workspace().unwrap().tab_order(true),
            vec![Review, Terminal(first), Terminal(second)]
        );
        assert_eq!(
            snapshot.selected_tab().unwrap().id,
            second,
            "reordering preserves focus"
        );
        assert!(snapshot.move_workspace_tab(Terminal(second), Some(Review), true));
        assert_eq!(
            snapshot.selected_workspace().unwrap().tab_order(true),
            vec![Terminal(second), Review, Terminal(first)]
        );
        assert_eq!(
            snapshot.selected_workspace().unwrap().tab_order(false),
            vec![Terminal(second), Terminal(first)]
        );
        assert!(!snapshot.move_workspace_tab(Review, Some(Terminal(first)), true));
        let mut restored: WorkspaceSnapshot =
            serde_json::from_str(&serde_json::to_string(&snapshot).unwrap()).unwrap();
        restored.normalize();
        assert_eq!(restored, snapshot);
        assert!(snapshot.move_workspace_tab(Review, None, true));
        assert_eq!(
            snapshot.selected_workspace().unwrap().tab_order(true),
            vec![Terminal(second), Terminal(first), Review]
        );
        let mut old = serde_json::to_value(snapshot.selected_workspace().unwrap()).unwrap();
        old.as_object_mut().unwrap().remove("reviewTabIndex");
        let old: TerminalWorkspaceSnapshot = serde_json::from_value(old).unwrap();
        assert_eq!(old.tab_order(true).last(), Some(&Review));
    }

    #[test]
    fn tab_merges_preserve_sessions_and_nested_geometry_in_every_direction() {
        for direction in [
            PaneSplitDirection::Left,
            PaneSplitDirection::Right,
            PaneSplitDirection::Up,
            PaneSplitDirection::Down,
        ] {
            let mut snapshot = workspace();
            let target_tab = snapshot.selected_tab().unwrap().id;
            let target = snapshot.selected_session().unwrap().id;
            let project = snapshot.selected_project_id.unwrap();
            let (source_tab, source) = snapshot.open_tab_in_project(project, true).unwrap();
            let selected = snapshot
                .split_selected_terminal(PaneSplitDirection::Down)
                .unwrap();
            snapshot.set_selected_split_ratio(&[], 7_000);
            snapshot.toggle_selected_pane_zoom();
            let source_layout = snapshot.selected_tab().unwrap().layout.clone();
            let sessions: Vec<_> = snapshot.terminal_sessions().cloned().collect();
            assert!(snapshot.merge_tab_into_pane(source_tab, target, direction));
            assert_eq!(snapshot.selected_workspace().unwrap().tabs.len(), 1);
            let merged = snapshot.selected_tab().unwrap();
            assert_eq!(merged.id, target_tab);
            assert_eq!(merged.selected_session_id, Some(selected));
            assert_eq!(merged.zoomed_session_id, None);
            let PaneLayoutSnapshot::Split {
                axis,
                ratio,
                first,
                second,
            } = &merged.layout
            else {
                panic!("expected split");
            };
            assert_eq!(*ratio, DEFAULT_PANE_SPLIT_RATIO);
            let inserted_first =
                matches!(direction, PaneSplitDirection::Left | PaneSplitDirection::Up);
            assert_eq!(**if inserted_first { first } else { second }, source_layout);
            assert_eq!(
                *axis,
                if matches!(
                    direction,
                    PaneSplitDirection::Left | PaneSplitDirection::Right
                ) {
                    WorkspaceSplitAxis::Horizontal
                } else {
                    WorkspaceSplitAxis::Vertical
                }
            );
            assert_eq!(
                merged.layout.terminal_ids(),
                if inserted_first {
                    vec![source, selected, target]
                } else {
                    vec![target, source, selected]
                }
            );
            assert_eq!(
                snapshot.terminal_sessions().cloned().collect::<Vec<_>>(),
                sessions
            );
            let before = snapshot.clone();
            snapshot.normalize();
            assert_eq!(snapshot, before);
        }
    }

    #[test]
    fn detaching_a_zoomed_pane_collapses_only_its_branch_and_reuses_the_session() {
        let mut snapshot = workspace();
        let tab_id = snapshot.selected_tab().unwrap().id;
        let left = snapshot.selected_session().unwrap().id;
        let right = snapshot
            .split_selected_terminal(PaneSplitDirection::Right)
            .unwrap();
        snapshot.set_selected_split_ratio(&[], 6_000);
        let bottom = snapshot
            .split_selected_terminal(PaneSplitDirection::Down)
            .unwrap();
        let session = snapshot.selected_session().unwrap().clone();
        snapshot.toggle_selected_pane_zoom();
        let detached = snapshot.detach_terminal_to_tab(bottom).unwrap();
        assert_ne!(detached, tab_id);
        assert_eq!(snapshot.selected_session(), Some(&session));
        assert_eq!(
            snapshot.selected_tab().unwrap().layout,
            PaneLayoutSnapshot::terminal(bottom)
        );
        let original = &snapshot.selected_workspace().unwrap().tabs[0];
        assert_eq!(original.layout.terminal_ids(), vec![left, right]);
        assert_eq!(original.zoomed_session_id, None);
        assert_eq!(original.selected_session_id, Some(left));
        assert!(matches!(
            original.layout,
            PaneLayoutSnapshot::Split { ratio: 6_000, .. }
        ));
        assert_eq!(snapshot.detach_terminal_to_tab(bottom), Some(detached));
        assert_eq!(snapshot.selected_workspace().unwrap().tabs.len(), 2);
        assert_eq!(snapshot.terminal_sessions().count(), 3);
    }

    #[test]
    fn removing_tabs_before_review_keeps_its_place_and_invalid_moves_are_atomic() {
        use WorkspaceTabId::{Review, Terminal};
        let mut snapshot = workspace();
        let first = snapshot.selected_tab().unwrap().id;
        let first_session = snapshot.selected_session().unwrap().id;
        let project = snapshot.selected_project_id.unwrap();
        let (second, second_session) = snapshot.open_tab_in_project(project, true).unwrap();
        let (third, _) = snapshot.open_tab_in_project(project, true).unwrap();
        snapshot.move_workspace_tab(Review, Some(Terminal(third)), true);
        assert!(snapshot.merge_tab_into_pane(first, second_session, PaneSplitDirection::Right));
        assert_eq!(
            snapshot.selected_workspace().unwrap().tab_order(true),
            vec![Terminal(second), Review, Terminal(third)]
        );
        snapshot.close_terminal(first_session);
        snapshot.close_terminal(second_session);
        assert_eq!(
            snapshot.selected_workspace().unwrap().tab_order(true),
            vec![Review, Terminal(third)]
        );
        let before = snapshot.clone();
        let missing = Uuid::new_v4();
        let session = snapshot.selected_session().unwrap().id;
        assert!(!snapshot.merge_tab_into_pane(third, session, PaneSplitDirection::Down));
        assert!(!snapshot.merge_tab_into_pane(missing, session, PaneSplitDirection::Down));
        assert!(!snapshot.merge_tab_into_pane(third, missing, PaneSplitDirection::Down));
        assert_eq!(snapshot.detach_terminal_to_tab(missing), None);
        assert!(!snapshot.move_workspace_tab(Review, Some(Terminal(missing)), true));
        assert_eq!(snapshot, before);
    }
}
