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
