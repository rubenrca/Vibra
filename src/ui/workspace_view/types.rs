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
    Activity,
    Settings,
}

/// A tool that opens as a tab of its space, after the space's own tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ToolTab {
    Repository,
    Activity,
}

impl ToolTab {
    /// Their order in the strip.
    pub(super) const ALL: [Self; 2] = [Self::Repository, Self::Activity];

    pub(super) fn of(section: WorkspaceSection) -> Option<Self> {
        match section {
            WorkspaceSection::Inbox => Some(Self::Repository),
            WorkspaceSection::Activity => Some(Self::Activity),
            WorkspaceSection::Workspace | WorkspaceSection::Settings => None,
        }
    }

    pub(super) fn section(self) -> WorkspaceSection {
        match self {
            Self::Repository => WorkspaceSection::Inbox,
            Self::Activity => WorkspaceSection::Activity,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Repository => "Repository",
            Self::Activity => "Activity",
        }
    }

    pub(super) fn icon(self) -> &'static str {
        match self {
            Self::Repository => "chrome-icons/repository.svg",
            Self::Activity => "chrome-icons/activity.svg",
        }
    }
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
    NewSpace,
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
    OpenFile(PathBuf),
}

#[derive(Debug, Clone)]
pub(super) struct PaletteItem {
    pub(super) label: String,
    pub(super) detail: String,
    /// Keyboard shortcut drawn as keycaps, empty when the command has none.
    pub(super) shortcut: &'static str,
    pub(super) action: PaletteAction,
}

#[derive(Debug, Clone)]
pub(super) enum ContextMenuKind {
    Pane {
        session_id: Uuid,
    },
    /// The review's tab in the strip.
    Review,
    /// A tool's tab in the strip.
    Tool(ToolTab),
    Project {
        project_id: Uuid,
    },
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
    NewSpace,
    AddProject,
    NewTab,
    RevealProject,
    RemoveProject,
    ToggleProjectPin,
    ClosePane,
    CloseTab,
    CloseReview,
    CloseTool(ToolTab),
    SplitRight,
    SplitDown,
    ToggleZoom,
}
