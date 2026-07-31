import Combine
import GhosttyTerminal
import SwiftUI
import VibraCore

enum WorkspaceWindowMode {
    case project
    case terminal
}

struct WorkspaceRootView: View {
    @Environment(\.openWindow) private var openWindow
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject private var chromeTheme = ChromeThemeController.shared
    @StateObject private var store: WorkspaceStore
    @StateObject private var gitModel = GitSidebarModel()
    @State private var terminalSidebarWidth: CGFloat
    @State private var gitSidebarWidth: CGFloat
    @State private var sidebarDragOriginWidth: CGFloat?
    @State private var gitSidebarDragOriginWidth: CGFloat?
    private let mode: WorkspaceWindowMode

    init(mode: WorkspaceWindowMode = .project) {
        self.mode = mode
        let savedWidth = UserDefaults.standard.double(forKey: SettingsKeys.terminalSidebarWidth)
        let savedGitWidth = UserDefaults.standard.double(forKey: SettingsKeys.gitSidebarWidth)
        _terminalSidebarWidth = State(initialValue: savedWidth > 0 ? savedWidth : 270)
        _gitSidebarWidth = State(initialValue: savedGitWidth > 0 ? savedGitWidth : 420)
        _store = StateObject(
            wrappedValue: WorkspaceStore(restoresWorkspace: mode == .project)
        )
    }

    var body: some View {
        ZStack(alignment: .top) {
            HStack(spacing: VibraLayout.panelGap) {
                if store.isTerminalSidebarVisible {
                    TerminalSidebar(store: store)
                        .frame(width: terminalSidebarWidth)
                        .overlay(alignment: .trailing) { sidebarDivider }
                        .transition(.move(edge: .leading).combined(with: .opacity))
                }
                WorkspaceDetail(store: store)
                    .vibraContentPanel()
                    .padding(.vertical, VibraLayout.contentInset)
                if store.isGitSidebarVisible,
                   let project = store.selectedProject {
                    GitSidebarView(
                        fallbackRoot: project.rootPath,
                        session: project.selectedSession,
                        store: store,
                        model: gitModel
                    )
                    .frame(width: effectiveGitSidebarWidth)
                    .vibraContentPanel()
                    .padding(.vertical, VibraLayout.contentInset)
                    .overlay(alignment: .leading) { gitSidebarDivider }
                    .transition(.move(edge: .trailing).combined(with: .opacity))
                }
            }
            .padding(.top, VibraLayout.windowChromeHeight)
            .padding(.leading, store.isTerminalSidebarVisible ? 0 : VibraLayout.contentInset)
            .padding(.trailing, VibraLayout.contentInset)
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            SessionHeader(
                store: store,
                sidebarWidth: store.isTerminalSidebarVisible ? terminalSidebarWidth : nil
            )
            .zIndex(1)
        }
        .animation(
            .spring(response: 0.34, dampingFraction: 0.9),
            value: store.isTerminalSidebarVisible
        )
        .animation(
            .spring(response: 0.34, dampingFraction: 0.9),
            value: store.isGitSidebarVisible
        )
        .frame(minWidth: store.isGitSidebarVisible ? 980 : 760, minHeight: 500)
        .background(chromeTheme.theme.recessed)
        // The titlebar is hidden, but macOS still reserves its height unless
        // the root view explicitly consumes the top safe-area inset. The
        // window chrome above then occupies that space instead of leaving a
        // blank strip over the panels.
        .ignoresSafeArea(.container, edges: .top)
        .environment(\.appChrome, chromeTheme.theme)
        .background {
            KeyboardShortcutMonitor(
                store: store,
                newTerminalWindow: { openWindow(id: VibraWindowID.terminal) }
            )
                .frame(width: 0, height: 0)
        }
        .background {
            if let project = store.selectedProject,
               let session = project.selectedSession {
                GitContextSync(
                    session: session,
                    fallbackRoot: project.rootPath,
                    model: gitModel
                )
                .id(session.id)
            } else if let project = store.selectedProject {
                Color.clear.onAppear { gitModel.sync(root: project.rootPath) }
            }
        }
        .overlay {
            if gitModel.isDiffPresented {
                GitDiffModalView(model: gitModel)
                    .transition(.opacity.combined(with: .scale(scale: 0.985)))
            }
        }
        .animation(.easeOut(duration: 0.16), value: gitModel.isDiffPresented)
        .focusedSceneObject(store)
        .onAppear { chromeTheme.refresh(colorScheme: colorScheme) }
        .onChange(of: colorScheme) { _, scheme in
            chromeTheme.refresh(colorScheme: scheme)
        }
        .onReceive(
            NotificationCenter.default.publisher(
                for: NSApplication.willTerminateNotification
            )
        ) { _ in
            store.shutdownAll()
        }
        .onDisappear {
            store.shutdownAll()
        }
    }

    private var effectiveGitSidebarWidth: CGFloat {
        gitSidebarWidth + (gitModel.expandedChangeID == nil ? 0 : 140)
    }

    private var sidebarDivider: some View {
        Color.clear
            .frame(width: 10)
            .overlay(alignment: .trailing) {
                Rectangle()
                    .fill(chromeTheme.theme.quietBorder)
                    .frame(width: 1)
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 1)
                    .onChanged { value in
                        if sidebarDragOriginWidth == nil {
                            sidebarDragOriginWidth = terminalSidebarWidth
                        }
                        let origin = sidebarDragOriginWidth ?? terminalSidebarWidth
                        terminalSidebarWidth = min(max(origin + value.translation.width, 208), 380)
                    }
                    .onEnded { _ in
                        sidebarDragOriginWidth = nil
                        UserDefaults.standard.set(
                            Double(terminalSidebarWidth),
                            forKey: SettingsKeys.terminalSidebarWidth
                        )
                    }
            )
            .onHover { hovering in
                if hovering {
                    NSCursor.resizeLeftRight.push()
                } else {
                    NSCursor.pop()
                }
            }
    }

    private var gitSidebarDivider: some View {
        Color.clear
            .frame(width: 10)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 1)
                    .onChanged { value in
                        if gitSidebarDragOriginWidth == nil {
                            gitSidebarDragOriginWidth = gitSidebarWidth
                        }
                        let origin = gitSidebarDragOriginWidth ?? gitSidebarWidth
                        gitSidebarWidth = min(max(origin - value.translation.width, 300), 680)
                    }
                    .onEnded { _ in
                        gitSidebarDragOriginWidth = nil
                        UserDefaults.standard.set(
                            Double(gitSidebarWidth),
                            forKey: SettingsKeys.gitSidebarWidth
                        )
                    }
            )
            .onHover { hovering in
                if hovering {
                    NSCursor.resizeLeftRight.push()
                } else {
                    NSCursor.pop()
                }
            }
    }
}

private struct TerminalSidebar: View {
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome
    @State private var isCreatingFolder = false
    @State private var folderName = ""
    @State private var folderTargetWorkspaceID: UUID?
    @State private var isUngroupedDropTargeted = false
    @FocusState private var isFolderNameFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            sidebarHeader
            Divider().overlay(chrome.quietBorder)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 4) {
                    VStack(spacing: 1) {
                        ForEach(store.ungroupedWorkspaces) { located in
                            SidebarWorkspaceRow(
                                located: located,
                                store: store,
                                createFolder: { beginCreatingFolder(containing: located.id) }
                            )
                        }
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.bottom, store.workspaceFolders.isEmpty ? 0 : 4)
                    .background {
                        if isUngroupedDropTargeted {
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(chrome.workspaceSelection)
                                .overlay {
                                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                                        .stroke(chrome.strongBorder, lineWidth: 1)
                                }
                        }
                    }
                    .dropDestination(for: String.self) { items, _ in
                        guard let value = items.first, let id = UUID(uuidString: value) else {
                            return false
                        }
                        withAnimation(.spring(response: 0.28, dampingFraction: 0.88)) {
                            store.moveWorkspaceToUngrouped(id)
                        }
                        return true
                    } isTargeted: { targeted in
                        withAnimation(.easeOut(duration: 0.14)) {
                            isUngroupedDropTargeted = targeted
                        }
                    }

                    if isCreatingFolder {
                        newFolderField
                    }

                    ForEach(store.workspaceFolders) { folder in
                        SidebarWorkspaceFolder(
                            folder: folder,
                            store: store,
                            createFolder: { beginCreatingFolder(containing: $0) }
                        )
                    }
                }
                .padding(.horizontal, 5)
                .padding(.vertical, 6)
                .animation(
                    .spring(response: 0.3, dampingFraction: 0.88),
                    value: store.sidebarWorkspaces.map(\.id)
                )
            }
            .contentShape(Rectangle())
            .contextMenu {
                Button("New Folder with Selected Tab…") {
                    beginCreatingFolder(containing: store.selectedWorkspace?.id)
                }
                .disabled(store.selectedWorkspace == nil)
            }
        }
        .frame(maxWidth: .infinity)
        .background(chrome.panel)
    }

    private var sidebarHeader: some View {
        HStack(spacing: 8) {
            Text("Workspaces")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(chrome.secondaryForeground)

            Spacer(minLength: 0)

            Text("\(store.tabCount)")
                .font(.system(size: 10, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(0.7))

            Button(action: store.newWorkspace) {
                Image(systemName: "plus")
                    .font(.system(size: 10, weight: .semibold))
                    .frame(width: 24, height: 24)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(chrome.secondaryForeground)
            .background(chrome.workspaceHover, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
            .help("New workspace · ⌘N")
        }
        .padding(.horizontal, 10)
        .frame(height: VibraLayout.panelHeaderHeight)
        .background(chrome.panelHeader)
    }

    private var newFolderField: some View {
        HStack(spacing: 7) {
            Image(systemName: "folder")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
            TextField("Folder name", text: $folderName)
                .textFieldStyle(.plain)
                .font(.system(size: 12, weight: .medium))
                .focused($isFolderNameFocused)
                .onSubmit(commitFolder)
                .onExitCommand(perform: cancelFolder)
            Button(action: cancelFolder) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 9)
        .frame(height: 32)
        .background(chrome.workspaceSelection, in: RoundedRectangle(cornerRadius: 6))
    }

    private func beginCreatingFolder(containing workspaceID: UUID?) {
        guard let workspaceID else { return }
        folderName = ""
        folderTargetWorkspaceID = workspaceID
        isCreatingFolder = true
        DispatchQueue.main.async { isFolderNameFocused = true }
    }

    private func commitFolder() {
        store.createFolder(named: folderName, containing: folderTargetWorkspaceID)
        cancelFolder()
    }

    private func cancelFolder() {
        folderName = ""
        folderTargetWorkspaceID = nil
        isFolderNameFocused = false
        isCreatingFolder = false
    }
}

private struct SidebarWorkspaceFolder: View {
    let folder: TerminalWorkspaceFolder
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome
    let createFolder: (UUID) -> Void
    @State private var isExpanded = true
    @State private var isRenaming = false
    @State private var name = ""
    @State private var isDropTargeted = false

    var body: some View {
        VStack(spacing: 2) {
            folderHeader
            if isExpanded {
                ForEach(folder.project.workspaces) { workspace in
                    SidebarWorkspaceRow(
                        located: LocatedTerminalWorkspace(
                            projectID: folder.id,
                            workspace: workspace
                        ),
                        store: store,
                        createFolder: { createFolder(workspace.id) }
                    )
                    .padding(.leading, 8)
                }
            }
        }
        .background {
            if isDropTargeted {
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .fill(chrome.workspaceSelection)
                    .overlay {
                        RoundedRectangle(cornerRadius: 7, style: .continuous)
                            .stroke(chrome.strongBorder, lineWidth: 1)
                    }
            }
        }
        .dropDestination(for: String.self) { items, _ in
            guard let value = items.first, let id = UUID(uuidString: value) else { return false }
            withAnimation(.spring(response: 0.28, dampingFraction: 0.86)) {
                store.moveWorkspace(id, to: folder.id)
                isExpanded = true
            }
            return true
        } isTargeted: { targeted in
            withAnimation(.easeOut(duration: 0.14)) {
                isDropTargeted = targeted
                if targeted { isExpanded = true }
            }
        }
    }

    private var folderHeader: some View {
        HStack(spacing: 5) {
            Button { isExpanded.toggle() } label: {
                Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 8, weight: .bold))
                    .frame(width: 12)
            }
            .buttonStyle(.plain)

            Image(systemName: isExpanded ? "folder.fill" : "folder")
                .font(.system(size: 9, weight: .medium))
                .foregroundStyle(chrome.secondaryForeground)

            if isRenaming {
                TextField("Folder name", text: $name)
                    .textFieldStyle(.plain)
                    .font(.system(size: 11, weight: .semibold))
                    .onSubmit(commitRename)
            } else {
                Text(folder.project.name)
                    .font(.system(size: 10.5, weight: .semibold))
                    .lineLimit(1)
            }

            Spacer(minLength: 4)
            Text("\(folder.project.workspaces.count)")
                .font(.system(size: 9, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(0.72))
        }
        .foregroundStyle(chrome.secondaryForeground)
        .padding(.horizontal, 8)
        .frame(height: 27)
        .contentShape(Rectangle())
        .onTapGesture { if !isRenaming { isExpanded.toggle() } }
        .contextMenu {
            if let selectedWorkspaceID = store.selectedWorkspace?.id {
                Button("New Folder with Selected Tab…") {
                    createFolder(selectedWorkspaceID)
                }
                Divider()
            }
            Button("Rename") {
                name = folder.project.name
                isRenaming = true
            }
            Button("Remove folder") {
                store.deleteFolder(folder.id)
            }
        }
    }

    private func commitRename() {
        store.renameFolder(folder.id, to: name)
        isRenaming = false
    }
}

private struct SidebarWorkspaceRow: View {
    let located: LocatedTerminalWorkspace
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome
    let createFolder: () -> Void
    @ObservedObject private var session: TerminalSession
    @State private var hovering = false
    @State private var branchName: String?
    @State private var isRenaming = false
    @State private var renameDraft = ""
    @FocusState private var isRenameFocused: Bool

    init(
        located: LocatedTerminalWorkspace,
        store: WorkspaceStore,
        createFolder: @escaping () -> Void
    ) {
        self.located = located
        self.store = store
        self.createFolder = createFolder
        let session = located.workspace.selectedSession
            ?? located.workspace.tabs[0].sessions[0]
        _session = ObservedObject(wrappedValue: session)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 7) {
                SidebarActivityGlyph(activity: session.agentActivity, selected: selected)
                    .frame(width: 14, height: 14)

                if isRenaming {
                    TextField("Workspace name", text: $renameDraft)
                        .textFieldStyle(.plain)
                        .font(.system(size: 12.25, weight: .semibold))
                        .focused($isRenameFocused)
                        .onSubmit(commitRename)
                        .onExitCommand(perform: cancelRename)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    Text(title)
                        .font(.system(size: 12.25, weight: .semibold))
                        .foregroundStyle(chrome.foreground.opacity(selected ? 1 : 0.92))
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }

                Spacer(minLength: 0)
                Button {
                    store.closeWorkspace(located.id, in: located.projectID)
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 8, weight: .bold))
                        .frame(width: 18, height: 18)
                }
                .buttonStyle(.plain)
                .foregroundStyle(chrome.secondaryForeground)
                .opacity(hovering && !isRenaming ? 0.78 : 0)
            }

            if let activitySummary {
                Text(activitySummary)
                    .font(.system(size: 10.25, weight: .medium))
                    .foregroundStyle(chrome.secondaryForeground.opacity(selected ? 0.95 : 0.82))
                    .lineLimit(1)
            }

            Text(metadataLine)
                .font(.system(size: 9.5, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(selected ? 0.92 : 0.72))
                .lineLimit(1)
        }
        .padding(.leading, 10)
        .padding(.trailing, 7)
        .padding(.vertical, activitySummary == nil ? 6 : 7)
        .frame(minHeight: activitySummary == nil ? 45 : 59)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(
                    selected
                        ? chrome.workspaceSelection
                        : hovering ? chrome.workspaceHover : .clear
                )
                .overlay(alignment: .leading) {
                    Capsule(style: .continuous)
                        .fill(chrome.accent)
                        .frame(width: 2)
                        .padding(.vertical, 8)
                        .opacity(selected ? 0.64 : 0)
                }
        }
        .contentShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .onTapGesture {
            if !isRenaming {
                store.selectWorkspace(located.id, in: located.projectID)
            }
        }
        .simultaneousGesture(
            TapGesture(count: 2).onEnded {
                beginRename()
            }
        )
        .onHover { hovering = $0 }
        .task(id: session.workingDirectory) {
            let directory = session.workingDirectory
            branchName = await Task.detached {
                GitClient.branch(from: directory)
            }.value
        }
        .draggable(located.id.uuidString) {
            SidebarWorkspaceDragPreview(
                title: title,
                detail: metadataLine,
                activity: session.agentActivity
            )
        }
        .contextMenu {
            Button("Rename workspace", action: beginRename)
            Button("New Folder with This Tab…", action: createFolder)
            openInEditorContextMenuItems(path: session.workingDirectory)
            Divider()
            if !currentFolderName.isEmpty {
                Button("Remove from folder") {
                    store.moveWorkspaceToUngrouped(located.id)
                }
            }
            if !store.workspaceFolders.isEmpty {
                Menu("Move to folder") {
                    ForEach(store.workspaceFolders) { folder in
                        Button(folder.project.name) {
                            store.moveWorkspace(located.id, to: folder.id)
                        }
                    }
                }
            }
            Divider()
            Button("Close workspace", role: .destructive) {
                store.closeWorkspace(located.id, in: located.projectID)
            }
        }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.spring(response: 0.24, dampingFraction: 0.9), value: selected)
    }

    private var selected: Bool { store.selectedWorkspace?.id == located.id }

    private var currentFolderName: String {
        store.workspaceFolders.first(where: { $0.id == located.projectID })?.project.name ?? ""
    }

    private var title: String {
        let customName = located.workspace.name.trimmingCharacters(in: .whitespacesAndNewlines)
        if !customName.isEmpty { return customName }
        let directoryName = URL(fileURLWithPath: session.workingDirectory).lastPathComponent
        return directoryName.isEmpty ? "Terminal" : directoryName
    }

    private var abbreviatedPath: String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let path = session.workingDirectory
        if path == home { return "~" }
        if path.hasPrefix(home + "/") { return "~" + path.dropFirst(home.count) }
        return path
    }

    private var terminalTabCountLabel: String {
        let count = located.workspace.tabs.count
        return count > 1 ? "\(count) surfaces" : ""
    }

    private var metadataLine: String {
        [branchName, abbreviatedPath, terminalTabCountLabel]
            .compactMap { value -> String? in
                guard let value, !value.isEmpty else { return nil }
                return value
            }
            .joined(separator: "  ·  ")
    }

    private var activitySummary: String? {
        switch session.agentActivity {
        case .idle:
            let sessionTitle = session.title.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !sessionTitle.isEmpty, sessionTitle != "Terminal", sessionTitle != title else {
                return nil
            }
            return sessionTitle
        case .ready:
            return "Agent ready"
        case .running:
            return "Agent working"
        case .needsAttention:
            return "Waiting for your input"
        case .finished(_, let succeeded, _):
            return succeeded == false ? "Command failed" : "Agent finished"
        }
    }

    private func beginRename() {
        store.selectWorkspace(located.id, in: located.projectID)
        renameDraft = title
        isRenaming = true
        DispatchQueue.main.async { isRenameFocused = true }
    }

    private func commitRename() {
        store.renameWorkspace(located.id, in: located.projectID, to: renameDraft)
        cancelRename()
    }

    private func cancelRename() {
        renameDraft = ""
        isRenameFocused = false
        isRenaming = false
    }
}

private struct SidebarWorkspaceDragPreview: View {
    let title: String
    let detail: String
    let activity: AgentActivity

    var body: some View {
        HStack(spacing: 8) {
            SidebarActivityGlyph(activity: activity, selected: false)
                .frame(width: 14, height: 14)
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text(detail)
                    .font(.system(size: 9, weight: .medium, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 11)
        .frame(width: 220, height: 48)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(Color.white.opacity(0.12), lineWidth: 0.5)
        }
        .shadow(color: Color.black.opacity(0.24), radius: 12, y: 5)
    }
}

private struct SidebarActivityGlyph: View {
    let activity: AgentActivity
    let selected: Bool
    @Environment(\.appChrome) private var chrome

    var body: some View {
        switch activity {
        case .idle:
            Circle()
                .fill(chrome.secondaryForeground.opacity(selected ? 0.86 : 0.68))
                .frame(width: 5, height: 5)
        case .ready:
            Circle()
                .fill(chrome.foreground.opacity(selected ? 0.8 : 0.6))
                .frame(width: 6, height: 6)
        case .running:
            Circle()
                .fill(chrome.accent)
                .frame(width: 6, height: 6)
        case .needsAttention:
            Circle()
                .fill(Color.orange)
                .frame(width: 13, height: 13)
                .overlay {
                    Text("1")
                        .font(.system(size: 8, weight: .bold, design: .rounded))
                        .foregroundStyle(chrome.recessed)
                }
        case .finished(_, let succeeded, _):
            Image(systemName: succeeded == false ? "xmark.circle.fill" : "checkmark.circle.fill")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(succeeded == false ? .red : .green)
        }
    }
}

private struct WorkspaceDetail: View {
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome

    var body: some View {
        VStack(spacing: 0) {
            terminalHeader

            Rectangle()
                .fill(chrome.quietBorder)
                .frame(height: 1)

            terminalCanvas
        }
        .background(chrome.panel)
    }

    private var terminalHeader: some View {
        HStack(spacing: 8) {
            terminalTabs
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .frame(height: VibraLayout.panelHeaderHeight)
        .background(chrome.panelHeader)
    }

    @ViewBuilder
    private var terminalTabs: some View {
        if let project = store.selectedProject,
           let workspace = project.selectedWorkspace {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 3) {
                    ForEach(Array(workspace.tabs.enumerated()), id: \.element.id) {
                        index, tab in
                        horizontalTab(tab, index: index, workspace: workspace, project: project)
                    }

                    WindowChromeButton(
                        systemImage: "plus",
                        help: "New terminal tab · ⌘T",
                        action: store.newSession
                    )
                }
                .padding(.vertical, 5)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .layoutPriority(1)
        }
    }

    @ViewBuilder
    private func horizontalTab(
        _ tab: TerminalTab,
        index: Int,
        workspace: TerminalWorkspace,
        project: VibraProject
    ) -> some View {
        if let session = tab.selectedSession ?? tab.sessions.first {
            HeaderHorizontalTab(
                session: session,
                fallbackTitle: workspace.tabs.count == 1 ? "Terminal" : "Terminal \(index + 1)",
                paneCount: tab.sessions.count,
                selected: workspace.selectedTabID == tab.id,
                select: { store.selectTab(tab.id, in: project.id) },
                close: { store.closeTab(tab.id, in: project.id) }
            )
        }
    }

    @ViewBuilder
    private var terminalCanvas: some View {
        if store.projects.isEmpty {
            EmptyWorkspaceView(
                title: "No tabs",
                detail: "Create a tab or open a folder to begin.",
                buttonTitle: "New Tab",
                action: store.newWorkspace
            )
        } else if store.selectedSession == nil {
            EmptyWorkspaceView(
                title: "No terminal sessions",
                detail: "Start a terminal in the selected tab.",
                buttonTitle: "New Terminal",
                action: store.newSession
            )
        } else {
            TerminalCanvas(store: store)
                .padding((store.selectedTab?.sessions.count ?? 1) > 1 ? 3 : 0)
        }
    }
}

private struct TerminalCanvas: View {
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome

    var body: some View {
        ZStack {
            hiddenSessions
            if let project = store.selectedProject {
                splitLayout(project)
            }
        }
        .background(chrome.recessed)
    }

    private var hiddenSessions: some View {
        let visible = Set(store.selectedTab?.layout.terminalIDs ?? [])
        return ForEach(store.projects) { project in
            ForEach(project.allSessions.filter {
                project.id != store.selectedProjectID || !visible.contains($0.id)
            }) { session in
                TerminalSurfaceHost(
                    session: session,
                    isVisible: false,
                    isFocused: false
                )
                .allowsHitTesting(false)
                .accessibilityHidden(true)
            }
        }
    }

    @ViewBuilder
    private func splitLayout(_ project: VibraProject) -> some View {
        if let tab = project.selectedTab {
            PaneTreeView(
                layout: tab.layout,
                sessions: tab.sessions,
                selectedSessionID: tab.selectedSessionID,
                usesPaneChrome: tab.sessions.count > 1,
                focus: { store.focusSession($0, in: project.id) }
            )
        }
    }
}

private struct PaneTreeView: View {
    let layout: PaneLayoutSnapshot
    let sessions: [TerminalSession]
    let selectedSessionID: UUID?
    let usesPaneChrome: Bool
    let focus: (UUID) -> Void
    @Environment(\.appChrome) private var chrome

    var body: some View {
        content(layout)
    }

    private func content(_ node: PaneLayoutSnapshot) -> AnyView {
        switch node {
        case .terminal(let id):
            guard let session = sessions.first(where: { $0.id == id }) else {
                return AnyView(Color.clear)
            }
            return AnyView(
                TerminalPane(
                    session: session,
                    focused: session.id == selectedSessionID,
                    framed: usesPaneChrome,
                    focus: { focus(session.id) }
                )
                .id(id)
            )
        case .split(let axis, let first, let second):
            if axis == .horizontal {
                return AnyView(
                    HStack(spacing: 1) {
                        PaneTreeView(
                            layout: first,
                            sessions: sessions,
                            selectedSessionID: selectedSessionID,
                            usesPaneChrome: usesPaneChrome,
                            focus: focus
                        )
                        PaneTreeView(
                            layout: second,
                            sessions: sessions,
                            selectedSessionID: selectedSessionID,
                            usesPaneChrome: usesPaneChrome,
                            focus: focus
                        )
                    }
                    .background(chrome.quietBorder)
                )
            }
            return AnyView(
                VStack(spacing: 1) {
                    PaneTreeView(
                        layout: first,
                        sessions: sessions,
                        selectedSessionID: selectedSessionID,
                        usesPaneChrome: usesPaneChrome,
                        focus: focus
                    )
                    PaneTreeView(
                        layout: second,
                        sessions: sessions,
                        selectedSessionID: selectedSessionID,
                        usesPaneChrome: usesPaneChrome,
                        focus: focus
                    )
                }
                .background(chrome.quietBorder)
            )
        }
    }
}

private struct TerminalPane: View {
    let session: TerminalSession
    let focused: Bool
    let framed: Bool
    let focus: () -> Void
    @ObservedObject private var state: TerminalViewState
    @Environment(\.appChrome) private var chrome

    init(
        session: TerminalSession,
        focused: Bool,
        framed: Bool,
        focus: @escaping () -> Void
    ) {
        self.session = session
        self.focused = focused
        self.framed = framed
        self.focus = focus
        _state = ObservedObject(wrappedValue: session.state)
    }

    var body: some View {
        Group {
            if framed {
                terminalSurface
                    .clipShape(RoundedRectangle(cornerRadius: 5, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 5, style: .continuous)
                            .stroke(
                                focused ? chrome.accent.opacity(0.62) : chrome.quietBorder,
                                lineWidth: focused ? 1.25 : 1
                            )
                    }
            } else {
                terminalSurface
            }
        }
    }

    private var terminalSurface: some View {
        TerminalSurfaceHost(session: session, isVisible: true, isFocused: focused)
            .onChange(of: state.isFocused) { _, isFocused in
                if isFocused && !focused { focus() }
            }
    }
}

private struct SessionHeader: View {
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome
    let sidebarWidth: CGFloat?

    var body: some View {
        HStack(spacing: 0) {
            windowControls
                .frame(
                    width: sidebarWidth ?? VibraLayout.collapsedChromeControlsWidth,
                    alignment: .leading
                )
                .overlay(alignment: .trailing) {
                    Rectangle()
                        .fill(chrome.quietBorder)
                        .frame(width: 1)
                }

            Spacer(minLength: 0)

            HStack(spacing: 8) {
                if let project = store.selectedProject {
                    OpenInEditorButton(path: project.rootPath, compact: true)
                }

                WindowChromeButton(
                    systemImage: "sidebar.right",
                    help: "Toggle Git Sidebar · ⌘R",
                    disabled: store.selectedProject == nil,
                    action: store.toggleGitSidebar
                )
            }
            .padding(.trailing, 10)
        }
        .frame(height: VibraLayout.windowChromeHeight)
        .background(chrome.panelHeader)
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(chrome.quietBorder)
                .frame(height: 1)
        }
    }

    private var windowControls: some View {
        HStack(spacing: 2) {
            WindowChromeButton(
                systemImage: store.isTerminalSidebarVisible ? "sidebar.left" : "sidebar.right",
                help: "Toggle Terminal Sidebar · ⌘B",
                action: store.toggleTerminalSidebar
            )

            WindowChromeButton(
                systemImage: "chevron.left",
                help: "Previous workspace · ⌃⌘[",
                disabled: store.tabCount < 2,
                action: { store.selectAdjacentWorkspace(-1) }
            )

            WindowChromeButton(
                systemImage: "chevron.right",
                help: "Next workspace · ⌃⌘]",
                disabled: store.tabCount < 2,
                action: { store.selectAdjacentWorkspace(1) }
            )

            Spacer(minLength: 0)
        }
        .padding(.leading, 76)
        .padding(.trailing, 8)
    }
}

private struct WindowChromeButton: View {
    let systemImage: String
    let help: String
    var disabled = false
    let action: () -> Void
    @Environment(\.appChrome) private var chrome
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 10, weight: .semibold))
                .frame(width: 24, height: 24)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(chrome.secondaryForeground.opacity(disabled ? 0.35 : 0.9))
        .background(
            hovering && !disabled ? chrome.workspaceHover : .clear,
            in: RoundedRectangle(cornerRadius: 5, style: .continuous)
        )
        .disabled(disabled)
        .help(help)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }
}

private struct HeaderHorizontalTab: View {
    @ObservedObject var session: TerminalSession
    @ObservedObject private var state: TerminalViewState
    @Environment(\.appChrome) private var chrome
    let fallbackTitle: String
    let paneCount: Int
    let selected: Bool
    let select: () -> Void
    let close: () -> Void
    @State private var hovering = false

    init(
        session: TerminalSession,
        fallbackTitle: String,
        paneCount: Int,
        selected: Bool,
        select: @escaping () -> Void,
        close: @escaping () -> Void
    ) {
        self.session = session
        _state = ObservedObject(wrappedValue: session.state)
        self.fallbackTitle = fallbackTitle
        self.paneCount = paneCount
        self.selected = selected
        self.select = select
        self.close = close
    }

    var body: some View {
        HStack(spacing: 6) {
            HeaderTabActivityGlyph(activity: session.agentActivity, selected: selected)
            Text(title)
                .font(.system(size: 11, weight: selected ? .semibold : .regular))
                .foregroundStyle(chrome.foreground.opacity(selected ? 1 : 0.78))
                .lineLimit(1)
                .frame(maxWidth: 145)
            if paneCount > 1 {
                Text("\(paneCount)")
                    .font(.system(size: 8.5, weight: .semibold, design: .rounded))
                    .foregroundStyle(.secondary)
            }
            Button(action: close) {
                Image(systemName: "xmark")
                    .font(.system(size: 7.5, weight: .bold))
                    .frame(width: 17, height: 20)
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .opacity(hovering ? 0.72 : 0)
        }
        .padding(.leading, 9)
        .padding(.trailing, 3)
        .frame(minWidth: 88)
        .frame(height: 28)
        .background {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(selected ? chrome.workspaceSelection : hovering ? chrome.workspaceHover : .clear)
                .overlay {
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .stroke(selected ? chrome.quietBorder : .clear, lineWidth: 1)
                }
        }
        .contentShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .onTapGesture(perform: select)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.easeOut(duration: 0.12), value: selected)
    }

    private var title: String {
        let reported = state.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return reported.isEmpty || reported == "Terminal" ? fallbackTitle : reported
    }
}

private struct HeaderTabActivityGlyph: View {
    let activity: AgentActivity
    let selected: Bool
    @Environment(\.appChrome) private var chrome

    var body: some View {
        switch activity {
        case .idle:
            Image(systemName: "terminal")
                .font(.system(size: 8.5, weight: .medium))
                .foregroundStyle(.secondary)
        case .ready:
            Circle()
                .fill(Color.secondary.opacity(0.7))
                .frame(width: 6, height: 6)
        case .running:
            ProgressView()
                .controlSize(.mini)
                .scaleEffect(0.56)
                .frame(width: 9, height: 9)
                .tint(selected ? chrome.accent : .secondary)
        case .needsAttention:
            Circle()
                .fill(Color.orange)
                .frame(width: 7, height: 7)
        case .finished(_, let succeeded, _):
            Image(systemName: succeeded == false ? "xmark.circle.fill" : "checkmark.circle.fill")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(succeeded == false ? Color.red : .green)
        }
    }
}

private struct EmptyWorkspaceView: View {
    let title: String
    let detail: String
    let buttonTitle: String
    let action: () -> Void
    @Environment(\.appChrome) private var chrome

    var body: some View {
        VStack(spacing: 14) {
            VStack(spacing: 4) {
                Text(title)
                    .font(.system(size: 15, weight: .semibold))
                Text(detail)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
            Button(buttonTitle, action: action)
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                .tint(chrome.accent)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct VibraContentPanelModifier: ViewModifier {
    @Environment(\.appChrome) private var chrome

    func body(content: Content) -> some View {
        content
            .background(chrome.panel)
            .clipShape(
                RoundedRectangle(
                    cornerRadius: VibraLayout.contentCornerRadius,
                    style: .continuous
                )
            )
            .overlay {
                RoundedRectangle(
                    cornerRadius: VibraLayout.contentCornerRadius,
                    style: .continuous
                )
                .strokeBorder(chrome.quietBorder, lineWidth: 1)
            }
    }
}

private extension View {
    func vibraContentPanel() -> some View {
        modifier(VibraContentPanelModifier())
    }
}

enum VibraLayout {
    static let panelHeaderHeight: CGFloat = 40
    static let windowChromeHeight: CGFloat = 32
    static let collapsedChromeControlsWidth: CGFloat = 158
    static let contentCornerRadius: CGFloat = 10
    static let contentInset: CGFloat = 8
    static let panelGap: CGFloat = 8
}
