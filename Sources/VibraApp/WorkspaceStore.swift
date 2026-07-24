import AppKit
import Combine
import Foundation
import VibraCore

struct TerminalTab: Identifiable {
    let id: UUID
    var sessions: [TerminalSession]
    var selectedSessionID: UUID?
    var layout: PaneLayoutSnapshot

    var selectedSession: TerminalSession? {
        sessions.first { $0.id == selectedSessionID }
    }

}

struct VibraProject: Identifiable {
    let id: UUID
    var name: String
    var rootPath: String
    var tabs: [TerminalTab]
    var selectedTabID: UUID?

    var selectedTab: TerminalTab? {
        tabs.first { $0.id == selectedTabID }
    }

    var selectedSession: TerminalSession? {
        selectedTab?.selectedSession
    }

    var allSessions: [TerminalSession] {
        tabs.flatMap(\.sessions)
    }
}

enum RightSidebarMode: String {
    case files
    case changes
}

@MainActor
final class WorkspaceStore: ObservableObject {
    @Published private(set) var projects: [VibraProject] = []
    @Published private(set) var selectedProjectID: UUID?
    @Published private(set) var isProjectSidebarVisible: Bool = {
        guard UserDefaults.standard.object(forKey: SettingsKeys.projectSidebarVisible) != nil else {
            return true
        }
        return UserDefaults.standard.bool(forKey: SettingsKeys.projectSidebarVisible)
    }()
    @Published private(set) var isGitSidebarVisible: Bool = {
        guard UserDefaults.standard.object(forKey: SettingsKeys.gitSidebarVisible) != nil else {
            return true
        }
        return UserDefaults.standard.bool(forKey: SettingsKeys.gitSidebarVisible)
    }()
    @Published private(set) var rightSidebarMode: RightSidebarMode = {
        guard let value = UserDefaults.standard.string(forKey: "rightSidebarMode") else {
            return .changes
        }
        return RightSidebarMode(rawValue: value) ?? .changes
    }()

    private var hiddenSessionPump: Timer?

    init() {
        restoreWorkspace()
        if let launchDirectory = Self.launchDirectory() {
            addProject(at: launchDirectory, persist: false)
        } else if projects.isEmpty {
            addProject(
                at: URL(fileURLWithPath: FileManager.default.homeDirectoryForCurrentUser.path),
                persist: false
            )
        }
        refreshSessionVisibility()
        saveWorkspace()
    }

    var selectedProject: VibraProject? {
        projects.first { $0.id == selectedProjectID }
    }

    var selectedTab: TerminalTab? {
        selectedProject?.selectedTab
    }

    var selectedSession: TerminalSession? {
        selectedTab?.selectedSession
    }

    var allSessions: [TerminalSession] {
        projects.flatMap(\.allSessions)
    }

    func selectProject(_ id: UUID) {
        guard projects.contains(where: { $0.id == id }) else { return }
        selectedProjectID = id
        refreshSessionVisibility()
        saveWorkspace()
    }

    func selectTab(_ id: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              projects[projectIndex].tabs.contains(where: { $0.id == id })
        else { return }
        projects[projectIndex].selectedTabID = id
        selectedProjectID = projectID
        refreshSessionVisibility()
        saveWorkspace()
    }

    func focusSession(_ id: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let tabIndex = projects[projectIndex].tabs.firstIndex(where: {
                  $0.sessions.contains(where: { $0.id == id })
              })
        else { return }
        projects[projectIndex].selectedTabID = projects[projectIndex].tabs[tabIndex].id
        projects[projectIndex].tabs[tabIndex].selectedSessionID = id
        selectedProjectID = projectID
        refreshSessionVisibility()
        saveWorkspace()
    }

    func toggleProjectSidebar() {
        isProjectSidebarVisible.toggle()
        UserDefaults.standard.set(
            isProjectSidebarVisible,
            forKey: SettingsKeys.projectSidebarVisible
        )
    }

    func toggleGitSidebar() {
        isGitSidebarVisible.toggle()
        UserDefaults.standard.set(isGitSidebarVisible, forKey: SettingsKeys.gitSidebarVisible)
    }

    func showGitSidebar(mode: RightSidebarMode = .changes) {
        rightSidebarMode = mode
        UserDefaults.standard.set(mode.rawValue, forKey: "rightSidebarMode")
        if !isGitSidebarVisible {
            isGitSidebarVisible = true
            UserDefaults.standard.set(true, forKey: SettingsKeys.gitSidebarVisible)
        }
    }

    func selectRightSidebarMode(_ mode: RightSidebarMode) {
        rightSidebarMode = mode
        UserDefaults.standard.set(mode.rawValue, forKey: "rightSidebarMode")
    }

    func addProject(at url: URL, persist: Bool = true) {
        let normalizedURL = url.standardizedFileURL
        let path = normalizedURL.path

        if let existing = projects.first(where: { $0.rootPath == path }) {
            selectProject(existing.id)
            return
        }

        let projectID = UUID()
        let tab = makeTab(workingDirectory: path, projectID: projectID)
        let name = normalizedURL.lastPathComponent.isEmpty
            ? normalizedURL.path
            : normalizedURL.lastPathComponent
        projects.append(
            VibraProject(
                id: projectID,
                name: name,
                rootPath: path,
                tabs: [tab],
                selectedTabID: tab.id
            )
        )
        selectedProjectID = projectID
        refreshSessionVisibility()
        if persist { saveWorkspace() }
    }

    func chooseProject() {
        let panel = NSOpenPanel()
        panel.title = "Open a project"
        panel.prompt = "Open Project"
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = selectedProject.map { URL(fileURLWithPath: $0.rootPath) }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        addProject(at: url)
    }

    func closeProject(_ id: UUID) {
        guard let index = projects.firstIndex(where: { $0.id == id }) else { return }
        let removed = projects.remove(at: index)
        removed.allSessions.forEach { $0.shutdown() }

        if selectedProjectID == id {
            selectedProjectID = projects.isEmpty
                ? nil
                : projects[min(index, projects.count - 1)].id
        }
        refreshSessionVisibility()
        saveWorkspace()
    }

    /// Creates a new terminal tab. Splits are panes inside this tab.
    func newSession() {
        guard let projectID = selectedProjectID,
              let projectIndex = projects.firstIndex(where: { $0.id == projectID })
        else { return }

        let directory = projects[projectIndex].selectedSession?.workingDirectory
            ?? projects[projectIndex].rootPath
        let tab = makeTab(workingDirectory: directory, projectID: projectID)
        projects[projectIndex].tabs.append(tab)
        projects[projectIndex].selectedTabID = tab.id
        refreshSessionVisibility()
        saveWorkspace()
    }

    func newWorkspace() {
        let panel = NSOpenPanel()
        panel.title = "Open a workspace"
        panel.prompt = "Open Workspace"
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = selectedProject.map {
            URL(fileURLWithPath: $0.rootPath).deletingLastPathComponent()
        } ?? FileManager.default.homeDirectoryForCurrentUser
        guard panel.runModal() == .OK, let url = panel.url else { return }
        addProject(at: url)
    }

    func splitSelected(_ axis: WorkspaceSplitAxis) {
        guard let projectID = selectedProjectID,
              let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let tabID = projects[projectIndex].selectedTabID,
              let tabIndex = projects[projectIndex].tabs.firstIndex(where: { $0.id == tabID }),
              let selected = projects[projectIndex].tabs[tabIndex].selectedSession
        else { return }

        let session = makeSession(
            workingDirectory: selected.workingDirectory,
            projectID: projectID
        )
        let replacement = PaneLayoutSnapshot.split(
            axis: axis,
            first: .terminal(selected.id),
            second: .terminal(session.id)
        )
        projects[projectIndex].tabs[tabIndex].sessions.append(session)
        projects[projectIndex].tabs[tabIndex].layout = projects[projectIndex]
            .tabs[tabIndex].layout.replacingTerminal(selected.id, with: replacement)
        projects[projectIndex].tabs[tabIndex].selectedSessionID = session.id
        refreshSessionVisibility()
        saveWorkspace()
    }

    func focusAdjacentPane(_ offset: Int) {
        guard let projectID = selectedProjectID,
              let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let tabID = projects[projectIndex].selectedTabID,
              let tabIndex = projects[projectIndex].tabs.firstIndex(where: { $0.id == tabID }),
              let selectedID = projects[projectIndex].tabs[tabIndex].selectedSessionID
        else { return }
        let ids = projects[projectIndex].tabs[tabIndex].layout.terminalIDs
        guard ids.count > 1, let selectedIndex = ids.firstIndex(of: selectedID) else { return }
        let target = max(0, min(ids.count - 1, selectedIndex + offset))
        guard target != selectedIndex else { return }
        focusSession(ids[target], in: projectID)
    }

    func closeSelectedSession() {
        guard let projectID = selectedProjectID,
              let sessionID = selectedSession?.id
        else { return }
        closeSession(sessionID, in: projectID)
    }

    func closeSession(_ sessionID: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let tabIndex = projects[projectIndex].tabs.firstIndex(where: {
                  $0.sessions.contains(where: { $0.id == sessionID })
              }),
              let sessionIndex = projects[projectIndex].tabs[tabIndex]
                .sessions.firstIndex(where: { $0.id == sessionID })
        else { return }

        let session = projects[projectIndex].tabs[tabIndex].sessions.remove(at: sessionIndex)
        session.shutdown()

        guard let newLayout = projects[projectIndex].tabs[tabIndex]
            .layout.removingTerminal(sessionID) else {
            closeTab(at: tabIndex, in: projectIndex, shutdownSessions: false)
            refreshSessionVisibility()
            saveWorkspace()
            return
        }

        projects[projectIndex].tabs[tabIndex].layout = newLayout
        if projects[projectIndex].tabs[tabIndex].selectedSessionID == sessionID {
            projects[projectIndex].tabs[tabIndex].selectedSessionID = newLayout.terminalIDs.first
        }
        refreshSessionVisibility()
        saveWorkspace()
    }

    func closeTab(_ tabID: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let tabIndex = projects[projectIndex].tabs.firstIndex(where: { $0.id == tabID })
        else { return }
        closeTab(at: tabIndex, in: projectIndex, shutdownSessions: true)
        refreshSessionVisibility()
        saveWorkspace()
    }

    func saveWorkspace() {
        var snapshot = WorkspaceSnapshot(
            projects: projects.map { project in
                let tabSnapshots = project.tabs.map { tab in
                    TabSnapshot(
                        id: tab.id,
                        sessions: tab.sessions.map(sessionSnapshot),
                        selectedSessionID: tab.selectedSessionID,
                        layout: tab.layout
                    )
                }
                return ProjectSnapshot(
                    id: project.id,
                    name: project.name,
                    rootPath: project.rootPath,
                    sessions: tabSnapshots.flatMap(\.sessions),
                    selectedSessionID: project.selectedSession?.id,
                    visibleSessionIDs: project.selectedTab?.layout.terminalIDs,
                    splitAxis: project.selectedTab?.layout.rootAxis,
                    tabs: tabSnapshots,
                    selectedTabID: project.selectedTabID
                )
            },
            selectedProjectID: selectedProjectID
        )
        snapshot.normalizeSelection()

        do {
            let url = try Self.workspaceURL()
            let data = try JSONEncoder.vibra.encode(snapshot)
            try data.write(to: url, options: .atomic)
        } catch {
            assertionFailure("Unable to save workspace: \(error)")
        }
    }

    func shutdownAll() {
        saveWorkspace()
        hiddenSessionPump?.invalidate()
        hiddenSessionPump = nil
        allSessions.forEach { $0.shutdown() }
    }

    private func makeTab(workingDirectory: String, projectID: UUID) -> TerminalTab {
        let session = makeSession(workingDirectory: workingDirectory, projectID: projectID)
        return TerminalTab(
            id: UUID(),
            sessions: [session],
            selectedSessionID: session.id,
            layout: .terminal(session.id)
        )
    }

    private func makeSession(
        id: UUID = UUID(),
        workingDirectory: String,
        projectID: UUID
    ) -> TerminalSession {
        let session = TerminalSession(id: id, workingDirectory: workingDirectory)
        session.onExit = { [weak self] session in
            self?.closeSession(session.id, in: projectID)
        }
        return session
    }

    private func sessionSnapshot(_ session: TerminalSession) -> SessionSnapshot {
        SessionSnapshot(
            id: session.id,
            title: session.title,
            workingDirectory: session.workingDirectory
        )
    }

    private func closeTab(
        at tabIndex: Int,
        in projectIndex: Int,
        shutdownSessions: Bool
    ) {
        let removed = projects[projectIndex].tabs.remove(at: tabIndex)
        if shutdownSessions { removed.sessions.forEach { $0.shutdown() } }
        if projects[projectIndex].selectedTabID == removed.id {
            let tabs = projects[projectIndex].tabs
            projects[projectIndex].selectedTabID = tabs.isEmpty
                ? nil
                : tabs[min(tabIndex, tabs.count - 1)].id
        }
    }

    private func restoreWorkspace() {
        do {
            let data = try Data(contentsOf: Self.workspaceURL(createDirectory: false))
            var snapshot = try JSONDecoder().decode(WorkspaceSnapshot.self, from: data)
            snapshot.normalizeSelection()
            projects = snapshot.projects.compactMap { savedProject in
                guard FileManager.default.fileExists(atPath: savedProject.rootPath) else {
                    return nil
                }
                let tabs = (savedProject.tabs ?? []).compactMap { savedTab -> TerminalTab? in
                    let sessions = savedTab.sessions.map { savedSession in
                        makeSession(
                            id: savedSession.id,
                            workingDirectory: FileManager.default.fileExists(
                                atPath: savedSession.workingDirectory
                            ) ? savedSession.workingDirectory : savedProject.rootPath,
                            projectID: savedProject.id
                        )
                    }
                    guard !sessions.isEmpty else { return nil }
                    let sessionIDs = Set(sessions.map(\.id))
                    let layout = Set(savedTab.layout.terminalIDs).isSubset(of: sessionIDs)
                        ? savedTab.layout
                        : PaneLayoutSnapshot.joining(
                            sessions.map { .terminal($0.id) },
                            axis: .horizontal
                        )
                    return TerminalTab(
                        id: savedTab.id,
                        sessions: sessions,
                        selectedSessionID: sessions.contains(where: {
                            $0.id == savedTab.selectedSessionID
                        }) ? savedTab.selectedSessionID : sessions.first?.id,
                        layout: layout
                    )
                }
                return VibraProject(
                    id: savedProject.id,
                    name: savedProject.name,
                    rootPath: savedProject.rootPath,
                    tabs: tabs,
                    selectedTabID: tabs.contains(where: {
                        $0.id == savedProject.selectedTabID
                    }) ? savedProject.selectedTabID : tabs.first?.id
                )
            }
            selectedProjectID = projects.contains(where: {
                $0.id == snapshot.selectedProjectID
            }) ? snapshot.selectedProjectID : projects.first?.id
        } catch {
            projects = []
            selectedProjectID = nil
        }
    }

    private func refreshSessionVisibility() {
        let visibleIDs = Set(selectedTab?.layout.terminalIDs ?? [])
        for session in allSessions {
            session.setVisible(visibleIDs.contains(session.id))
        }

        let hasHiddenSessions = allSessions.contains { !$0.isVisible && !$0.isClosed }
        if hasHiddenSessions, hiddenSessionPump == nil {
            hiddenSessionPump = Timer.scheduledTimer(
                withTimeInterval: 0.2,
                repeats: true
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.allSessions.forEach { $0.tickWhileHidden() }
                }
            }
            if let hiddenSessionPump {
                RunLoop.main.add(hiddenSessionPump, forMode: .common)
            }
        } else if !hasHiddenSessions {
            hiddenSessionPump?.invalidate()
            hiddenSessionPump = nil
        }
    }

    private static func workspaceURL(createDirectory: Bool = true) throws -> URL {
        let base = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: createDirectory
        )
        let directory = base.appendingPathComponent("Vibra", isDirectory: true)
        if createDirectory {
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
        }
        return directory.appendingPathComponent("workspace.json")
    }

    private static func launchDirectory() -> URL? {
        guard let argument = ProcessInfo.processInfo.arguments.dropFirst().first,
              !argument.hasPrefix("-") else { return nil }
        let url = URL(fileURLWithPath: argument).standardizedFileURL
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory),
              isDirectory.boolValue else { return nil }
        return url
    }
}

private extension JSONEncoder {
    static var vibra: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return encoder
    }
}
