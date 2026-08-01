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

struct TerminalWorkspace: Identifiable {
    let id: UUID
    var name: String
    var titleSource: WorkspaceTitleSource
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

    @MainActor var suggestedTaskTitle: String? {
        selectedSession?.taskTitle ?? allSessions.compactMap(\.taskTitle).first
    }

    @MainActor var agentActivity: AgentActivity {
        let activities = allSessions.map { $0.agentActivity }
        if let attention = activities.first(where: {
            if case .needsAttention = $0 { true } else { false }
        }) { return attention }
        if let running = activities.first(where: {
            if case .running = $0 { true } else { false }
        }) { return running }
        if let selectedActivity = selectedSession?.agentActivity,
           selectedActivity != .idle {
            return selectedActivity
        }
        if let finished = activities.compactMap({ activity -> AgentActivity? in
            if case .finished = activity { return activity }
            return nil
        }).max(by: { lhs, rhs in
            guard case .finished(_, _, let leftDate) = lhs,
                  case .finished(_, _, let rightDate) = rhs else { return false }
            return leftDate < rightDate
        }) { return finished }
        if let ready = activities.first(where: {
            if case .ready = $0 { true } else { false }
        }) { return ready }
        return .idle
    }
}

struct VibraProject: Identifiable {
    let id: UUID
    var name: String
    var rootPath: String
    var workspaces: [TerminalWorkspace]
    var selectedWorkspaceID: UUID?

    var selectedWorkspace: TerminalWorkspace? {
        workspaces.first { $0.id == selectedWorkspaceID }
    }

    var selectedTab: TerminalTab? {
        selectedWorkspace?.selectedTab
    }

    var selectedSession: TerminalSession? {
        selectedTab?.selectedSession
    }

    var allSessions: [TerminalSession] {
        workspaces.flatMap(\.allSessions)
    }
}

struct LocatedTerminalWorkspace: Identifiable {
    let projectID: UUID
    let workspace: TerminalWorkspace

    var id: UUID { workspace.id }
}

struct TerminalWorkspaceFolder: Identifiable {
    let project: VibraProject

    var id: UUID { project.id }
}

enum RightSidebarMode: String {
    case files
    case changes
}

@MainActor
final class WorkspaceStore: ObservableObject {
    @Published private(set) var projects: [VibraProject] = []
    @Published private(set) var selectedProjectID: UUID?
    @Published private(set) var isTerminalSidebarVisible: Bool = {
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
    private var agentActivityTimer: Timer?
    private var isRefreshingAgentActivity = false
    private var lastWorkspaceActivities: [UUID: AgentActivity] = [:]
    private let persistsWorkspace: Bool
    /// Stored as nonisolated so `deinit` can remove it without hopping to the main actor.
    nonisolated(unsafe) private var appearanceObserver: NSObjectProtocol?

    init(restoresWorkspace: Bool = true) {
        persistsWorkspace = restoresWorkspace
        if restoresWorkspace {
            restoreWorkspace()
        }
        if restoresWorkspace, let launchDirectory = Self.launchDirectory() {
            addProject(at: launchDirectory, persist: false)
        } else if projects.isEmpty {
            addProject(
                at: URL(fileURLWithPath: FileManager.default.homeDirectoryForCurrentUser.path),
                persist: false
            )
        }
        if !restoresWorkspace {
            isGitSidebarVisible = false
        }
        refreshSessionVisibility()
        lastWorkspaceActivities = workspaceActivitySnapshot()
        startAgentActivityMonitoring()
        observeTerminalAppearanceChanges()
        saveWorkspace()
    }

    deinit {
        if let appearanceObserver {
            NotificationCenter.default.removeObserver(appearanceObserver)
        }
    }

    var selectedProject: VibraProject? {
        projects.first { $0.id == selectedProjectID }
    }

    var selectedTab: TerminalTab? {
        selectedProject?.selectedTab
    }

    var selectedWorkspace: TerminalWorkspace? {
        selectedProject?.selectedWorkspace
    }

    var selectedSession: TerminalSession? {
        selectedTab?.selectedSession
    }

    var allSessions: [TerminalSession] {
        projects.flatMap(\.allSessions)
    }

    var tabCount: Int {
        projects.reduce(0) { $0 + $1.workspaces.count }
    }

    var ungroupedWorkspaces: [LocatedTerminalWorkspace] {
        projects.filter { $0.name.isEmpty }.flatMap { project in
            project.workspaces.map { workspace in
                LocatedTerminalWorkspace(projectID: project.id, workspace: workspace)
            }
        }
    }

    var workspaceFolders: [TerminalWorkspaceFolder] {
        projects.filter { !$0.name.isEmpty }.map(TerminalWorkspaceFolder.init(project:))
    }

    var sidebarWorkspaces: [LocatedTerminalWorkspace] {
        ungroupedWorkspaces + workspaceFolders.flatMap { folder in
            folder.project.workspaces.map {
                LocatedTerminalWorkspace(projectID: folder.id, workspace: $0)
            }
        }
    }

    /// Selects a workspace by its visible left-sidebar position. This mirrors
    /// cmux's vertical workspace navigation while keeping terminal tabs scoped
    /// to the selected workspace.
    func selectWorkspace(at sidebarIndex: Int) {
        guard sidebarWorkspaces.indices.contains(sidebarIndex) else { return }
        let workspace = sidebarWorkspaces[sidebarIndex]
        selectWorkspace(workspace.id, in: workspace.projectID)
    }

    func selectAdjacentWorkspace(_ offset: Int) {
        let workspaces = sidebarWorkspaces
        guard !workspaces.isEmpty, offset != 0 else { return }
        let currentIndex = workspaces.firstIndex { $0.id == selectedWorkspace?.id } ?? 0
        let targetIndex = (currentIndex + offset % workspaces.count + workspaces.count)
            % workspaces.count
        let target = workspaces[targetIndex]
        selectWorkspace(target.id, in: target.projectID)
    }

    func selectProject(_ id: UUID) {
        guard projects.contains(where: { $0.id == id }) else { return }
        selectedProjectID = id
        refreshSessionVisibility()
        saveWorkspace()
    }

    func selectWorkspace(_ id: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              projects[projectIndex].workspaces.contains(where: { $0.id == id })
        else { return }
        projects[projectIndex].selectedWorkspaceID = id
        selectedProjectID = projectID
        refreshSessionVisibility()
        saveWorkspace()
    }

    func renameWorkspace(_ id: UUID, in projectID: UUID, to rawName: String) {
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty,
              let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.id == id
              })
        else { return }
        projects[projectIndex].workspaces[workspaceIndex].name = name
        projects[projectIndex].workspaces[workspaceIndex].titleSource = .manual
        saveWorkspace()
    }

    func useAutomaticWorkspaceTitle(_ id: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.id == id
              })
        else { return }
        projects[projectIndex].workspaces[workspaceIndex].titleSource = .automatic
        let suggested = projects[projectIndex].workspaces[workspaceIndex].suggestedTaskTitle
        projects[projectIndex].workspaces[workspaceIndex].name = suggested ?? "Nueva tarea"
        saveWorkspace()
    }

    func selectTab(_ id: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.tabs.contains(where: { $0.id == id })
              })
        else { return }
        projects[projectIndex].selectedWorkspaceID = projects[projectIndex]
            .workspaces[workspaceIndex].id
        projects[projectIndex].workspaces[workspaceIndex].selectedTabID = id
        selectedProjectID = projectID
        refreshSessionVisibility()
        saveWorkspace()
    }

    func focusSession(_ id: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.tabs.contains(where: { tab in
                      tab.sessions.contains(where: { $0.id == id })
                  })
              }),
              let tabIndex = projects[projectIndex].workspaces[workspaceIndex]
                .tabs.firstIndex(where: {
                    $0.sessions.contains(where: { $0.id == id })
              })
        else { return }
        projects[projectIndex].selectedWorkspaceID = projects[projectIndex]
            .workspaces[workspaceIndex].id
        projects[projectIndex].workspaces[workspaceIndex].selectedTabID = projects[projectIndex]
            .workspaces[workspaceIndex].tabs[tabIndex].id
        projects[projectIndex].workspaces[workspaceIndex]
            .tabs[tabIndex].selectedSessionID = id
        selectedProjectID = projectID
        refreshSessionVisibility()
        saveWorkspace()
    }

    func toggleTerminalSidebar() {
        isTerminalSidebarVisible.toggle()
        UserDefaults.standard.set(
            isTerminalSidebarVisible,
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
        addWorkspace(
            workingDirectory: url.standardizedFileURL.path,
            persist: persist
        )
    }

    func chooseFolder() {
        let panel = NSOpenPanel()
        panel.title = "Open a folder in a new tab"
        panel.prompt = "Open Folder"
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = selectedProject.map {
            let projectURL = URL(fileURLWithPath: $0.rootPath).standardizedFileURL
            let parentURL = projectURL.deletingLastPathComponent()
            return parentURL.path.isEmpty ? projectURL : parentURL
        }
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

    /// Creates a horizontal terminal tab inside the selected vertical tab.
    func newSession() {
        selectedSession?.refreshWorkingDirectory()
        guard let projectID = selectedProjectID,
              let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceID = projects[projectIndex].selectedWorkspaceID,
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.id == workspaceID
              })
        else {
            newWorkspace()
            return
        }
        let directory = projects[projectIndex].selectedSession?.workingDirectory
            ?? projects[projectIndex].rootPath
        let tab = makeTab(workingDirectory: directory, projectID: projectID)
        projects[projectIndex].workspaces[workspaceIndex].tabs.append(tab)
        projects[projectIndex].workspaces[workspaceIndex].selectedTabID = tab.id
        refreshSessionVisibility()
        saveWorkspace()
    }

    /// Creates a vertical tab in the active terminal directory. Supplying a
    /// space keeps the new agent within that space instead of moving it into
    /// the ungrouped local checkout.
    func newWorkspace(in requestedProjectID: UUID? = nil) {
        selectedSession?.refreshWorkingDirectory()
        let directory = selectedSession?.workingDirectory
            ?? FileManager.default.homeDirectoryForCurrentUser.path
        addWorkspace(
            workingDirectory: directory,
            persist: true,
            projectID: requestedProjectID
        )
    }

    func createFolder(named rawName: String, containing requestedWorkspaceID: UUID? = nil) {
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        let workspaceID = requestedWorkspaceID ?? selectedWorkspace?.id
        guard !name.isEmpty, let workspaceID,
              let projectIndex = projects.firstIndex(where: {
                  $0.workspaces.contains(where: { $0.id == workspaceID })
              }),
              let workspace = projects[projectIndex].workspaces.first(where: {
                  $0.id == workspaceID
              })
        else { return }

        if let existing = projects.first(where: {
            $0.name.localizedCaseInsensitiveCompare(name) == .orderedSame
        }) {
            moveWorkspace(workspaceID, to: existing.id)
            return
        }

        let folder = VibraProject(
            id: UUID(),
            name: name,
            rootPath: workspace.selectedSession?.workingDirectory
                ?? FileManager.default.homeDirectoryForCurrentUser.path,
            workspaces: [],
            selectedWorkspaceID: nil
        )
        projects.append(folder)
        moveWorkspace(workspaceID, to: folder.id)
    }

    func renameFolder(_ projectID: UUID, to rawName: String) {
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty,
              let projectIndex = projects.firstIndex(where: { $0.id == projectID })
        else { return }
        projects[projectIndex].name = name
        saveWorkspace()
    }

    func moveWorkspace(_ workspaceID: UUID, to targetProjectID: UUID) {
        guard let sourceProjectIndex = projects.firstIndex(where: {
            $0.workspaces.contains(where: { $0.id == workspaceID })
        }),
              let sourceWorkspaceIndex = projects[sourceProjectIndex].workspaces.firstIndex(
                where: { $0.id == workspaceID }
              ),
              projects.contains(where: { $0.id == targetProjectID })
        else { return }

        if projects[sourceProjectIndex].id == targetProjectID {
            selectWorkspace(workspaceID, in: targetProjectID)
            return
        }

        let workspace = projects[sourceProjectIndex].workspaces.remove(at: sourceWorkspaceIndex)
        workspace.allSessions.forEach { bindSession($0, to: targetProjectID) }
        if projects[sourceProjectIndex].workspaces.isEmpty {
            projects.remove(at: sourceProjectIndex)
        } else if projects[sourceProjectIndex].selectedWorkspaceID == workspaceID {
            projects[sourceProjectIndex].selectedWorkspaceID = projects[sourceProjectIndex]
                .workspaces.first?.id
        }

        guard let targetProjectIndex = projects.firstIndex(where: { $0.id == targetProjectID })
        else { return }
        projects[targetProjectIndex].workspaces.append(workspace)
        projects[targetProjectIndex].selectedWorkspaceID = workspace.id
        selectedProjectID = targetProjectID
        refreshSessionVisibility()
        saveWorkspace()
    }

    func moveWorkspaceToUngrouped(_ workspaceID: UUID) {
        let projectID = ensureUngroupedProject()
        moveWorkspace(workspaceID, to: projectID)
    }

    func deleteFolder(_ projectID: UUID) {
        guard let project = projects.first(where: { $0.id == projectID && !$0.name.isEmpty })
        else { return }
        for workspaceID in project.workspaces.map(\.id) {
            moveWorkspaceToUngrouped(workspaceID)
        }
    }

    func splitSelected(_ axis: WorkspaceSplitAxis) {
        guard let projectID = selectedProjectID,
              let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceID = projects[projectIndex].selectedWorkspaceID,
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.id == workspaceID
              }),
              let tabID = projects[projectIndex].workspaces[workspaceIndex].selectedTabID,
              let tabIndex = projects[projectIndex].workspaces[workspaceIndex]
                .tabs.firstIndex(where: { $0.id == tabID }),
              let selected = projects[projectIndex].workspaces[workspaceIndex]
                .tabs[tabIndex].selectedSession
        else { return }

        selected.refreshWorkingDirectory()
        let session = makeSession(
            workingDirectory: selected.workingDirectory,
            projectID: projectID
        )
        let replacement = PaneLayoutSnapshot.split(
            axis: axis,
            first: .terminal(selected.id),
            second: .terminal(session.id)
        )
        projects[projectIndex].workspaces[workspaceIndex].tabs[tabIndex]
            .sessions.append(session)
        projects[projectIndex].workspaces[workspaceIndex].tabs[tabIndex].layout = projects[
            projectIndex
        ].workspaces[workspaceIndex].tabs[tabIndex].layout.replacingTerminal(
            selected.id,
            with: replacement
        )
        projects[projectIndex].workspaces[workspaceIndex]
            .tabs[tabIndex].selectedSessionID = session.id
        refreshSessionVisibility()
        saveWorkspace()
    }

    func focusAdjacentPane(_ offset: Int) {
        guard let projectID = selectedProjectID,
              let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceID = projects[projectIndex].selectedWorkspaceID,
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.id == workspaceID
              }),
              let tabID = projects[projectIndex].workspaces[workspaceIndex].selectedTabID,
              let tabIndex = projects[projectIndex].workspaces[workspaceIndex]
                .tabs.firstIndex(where: { $0.id == tabID }),
              let selectedID = projects[projectIndex].workspaces[workspaceIndex]
                .tabs[tabIndex].selectedSessionID
        else { return }
        let ids = projects[projectIndex].workspaces[workspaceIndex]
            .tabs[tabIndex].layout.terminalIDs
        guard ids.count > 1, let selectedIndex = ids.firstIndex(of: selectedID) else { return }
        let target = max(0, min(ids.count - 1, selectedIndex + offset))
        guard target != selectedIndex else { return }
        focusSession(ids[target], in: projectID)
    }

    func observeTerminalKeyEvent(_ event: NSEvent) {
        guard event.keyCode == 36 || event.keyCode == 76,
              let session = selectedSession,
              event.window?.firstResponder === session.terminalView
        else { return }
        session.noteUserSubmittedInput()
    }

    func closeSelectedSession() {
        guard let projectID = selectedProjectID,
              let sessionID = selectedSession?.id
        else { return }
        closeSession(sessionID, in: projectID)
    }

    func closeSession(_ sessionID: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.tabs.contains(where: { tab in
                      tab.sessions.contains(where: { $0.id == sessionID })
                  })
              }),
              let tabIndex = projects[projectIndex].workspaces[workspaceIndex]
                .tabs.firstIndex(where: {
                    $0.sessions.contains(where: { $0.id == sessionID })
              }),
              let sessionIndex = projects[projectIndex].workspaces[workspaceIndex].tabs[tabIndex]
                .sessions.firstIndex(where: { $0.id == sessionID })
        else { return }

        let session = projects[projectIndex].workspaces[workspaceIndex]
            .tabs[tabIndex].sessions.remove(at: sessionIndex)
        session.shutdown()

        guard let newLayout = projects[projectIndex].workspaces[workspaceIndex].tabs[tabIndex]
            .layout.removingTerminal(sessionID) else {
            closeTab(
                at: tabIndex,
                in: workspaceIndex,
                projectIndex: projectIndex,
                shutdownSessions: false
            )
            refreshSessionVisibility()
            saveWorkspace()
            return
        }

        projects[projectIndex].workspaces[workspaceIndex].tabs[tabIndex].layout = newLayout
        if projects[projectIndex].workspaces[workspaceIndex]
            .tabs[tabIndex].selectedSessionID == sessionID {
            projects[projectIndex].workspaces[workspaceIndex]
                .tabs[tabIndex].selectedSessionID = newLayout.terminalIDs.first
        }
        refreshSessionVisibility()
        saveWorkspace()
    }

    func closeTab(_ tabID: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.tabs.contains(where: { $0.id == tabID })
              }),
              let tabIndex = projects[projectIndex].workspaces[workspaceIndex]
                .tabs.firstIndex(where: { $0.id == tabID })
        else { return }
        closeTab(
            at: tabIndex,
            in: workspaceIndex,
            projectIndex: projectIndex,
            shutdownSessions: true
        )
        refreshSessionVisibility()
        saveWorkspace()
    }

    func closeWorkspace(_ workspaceID: UUID, in projectID: UUID) {
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }),
              let workspaceIndex = projects[projectIndex].workspaces.firstIndex(where: {
                  $0.id == workspaceID
              })
        else { return }
        let removed = projects[projectIndex].workspaces.remove(at: workspaceIndex)
        removed.allSessions.forEach { $0.shutdown() }
        if projects[projectIndex].selectedWorkspaceID == removed.id {
            let workspaces = projects[projectIndex].workspaces
            projects[projectIndex].selectedWorkspaceID = workspaces.isEmpty
                ? nil
                : workspaces[min(workspaceIndex, workspaces.count - 1)].id
        }
        if projects[projectIndex].workspaces.isEmpty {
            projects.remove(at: projectIndex)
            if selectedProjectID == projectID {
                selectedProjectID = projects.first?.id
            }
        }
        refreshSessionVisibility()
        saveWorkspace()
    }

    func closeSelectedWorkspace() {
        guard let projectID = selectedProjectID, let workspaceID = selectedWorkspace?.id else {
            return
        }
        closeWorkspace(workspaceID, in: projectID)
    }

    func saveWorkspace() {
        guard persistsWorkspace else { return }
        var snapshot = WorkspaceSnapshot(
            projects: projects.map { project in
                let workspaceSnapshots = project.workspaces.map { workspace in
                    TerminalWorkspaceSnapshot(
                        id: workspace.id,
                        name: workspace.name,
                        titleSource: workspace.titleSource,
                        tabs: workspace.tabs.map(tabSnapshot),
                        selectedTabID: workspace.selectedTabID
                    )
                }
                let selectedWorkspace = workspaceSnapshots.first {
                    $0.id == project.selectedWorkspaceID
                }
                return ProjectSnapshot(
                    id: project.id,
                    name: project.name,
                    rootPath: project.rootPath,
                    sessions: workspaceSnapshots.flatMap { $0.tabs }.flatMap(\.sessions),
                    selectedSessionID: project.selectedSession?.id,
                    visibleSessionIDs: project.selectedTab?.layout.terminalIDs,
                    splitAxis: project.selectedTab?.layout.rootAxis,
                    tabs: selectedWorkspace?.tabs,
                    selectedTabID: selectedWorkspace?.selectedTabID,
                    workspaces: workspaceSnapshots,
                    selectedWorkspaceID: project.selectedWorkspaceID
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
        agentActivityTimer?.invalidate()
        agentActivityTimer = nil
        allSessions.forEach { $0.shutdown() }
    }

    private func ensureUngroupedProject() -> UUID {
        if let project = projects.first(where: { $0.name.isEmpty }) {
            return project.id
        }
        let project = VibraProject(
            id: UUID(),
            name: "",
            rootPath: FileManager.default.homeDirectoryForCurrentUser.path,
            workspaces: [],
            selectedWorkspaceID: nil
        )
        projects.insert(project, at: 0)
        return project.id
    }

    private func addWorkspace(
        workingDirectory: String,
        persist: Bool,
        projectID requestedProjectID: UUID? = nil
    ) {
        let projectID = requestedProjectID.flatMap { candidate in
            projects.contains(where: { $0.id == candidate }) ? candidate : nil
        } ?? ensureUngroupedProject()
        guard let projectIndex = projects.firstIndex(where: { $0.id == projectID }) else { return }
        let directoryURL = URL(fileURLWithPath: workingDirectory).standardizedFileURL
        let workspace = makeWorkspace(
            name: "Nueva tarea",
            titleSource: .automatic,
            workingDirectory: directoryURL.path,
            projectID: projectID
        )
        projects[projectIndex].workspaces.append(workspace)
        projects[projectIndex].selectedWorkspaceID = workspace.id
        selectedProjectID = projectID
        refreshSessionVisibility()
        if persist { saveWorkspace() }
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

    private func makeWorkspace(
        name: String,
        titleSource: WorkspaceTitleSource,
        workingDirectory: String,
        projectID: UUID
    ) -> TerminalWorkspace {
        let tab = makeTab(workingDirectory: workingDirectory, projectID: projectID)
        return TerminalWorkspace(
            id: UUID(),
            name: name,
            titleSource: titleSource,
            tabs: [tab],
            selectedTabID: tab.id
        )
    }

    private func makeSession(
        id: UUID = UUID(),
        workingDirectory: String,
        projectID: UUID
    ) -> TerminalSession {
        let session = TerminalSession(id: id, workingDirectory: workingDirectory)
        bindSession(session, to: projectID)
        return session
    }

    private func bindSession(_ session: TerminalSession, to projectID: UUID) {
        session.onExit = { [weak self] session in
            self?.closeSession(session.id, in: projectID)
        }
    }

    private func observeTerminalAppearanceChanges() {
        appearanceObserver = NotificationCenter.default.addObserver(
            forName: .terminalAppearanceDidChange,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            guard let self else { return }
            Task { @MainActor in
                self.applyTerminalAppearanceToAllSessions()
            }
        }
    }

    private func applyTerminalAppearanceToAllSessions() {
        TerminalAppearanceApplier.apply(TerminalAppearance.current, to: allSessions)
    }

    private func sessionSnapshot(_ session: TerminalSession) -> SessionSnapshot {
        SessionSnapshot(
            id: session.id,
            title: session.title,
            workingDirectory: session.workingDirectory
        )
    }

    private func tabSnapshot(_ tab: TerminalTab) -> TabSnapshot {
        TabSnapshot(
            id: tab.id,
            sessions: tab.sessions.map(sessionSnapshot),
            selectedSessionID: tab.selectedSessionID,
            layout: tab.layout
        )
    }

    private func closeTab(
        at tabIndex: Int,
        in workspaceIndex: Int,
        projectIndex: Int,
        shutdownSessions: Bool
    ) {
        let removed = projects[projectIndex].workspaces[workspaceIndex]
            .tabs.remove(at: tabIndex)
        if shutdownSessions { removed.sessions.forEach { $0.shutdown() } }
        if projects[projectIndex].workspaces[workspaceIndex].selectedTabID == removed.id {
            let tabs = projects[projectIndex].workspaces[workspaceIndex].tabs
            projects[projectIndex].workspaces[workspaceIndex].selectedTabID = tabs.isEmpty
                ? nil
                : tabs[min(tabIndex, tabs.count - 1)].id
        }
        guard projects[projectIndex].workspaces[workspaceIndex].tabs.isEmpty else { return }
        let removedWorkspace = projects[projectIndex].workspaces.remove(at: workspaceIndex)
        if projects[projectIndex].selectedWorkspaceID == removedWorkspace.id {
            let workspaces = projects[projectIndex].workspaces
            projects[projectIndex].selectedWorkspaceID = workspaces.isEmpty
                ? nil
                : workspaces[min(workspaceIndex, workspaces.count - 1)].id
        }
        if projects[projectIndex].workspaces.isEmpty {
            let projectID = projects[projectIndex].id
            projects.remove(at: projectIndex)
            if selectedProjectID == projectID {
                selectedProjectID = projects.first?.id
            }
        }
    }

    private func restoreWorkspace() {
        do {
            let data = try Data(contentsOf: Self.workspaceURL(createDirectory: false))
            var snapshot = try JSONDecoder().decode(WorkspaceSnapshot.self, from: data)
            snapshot.normalizeSelection()
            let migrateLegacyProjects = !UserDefaults.standard.bool(
                forKey: SettingsKeys.tabFolderModelMigrated
            )
            projects = snapshot.projects.compactMap { savedProject -> VibraProject? in
                guard FileManager.default.fileExists(atPath: savedProject.rootPath) else {
                    return nil
                }
                let workspaces = (savedProject.workspaces ?? []).compactMap {
                    savedWorkspace -> TerminalWorkspace? in
                    let tabs = savedWorkspace.tabs.compactMap { savedTab in
                        restoreTab(
                            savedTab,
                            rootPath: savedProject.rootPath,
                            projectID: savedProject.id
                        )
                    }
                    guard !tabs.isEmpty else { return nil }
                    let workingDirectory = tabs.first?.selectedSession?.workingDirectory
                    let inferredName = workingDirectory.map {
                        URL(fileURLWithPath: $0).lastPathComponent
                    }
                    let workspaceName = savedWorkspace.name.isEmpty
                        ? (inferredName.flatMap { $0.isEmpty ? nil : $0 } ?? "Terminal")
                        : savedWorkspace.name
                    let titleSource = savedWorkspace.titleSource ?? inferredTitleSource(
                        for: workspaceName,
                        workingDirectory: workingDirectory
                    )
                    return TerminalWorkspace(
                        id: savedWorkspace.id,
                        name: workspaceName,
                        titleSource: titleSource,
                        tabs: tabs,
                        selectedTabID: tabs.contains(where: {
                            $0.id == savedWorkspace.selectedTabID
                        }) ? savedWorkspace.selectedTabID : tabs.first?.id
                    )
                }
                return VibraProject(
                    id: savedProject.id,
                    name: migrateLegacyProjects ? "" : savedProject.name,
                    rootPath: savedProject.rootPath,
                    workspaces: workspaces,
                    selectedWorkspaceID: workspaces.contains(where: {
                        $0.id == savedProject.selectedWorkspaceID
                    }) ? savedProject.selectedWorkspaceID : workspaces.first?.id
                )
            }
            selectedProjectID = projects.contains(where: {
                $0.id == snapshot.selectedProjectID
            }) ? snapshot.selectedProjectID : projects.first?.id
            if migrateLegacyProjects {
                UserDefaults.standard.set(true, forKey: SettingsKeys.tabFolderModelMigrated)
            }
        } catch {
            projects = []
            selectedProjectID = nil
        }
    }

    private func restoreTab(
        _ savedTab: TabSnapshot,
        rootPath: String,
        projectID: UUID
    ) -> TerminalTab? {
        let sessions = savedTab.sessions.map { savedSession in
            makeSession(
                id: savedSession.id,
                workingDirectory: FileManager.default.fileExists(
                    atPath: savedSession.workingDirectory
                ) ? savedSession.workingDirectory : rootPath,
                projectID: projectID
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

    private func inferredTitleSource(
        for name: String,
        workingDirectory: String?
    ) -> WorkspaceTitleSource {
        let directoryName = workingDirectory.map {
            URL(fileURLWithPath: $0).lastPathComponent
        }
        return name == directoryName || name == "Terminal" ? .automatic : .manual
    }

    private func refreshSessionVisibility() {
        let visibleIDs = Set(selectedTab?.layout.terminalIDs ?? [])
        for session in allSessions {
            session.setVisible(visibleIDs.contains(session.id))
        }

        let hasHiddenSessions = allSessions.contains { !$0.isVisible && !$0.isClosed }
        if hasHiddenSessions, hiddenSessionPump == nil {
            hiddenSessionPump = Timer.scheduledTimer(
                withTimeInterval: 0.35,
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

    private func startAgentActivityMonitoring() {
        guard agentActivityTimer == nil else { return }
        let timer = Timer.scheduledTimer(withTimeInterval: 1.4, repeats: true) {
            [weak self] _ in
            Task { @MainActor [weak self] in
                await self?.refreshAgentActivity()
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        agentActivityTimer = timer
        Task { await refreshAgentActivity() }
    }

    private func refreshAgentActivity() async {
        guard !isRefreshingAgentActivity else { return }
        isRefreshingAgentActivity = true
        defer { isRefreshingAgentActivity = false }

        allSessions.forEach { $0.refreshWorkingDirectory() }

        let sessionsByPID = Dictionary(
            uniqueKeysWithValues: allSessions.compactMap { session in
                session.terminalView.foregroundPid.map { ($0, session.id) }
            }
        )
        let pids = Set(sessionsByPID.keys)
        let runtime = await Task.detached(priority: .utility) {
            let processes = AgentProcessProbe.snapshots(for: pids)
            let agentSessionIDs = Set<UUID>(processes.compactMap { pid, snapshot in
                guard CodingAgent.detect(commandLine: snapshot.commandLine) != nil else {
                    return nil
                }
                return sessionsByPID[pid]
            })
            let lifecycle = Dictionary(
                uniqueKeysWithValues: agentSessionIDs.compactMap { sessionID in
                    AgentLifecycleStore.snapshot(for: sessionID).map { (sessionID, $0) }
                }
            )
            return (processes, lifecycle)
        }.value

        let sessionsByID = Dictionary(uniqueKeysWithValues: allSessions.map { ($0.id, $0) })
        for (pid, sessionID) in sessionsByPID {
            sessionsByID[sessionID]?.updateForegroundProcess(
                runtime.0[pid],
                lifecycle: runtime.1[sessionID]
            )
        }
        let sessionsWithoutProcess = Set(sessionsByID.keys).subtracting(sessionsByPID.values)
        for sessionID in sessionsWithoutProcess {
            sessionsByID[sessionID]?.updateForegroundProcess(
                nil,
                lifecycle: runtime.1[sessionID]
            )
        }
        let didUpdateWorkspaceTitles = refreshAutomaticWorkspaceTitles()
        if didUpdateWorkspaceTitles {
            saveWorkspace()
        }
        let activities = workspaceActivitySnapshot()
        if activities != lastWorkspaceActivities || didUpdateWorkspaceTitles {
            lastWorkspaceActivities = activities
            objectWillChange.send()
        }
    }

    private func refreshAutomaticWorkspaceTitles() -> Bool {
        var didUpdate = false
        for projectIndex in projects.indices {
            for workspaceIndex in projects[projectIndex].workspaces.indices {
                guard projects[projectIndex].workspaces[workspaceIndex].titleSource == .automatic,
                      let title = projects[projectIndex].workspaces[workspaceIndex].suggestedTaskTitle,
                      projects[projectIndex].workspaces[workspaceIndex].name != title
                else { continue }
                projects[projectIndex].workspaces[workspaceIndex].name = title
                didUpdate = true
            }
        }
        return didUpdate
    }

    private func workspaceActivitySnapshot() -> [UUID: AgentActivity] {
        Dictionary(uniqueKeysWithValues: projects.flatMap { project in
            project.workspaces.map { ($0.id, $0.agentActivity) }
        })
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
