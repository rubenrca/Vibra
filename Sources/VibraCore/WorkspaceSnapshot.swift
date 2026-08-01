import Foundation

public struct WorkspaceSnapshot: Codable, Equatable, Sendable {
    public var projects: [ProjectSnapshot]
    public var selectedProjectID: UUID?

    public init(projects: [ProjectSnapshot], selectedProjectID: UUID?) {
        self.projects = projects
        self.selectedProjectID = selectedProjectID
        normalizeSelection()
    }

    public mutating func normalizeSelection() {
        for index in projects.indices {
            projects[index].normalizeSelection()
        }

        guard !projects.isEmpty else {
            selectedProjectID = nil
            return
        }

        if !projects.contains(where: { $0.id == selectedProjectID }) {
            selectedProjectID = projects[0].id
        }
    }
}

public struct ProjectSnapshot: Codable, Equatable, Identifiable, Sendable {
    public let id: UUID
    public var name: String
    public var rootPath: String

    // Kept in the file format so 0.1 workspaces migrate without data loss.
    public var sessions: [SessionSnapshot]
    public var selectedSessionID: UUID?
    public var visibleSessionIDs: [UUID]?
    public var splitAxis: WorkspaceSplitAxis?

    public var tabs: [TabSnapshot]?
    public var selectedTabID: UUID?
    public var workspaces: [TerminalWorkspaceSnapshot]?
    public var selectedWorkspaceID: UUID?

    public init(
        id: UUID = UUID(),
        name: String,
        rootPath: String,
        sessions: [SessionSnapshot],
        selectedSessionID: UUID?,
        visibleSessionIDs: [UUID]? = nil,
        splitAxis: WorkspaceSplitAxis? = nil,
        tabs: [TabSnapshot]? = nil,
        selectedTabID: UUID? = nil,
        workspaces: [TerminalWorkspaceSnapshot]? = nil,
        selectedWorkspaceID: UUID? = nil
    ) {
        self.id = id
        self.name = name
        self.rootPath = rootPath
        self.sessions = sessions
        self.selectedSessionID = selectedSessionID
        self.visibleSessionIDs = visibleSessionIDs
        self.splitAxis = splitAxis
        self.tabs = tabs
        self.selectedTabID = selectedTabID
        self.workspaces = workspaces
        self.selectedWorkspaceID = selectedWorkspaceID
        normalizeSelection()
    }

    public mutating func normalizeSelection() {
        if workspaces?.isEmpty != false {
            var migratedTabs = tabs
            if migratedTabs?.isEmpty != false {
                migratedTabs = migrateLegacyTabs()
            }
            if var migratedTabs {
                for index in migratedTabs.indices {
                    migratedTabs[index].normalizeSelection()
                }
                migratedTabs.removeAll(where: { $0.sessions.isEmpty })
                if !migratedTabs.isEmpty {
                    let selectedTabID = migratedTabs.contains(where: {
                        $0.id == self.selectedTabID
                    }) ? self.selectedTabID : migratedTabs.first(where: {
                        $0.sessions.contains(where: { $0.id == selectedSessionID })
                    })?.id ?? migratedTabs[0].id
                    workspaces = [
                        TerminalWorkspaceSnapshot(
                            name: name,
                            tabs: migratedTabs,
                            selectedTabID: selectedTabID
                        ),
                    ]
                }
            }
        }

        if var normalizedWorkspaces = workspaces {
            for index in normalizedWorkspaces.indices {
                normalizedWorkspaces[index].normalizeSelection()
            }
            normalizedWorkspaces.removeAll(where: { $0.tabs.isEmpty })
            workspaces = normalizedWorkspaces
        }

        guard let workspaces, !workspaces.isEmpty else {
            self.workspaces = []
            selectedWorkspaceID = nil
            self.tabs = []
            selectedTabID = nil
            sessions = []
            selectedSessionID = nil
            visibleSessionIDs = []
            splitAxis = nil
            return
        }

        if !workspaces.contains(where: { $0.id == selectedWorkspaceID }) {
            selectedWorkspaceID = workspaces.first(where: { workspace in
                workspace.tabs.contains(where: { tab in
                    tab.sessions.contains(where: { $0.id == selectedSessionID })
                })
            })?.id ?? workspaces[0].id
        }

        let selectedWorkspace = workspaces.first { $0.id == selectedWorkspaceID }
            ?? workspaces[0]
        let selectedTab = selectedWorkspace.tabs.first {
            $0.id == selectedWorkspace.selectedTabID
        } ?? selectedWorkspace.tabs[0]
        sessions = workspaces.flatMap { $0.tabs }.flatMap(\.sessions)
        selectedSessionID = selectedTab.selectedSessionID
        visibleSessionIDs = selectedTab.layout.terminalIDs
        splitAxis = selectedTab.layout.rootAxis
        tabs = selectedWorkspace.tabs
        selectedTabID = selectedWorkspace.selectedTabID
    }

    private func migrateLegacyTabs() -> [TabSnapshot] {
        guard !sessions.isEmpty else { return [] }
        let validIDs = Set(sessions.map(\.id))
        let visibleIDs = (visibleSessionIDs ?? [selectedSessionID].compactMap { $0 })
            .filter { validIDs.contains($0) }
        let visibleSet = Set(visibleIDs)
        let groupedSessions = sessions.filter { visibleSet.contains($0.id) }
        var insertedGroup = false
        var migrated: [TabSnapshot] = []

        for session in sessions {
            if visibleSet.contains(session.id) {
                guard !insertedGroup else { continue }
                insertedGroup = true
                migrated.append(
                    TabSnapshot(
                        sessions: groupedSessions,
                        selectedSessionID: groupedSessions.contains(where: {
                            $0.id == selectedSessionID
                        }) ? selectedSessionID : groupedSessions.first?.id,
                        layout: PaneLayoutSnapshot.joining(
                            groupedSessions.map { .terminal($0.id) },
                            axis: splitAxis ?? .horizontal
                        )
                    )
                )
            } else {
                migrated.append(
                    TabSnapshot(
                        sessions: [session],
                        selectedSessionID: session.id,
                        layout: .terminal(session.id)
                    )
                )
            }
        }
        return migrated
    }
}

public struct TerminalWorkspaceSnapshot: Codable, Equatable, Identifiable, Sendable {
    public let id: UUID
    public var name: String
    /// Whether the name is maintained from the active coding task or was set
    /// explicitly by the user. Optional for seamless decoding of 0.2.4 and
    /// earlier workspace files.
    public var titleSource: WorkspaceTitleSource?
    public var tabs: [TabSnapshot]
    public var selectedTabID: UUID?

    public init(
        id: UUID = UUID(),
        name: String,
        titleSource: WorkspaceTitleSource? = nil,
        tabs: [TabSnapshot],
        selectedTabID: UUID?
    ) {
        self.id = id
        self.name = name
        self.titleSource = titleSource
        self.tabs = tabs
        self.selectedTabID = selectedTabID
        normalizeSelection()
    }

    public mutating func normalizeSelection() {
        for index in tabs.indices {
            tabs[index].normalizeSelection()
        }
        tabs.removeAll(where: { $0.sessions.isEmpty })
        guard !tabs.isEmpty else {
            selectedTabID = nil
            return
        }
        if !tabs.contains(where: { $0.id == selectedTabID }) {
            selectedTabID = tabs[0].id
        }
    }
}

public enum WorkspaceTitleSource: String, Codable, Equatable, Sendable {
    case automatic
    case manual
}

public struct TabSnapshot: Codable, Equatable, Identifiable, Sendable {
    public let id: UUID
    public var sessions: [SessionSnapshot]
    public var selectedSessionID: UUID?
    public var layout: PaneLayoutSnapshot

    public init(
        id: UUID = UUID(),
        sessions: [SessionSnapshot],
        selectedSessionID: UUID?,
        layout: PaneLayoutSnapshot
    ) {
        self.id = id
        self.sessions = sessions
        self.selectedSessionID = selectedSessionID
        self.layout = layout
        normalizeSelection()
    }

    public mutating func normalizeSelection() {
        guard !sessions.isEmpty else {
            selectedSessionID = nil
            return
        }
        let sessionIDs = sessions.map(\.id)
        if Set(layout.terminalIDs) != Set(sessionIDs)
            || layout.terminalIDs.count != sessionIDs.count {
            layout = PaneLayoutSnapshot.joining(
                sessionIDs.map(PaneLayoutSnapshot.terminal),
                axis: .horizontal
            )
        }
        if !sessionIDs.contains(where: { $0 == selectedSessionID }) {
            selectedSessionID = sessionIDs[0]
        }
    }
}

public indirect enum PaneLayoutSnapshot: Codable, Equatable, Sendable {
    case terminal(UUID)
    case split(
        axis: WorkspaceSplitAxis,
        first: PaneLayoutSnapshot,
        second: PaneLayoutSnapshot
    )

    public var terminalIDs: [UUID] {
        switch self {
        case .terminal(let id): [id]
        case .split(_, let first, let second): first.terminalIDs + second.terminalIDs
        }
    }

    public var rootAxis: WorkspaceSplitAxis? {
        guard case .split(let axis, _, _) = self else { return nil }
        return axis
    }

    public func replacingTerminal(
        _ id: UUID,
        with replacement: PaneLayoutSnapshot
    ) -> PaneLayoutSnapshot {
        switch self {
        case .terminal(let current):
            current == id ? replacement : self
        case .split(let axis, let first, let second):
            .split(
                axis: axis,
                first: first.replacingTerminal(id, with: replacement),
                second: second.replacingTerminal(id, with: replacement)
            )
        }
    }

    public func removingTerminal(_ id: UUID) -> PaneLayoutSnapshot? {
        switch self {
        case .terminal(let current):
            current == id ? nil : self
        case .split(let axis, let first, let second):
            switch (first.removingTerminal(id), second.removingTerminal(id)) {
            case (nil, nil): nil
            case (nil, let remaining?): remaining
            case (let remaining?, nil): remaining
            case (let first?, let second?): .split(
                axis: axis,
                first: first,
                second: second
            )
            }
        }
    }

    public static func joining(
        _ layouts: [PaneLayoutSnapshot],
        axis: WorkspaceSplitAxis
    ) -> PaneLayoutSnapshot {
        guard let first = layouts.first else {
            preconditionFailure("A pane layout requires at least one terminal")
        }
        return layouts.dropFirst().reduce(first) { partial, next in
            .split(axis: axis, first: partial, second: next)
        }
    }
}

public enum WorkspaceSplitAxis: String, Codable, Equatable, Sendable {
    case horizontal
    case vertical
}

public struct SessionSnapshot: Codable, Equatable, Identifiable, Sendable {
    public let id: UUID
    public var title: String
    public var workingDirectory: String

    public init(
        id: UUID = UUID(),
        title: String = "Terminal",
        workingDirectory: String
    ) {
        self.id = id
        self.title = title
        self.workingDirectory = workingDirectory
    }
}
