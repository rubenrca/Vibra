use std::path::PathBuf;

use uuid::Uuid;

use crate::domain::agent_chat::ChatAgent;
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
    OpenIde,
    Split(PaneSplitDirection),
    EqualizePanes,
    TogglePaneZoom,
    ToggleWorkspacePanel,
    ShowFiles,
    ShowSettings,
    ShowSection(WorkspaceSection),
    /// A new pane split right of the selected pane, showing a chat.
    NewChatPane(ChatAgent),
    /// A new tab in the selected project, showing a chat.
    NewChatTab(ChatAgent),
    /// Switches the selected pane to a chat with this agent.
    ShowPaneAsChat(ChatAgent),
    OpenFile(PathBuf),
}

/// Where a chat chosen from the agent picker goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChatTarget {
    /// The pane's own view, from its Terminal | Chat control.
    Pane(Uuid),
    /// A new pane split from this one.
    Split {
        session_id: Uuid,
        direction: PaneSplitDirection,
    },
    /// A new tab in this project.
    NewTab(Uuid),
}

#[derive(Debug, Clone)]
pub(super) struct PaletteItem {
    pub(super) label: String,
    pub(super) detail: String,
    pub(super) action: PaletteAction,
}

#[derive(Debug, Clone)]
pub(super) enum ContextMenuKind {
    Pane {
        session_id: Uuid,
    },
    Project {
        project_id: Uuid,
    },
    SidebarBackground,
    /// Chat agents to choose from, for a pane or a new tab.
    ChatAgentPicker {
        target: ChatTarget,
    },
}

#[derive(Debug, Clone)]
pub(super) struct ContextMenuState {
    pub(super) kind: ContextMenuKind,
    pub(super) x: f32,
    pub(super) y: f32,
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
    /// Switches the menu's pane to a chat with this agent.
    ShowAsChat(ChatAgent),
    /// Opens the agent picker for `ChatTarget` next to the menu.
    OpenChatPicker(ChatTarget),
    /// Starts a chat with this agent at the picker's target.
    StartChat(ChatAgent),
    /// Closes the menu without acting, for a picker with no agents.
    Dismiss,
}
