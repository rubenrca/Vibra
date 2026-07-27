import Foundation
import Testing
@testable import VibraCore

@Test func workspaceNormalizesMissingSelections() {
    let session = SessionSnapshot(workingDirectory: "/tmp")
    let project = ProjectSnapshot(
        name: "Example",
        rootPath: "/tmp",
        sessions: [session],
        selectedSessionID: UUID()
    )

    let workspace = WorkspaceSnapshot(
        projects: [project],
        selectedProjectID: UUID()
    )

    #expect(workspace.selectedProjectID == project.id)
    #expect(workspace.projects[0].selectedSessionID == session.id)
}

@Test func emptyWorkspaceHasNoSelection() {
    let workspace = WorkspaceSnapshot(projects: [], selectedProjectID: UUID())

    #expect(workspace.selectedProjectID == nil)
}

@Test func snapshotRoundTripsThroughJSON() throws {
    let session = SessionSnapshot(title: "Agent", workingDirectory: "/tmp/repo")
    let project = ProjectSnapshot(
        name: "Repo",
        rootPath: "/tmp/repo",
        sessions: [session],
        selectedSessionID: session.id
    )
    let original = WorkspaceSnapshot(
        projects: [project],
        selectedProjectID: project.id
    )

    let data = try JSONEncoder().encode(original)
    let restored = try JSONDecoder().decode(WorkspaceSnapshot.self, from: data)

    #expect(restored == original)
}

@Test func legacySnapshotRestoresVisibleSession() throws {
    let projectID = UUID()
    let sessionID = UUID()
    let json = """
    {
      "projects": [{
        "id": "\(projectID.uuidString)",
        "name": "Repo",
        "rootPath": "/tmp/repo",
        "sessions": [{
          "id": "\(sessionID.uuidString)",
          "title": "Terminal",
          "workingDirectory": "/tmp/repo"
        }],
        "selectedSessionID": "\(sessionID.uuidString)"
      }],
      "selectedProjectID": "\(projectID.uuidString)"
    }
    """

    var restored = try JSONDecoder().decode(
        WorkspaceSnapshot.self,
        from: Data(json.utf8)
    )
    restored.normalizeSelection()

    #expect(restored.projects[0].visibleSessionIDs == [sessionID])
    #expect(restored.projects[0].splitAxis == nil)
    #expect(restored.projects[0].tabs?.count == 1)
    #expect(restored.projects[0].tabs?[0].layout == .terminal(sessionID))
    #expect(restored.projects[0].workspaces?.count == 1)
    #expect(restored.projects[0].workspaces?[0].tabs.count == 1)
}

@Test func legacySessionsMigrateToTabsAndPreserveSplitGroup() {
    let first = SessionSnapshot(workingDirectory: "/tmp/repo")
    let second = SessionSnapshot(workingDirectory: "/tmp/repo")
    let third = SessionSnapshot(workingDirectory: "/tmp/repo")
    let project = ProjectSnapshot(
        name: "Repo",
        rootPath: "/tmp/repo",
        sessions: [first, second, third],
        selectedSessionID: second.id,
        visibleSessionIDs: [first.id, second.id],
        splitAxis: .horizontal
    )

    #expect(project.tabs?.count == 2)
    #expect(project.tabs?.first?.layout.terminalIDs == [first.id, second.id])
    #expect(project.tabs?.last?.layout == .terminal(third.id))
    #expect(project.selectedTabID == project.tabs?.first?.id)
    #expect(project.workspaces?.count == 1)
    #expect(project.workspaces?.first?.tabs.count == 2)
}

@Test func workspacesKeepIndependentTabSelections() throws {
    let firstSession = SessionSnapshot(workingDirectory: "/tmp/repo")
    let secondSession = SessionSnapshot(workingDirectory: "/tmp/repo")
    let firstTab = TabSnapshot(
        sessions: [firstSession],
        selectedSessionID: firstSession.id,
        layout: .terminal(firstSession.id)
    )
    let secondTab = TabSnapshot(
        sessions: [secondSession],
        selectedSessionID: secondSession.id,
        layout: .terminal(secondSession.id)
    )
    let firstWorkspace = TerminalWorkspaceSnapshot(
        name: "Review",
        tabs: [firstTab],
        selectedTabID: firstTab.id
    )
    let secondWorkspace = TerminalWorkspaceSnapshot(
        name: "Implementation",
        tabs: [secondTab],
        selectedTabID: secondTab.id
    )
    let project = ProjectSnapshot(
        name: "Repo",
        rootPath: "/tmp/repo",
        sessions: [],
        selectedSessionID: secondSession.id,
        workspaces: [firstWorkspace, secondWorkspace],
        selectedWorkspaceID: secondWorkspace.id
    )

    #expect(project.workspaces?.count == 2)
    #expect(project.selectedWorkspaceID == secondWorkspace.id)
    #expect(project.tabs?.map(\.id) == [secondTab.id])
    #expect(project.selectedSessionID == secondSession.id)
    #expect(Set(project.sessions.map(\.id)) == Set([firstSession.id, secondSession.id]))

    let data = try JSONEncoder().encode(project)
    var restored = try JSONDecoder().decode(ProjectSnapshot.self, from: data)
    restored.normalizeSelection()
    #expect(restored == project)
}

@Test func verticalTabsRetainTheirHorizontalTabsAcrossRestoration() throws {
    let looseSession = SessionSnapshot(workingDirectory: "/tmp")
    let groupedSession = SessionSnapshot(workingDirectory: "/tmp/repo")
    let looseTab = TabSnapshot(
        sessions: [looseSession],
        selectedSessionID: looseSession.id,
        layout: .terminal(looseSession.id)
    )
    let groupedTab = TabSnapshot(
        sessions: [groupedSession],
        selectedSessionID: groupedSession.id,
        layout: .terminal(groupedSession.id)
    )
    let firstVerticalTab = TerminalWorkspaceSnapshot(
        name: "Vibra",
        tabs: [looseTab],
        selectedTabID: looseTab.id
    )
    let secondVerticalTab = TerminalWorkspaceSnapshot(
        name: "Backend",
        tabs: [groupedTab],
        selectedTabID: groupedTab.id
    )
    let project = ProjectSnapshot(
        name: "",
        rootPath: "/tmp",
        sessions: [],
        selectedSessionID: groupedSession.id,
        workspaces: [firstVerticalTab, secondVerticalTab],
        selectedWorkspaceID: secondVerticalTab.id
    )

    let data = try JSONEncoder().encode(project)
    var restored = try JSONDecoder().decode(ProjectSnapshot.self, from: data)
    restored.normalizeSelection()

    #expect(restored.workspaces?.map(\.name) == ["Vibra", "Backend"])
    #expect(restored.workspaces?.first?.tabs.map(\.id) == [looseTab.id])
    #expect(restored.workspaces?.last?.tabs.map(\.id) == [groupedTab.id])
}

@Test func paneTreeSplitsAndCollapsesWithoutCreatingTabs() throws {
    let first = UUID()
    let second = UUID()
    let third = UUID()
    let initial = PaneLayoutSnapshot.terminal(first)
    let horizontal = initial.replacingTerminal(
        first,
        with: .split(
            axis: .horizontal,
            first: .terminal(first),
            second: .terminal(second)
        )
    )
    let nested = horizontal.replacingTerminal(
        second,
        with: .split(
            axis: .vertical,
            first: .terminal(second),
            second: .terminal(third)
        )
    )

    #expect(nested.terminalIDs == [first, second, third])
    let collapsed = try #require(nested.removingTerminal(second))
    #expect(collapsed.terminalIDs == [first, third])
    #expect(collapsed.rootAxis == .horizontal)
}
