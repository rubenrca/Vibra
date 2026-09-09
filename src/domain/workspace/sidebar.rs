use uuid::Uuid;

use super::types::SidebarItemSnapshot;

pub(super) fn sidebar_space_for(items: &[SidebarItemSnapshot], workspace_id: Uuid) -> Option<Uuid> {
    items.iter().find_map(|item| match item {
        SidebarItemSnapshot::Space {
            id, workspace_ids, ..
        } if workspace_ids.contains(&workspace_id) => Some(*id),
        _ => None,
    })
}

pub(super) fn sidebar_contains_workspace(
    items: &[SidebarItemSnapshot],
    workspace_id: Uuid,
) -> bool {
    items.iter().any(|item| match item {
        SidebarItemSnapshot::Workspace { workspace_id: id } => *id == workspace_id,
        SidebarItemSnapshot::Space { workspace_ids, .. } => workspace_ids.contains(&workspace_id),
        SidebarItemSnapshot::Spacer { .. } => false,
    })
}

pub(super) fn detach_workspace_from_sidebar(
    items: &mut Vec<SidebarItemSnapshot>,
    workspace_id: Uuid,
) {
    if let Some(index) = items.iter().position(
        |item| matches!(item, SidebarItemSnapshot::Workspace { workspace_id: id } if *id == workspace_id),
    ) {
        items.remove(index);
        return;
    }
    for item in items {
        if let SidebarItemSnapshot::Space { workspace_ids, .. } = item {
            workspace_ids.retain(|id| *id != workspace_id);
        }
    }
}

pub(super) fn sidebar_workspace_ids(items: &[SidebarItemSnapshot]) -> Vec<Uuid> {
    items
        .iter()
        .flat_map(|item| match item {
            SidebarItemSnapshot::Workspace { workspace_id } => vec![*workspace_id],
            SidebarItemSnapshot::Space { workspace_ids, .. } => workspace_ids.clone(),
            SidebarItemSnapshot::Spacer { .. } => Vec::new(),
        })
        .collect()
}
