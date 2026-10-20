use std::path::PathBuf;

use uuid::Uuid;

use crate::domain::workspace::PaneSplitDirection;
use crate::infrastructure::automation::{AgentAttention, AgentRuntimeState};
use crate::ports::files::FileEntry;

#[derive(Clone)]
pub(super) struct PaneIdentity {
    pub(super) title: String,
    pub(super) detail: Option<String>,
    pub(super) agent_kind: Option<String>,
    pub(super) agent_state: Option<AgentRuntimeState>,
    pub(super) agent_attention: Option<AgentAttention>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkspaceSection {
    Workspace,
    Inbox,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RightSidebarMode {
    Files,
    Diff,
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectFileRow {
    pub(super) entry: FileEntry,
    pub(super) depth: usize,
    pub(super) expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaletteMode {
    Commands,
    Files,
}

#[derive(Debug, Clone)]
pub(super) enum PaletteAction {
    AddProject,
    SelectProject(Uuid),
    NewTerminalTab,
    ToggleScratch,
    OpenIde,
    Split(PaneSplitDirection),
    EqualizePanes,
    TogglePaneZoom,
    ToggleWorkspacePanel,
    ShowFiles,
    ShowSettings,
    ShowSection(WorkspaceSection),
    OpenFile(PathBuf),
}

#[derive(Debug, Clone)]
pub(super) struct PaletteItem {
    pub(super) label: String,
    pub(super) detail: String,
    pub(super) action: PaletteAction,
}

#[derive(Debug, Clone)]
pub(super) enum ContextMenuKind {
    Pane { session_id: Uuid },
    Project { project_id: Uuid },
    SidebarBackground,
}

/// Where a context menu sits in the window.
#[derive(Debug, Clone, Copy)]
pub(super) enum ContextMenuAnchor {
    /// Top-left corner at the pointer.
    Pointer { x: f32, y: f32 },
    /// Beside the left sidebar, level with the row that was clicked: the menu's
    /// top meets the row's top when it fits below, otherwise its bottom meets
    /// the row's bottom.
    Sidebar { left: f32, top: f32, bottom: f32 },
}

#[derive(Debug, Clone)]
pub(super) struct ContextMenuState {
    pub(super) kind: ContextMenuKind,
    pub(super) anchor: ContextMenuAnchor,
}

#[derive(Debug, Clone)]
pub(super) enum RenamePromptKind {
    Pane { session_id: Uuid },
    Project { project_id: Uuid },
    NewFile { directory: PathBuf },
    NewFolder { directory: PathBuf },
}

#[derive(Debug, Clone)]
pub(super) struct RenamePrompt {
    pub(super) kind: RenamePromptKind,
    pub(super) value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContextMenuAction {
    Rename,
    AddProject,
    NewTab,
    AssociateFolder,
    RevealProject,
    RemoveProject,
    ToggleProjectPin,
    ClosePane,
    SplitRight,
    SplitDown,
    ToggleZoom,
}
