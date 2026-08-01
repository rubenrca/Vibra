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
    @GestureState private var sidebarDragTranslation: CGFloat = 0
    @GestureState private var gitSidebarDragTranslation: CGFloat = 0
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
                        .frame(width: effectiveTerminalSidebarWidth)
                        .vibraContentPanel()
                        .padding(.vertical, VibraLayout.contentInset)
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
            .padding(.leading, VibraLayout.contentInset)
            .padding(.trailing, VibraLayout.contentInset)
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            SessionHeader(
                store: store,
                sidebarWidth: store.isTerminalSidebarVisible
                    ? effectiveTerminalSidebarWidth + VibraLayout.contentInset
                    : nil
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
        clampedGitSidebarWidth(gitSidebarWidth - gitSidebarDragTranslation)
            + (gitModel.expandedChangeID == nil ? 0 : 140)
    }

    private var effectiveTerminalSidebarWidth: CGFloat {
        clampedTerminalSidebarWidth(terminalSidebarWidth + sidebarDragTranslation)
    }

    private var sidebarDivider: some View {
        Color.clear
            .frame(width: 10)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .global)
                    .updating($sidebarDragTranslation) { value, translation, transaction in
                        transaction.animation = nil
                        translation = value.translation.width
                    }
                    .onEnded { value in
                        let width = clampedTerminalSidebarWidth(
                            terminalSidebarWidth + value.translation.width
                        )
                        terminalSidebarWidth = width
                        UserDefaults.standard.set(
                            Double(width),
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
                DragGesture(minimumDistance: 0, coordinateSpace: .global)
                    .updating($gitSidebarDragTranslation) { value, translation, transaction in
                        transaction.animation = nil
                        translation = value.translation.width
                    }
                    .onEnded { value in
                        let width = clampedGitSidebarWidth(
                            gitSidebarWidth - value.translation.width
                        )
                        gitSidebarWidth = width
                        UserDefaults.standard.set(
                            Double(width),
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

    private func clampedTerminalSidebarWidth(_ width: CGFloat) -> CGFloat {
        min(max(width, 208), 380)
    }

    private func clampedGitSidebarWidth(_ width: CGFloat) -> CGFloat {
        min(max(width, 300), 680)
    }
}

private struct TerminalSidebar: View {
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome
    @State private var isCreatingSpace = false
    @State private var spaceName = ""
    @State private var spaceTargetWorkspaceID: UUID?
    @FocusState private var isSpaceNameFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            sidebarHeader
            Divider().overlay(chrome.quietBorder)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    SidebarLocalSpaceRow(store: store)
                    ForEach(store.workspaceFolders) { folder in
                        SidebarSpaceRow(
                            folder: folder,
                            store: store,
                            createSpace: { beginCreatingSpace(containing: $0) }
                        )
                    }

                    if isCreatingSpace {
                        newSpaceField
                            .padding(.top, 3)
                    }

                    Divider()
                        .overlay(chrome.quietBorder.opacity(0.78))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 10)

                    agentSectionHeader

                    ForEach(spaceWorkspaces) { located in
                        SidebarAgentRow(
                            located: located,
                            store: store,
                            createSpace: { beginCreatingSpace(containing: located.id) }
                        )
                    }
                }
                .padding(.horizontal, 5)
                .padding(.vertical, 6)
                .animation(
                    .spring(response: 0.3, dampingFraction: 0.88),
                    value: spaceWorkspaces.map(\.id)
                )
            }
            .contentShape(Rectangle())
            .contextMenu {
                Button("New Space with Selected Agent…") {
                    beginCreatingSpace(containing: store.selectedWorkspace?.id)
                }
                .disabled(store.selectedWorkspace == nil)
            }
        }
        .frame(maxWidth: .infinity)
        .background(chrome.panel)
    }

    private var sidebarHeader: some View {
        HStack(spacing: 8) {
            Text("Spaces")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(chrome.secondaryForeground)

            Spacer(minLength: 0)

            Text("\(store.workspaceFolders.count + 1)")
                .font(.system(size: 10, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(0.7))

            Button {
                beginCreatingSpace(containing: store.selectedWorkspace?.id)
            } label: {
                Image(systemName: "plus")
                    .font(.system(size: 10, weight: .semibold))
                    .frame(width: 24, height: 24)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(chrome.secondaryForeground)
            .background(chrome.workspaceHover, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
            .disabled(store.selectedWorkspace == nil)
            .help("New space from selected agent")
        }
        .padding(.horizontal, 10)
        .frame(height: VibraLayout.panelHeaderHeight)
        .background(chrome.panelHeader)
    }

    private var agentSectionHeader: some View {
        HStack(spacing: 7) {
            Text("Agents")
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundStyle(chrome.secondaryForeground)
                .textCase(.uppercase)
                .tracking(0.7)

            Text("\(spaceWorkspaces.count)")
                .font(.system(size: 9.5, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(0.65))

            Spacer(minLength: 0)

            Button {
                store.newWorkspace(in: store.selectedProject?.id)
            } label: {
                Image(systemName: "plus")
                    .font(.system(size: 9, weight: .bold))
                    .frame(width: 20, height: 20)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(chrome.secondaryForeground.opacity(0.84))
            .background(chrome.workspaceHover, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
            .help("New agent · ⌘N")
        }
        .padding(.horizontal, 7)
        .padding(.bottom, 5)
    }

    /// Agents are the tabs that belong to the selected space, rather than a
    /// global list. Selecting a space therefore changes the working set in the
    /// lower half of the sidebar without losing its active terminal selection.
    private var spaceWorkspaces: [LocatedTerminalWorkspace] {
        guard let project = store.selectedProject else { return [] }
        return project.workspaces.map {
            LocatedTerminalWorkspace(projectID: project.id, workspace: $0)
        }
    }

    private var newSpaceField: some View {
        HStack(spacing: 7) {
            Image(systemName: "square.stack.3d.up")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(chrome.secondaryForeground)
            TextField("Space name", text: $spaceName)
                .textFieldStyle(.plain)
                .font(.system(size: 12, weight: .medium))
                .focused($isSpaceNameFocused)
                .onSubmit(commitSpace)
                .onExitCommand(perform: cancelSpace)
            Button(action: cancelSpace) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 9)
        .frame(height: 32)
        .background(chrome.workspaceSelection, in: RoundedRectangle(cornerRadius: 6))
    }

    private func beginCreatingSpace(containing workspaceID: UUID?) {
        guard let workspaceID else { return }
        spaceName = ""
        spaceTargetWorkspaceID = workspaceID
        isCreatingSpace = true
        DispatchQueue.main.async { isSpaceNameFocused = true }
    }

    private func commitSpace() {
        store.createFolder(named: spaceName, containing: spaceTargetWorkspaceID)
        cancelSpace()
    }

    private func cancelSpace() {
        spaceName = ""
        spaceTargetWorkspaceID = nil
        isSpaceNameFocused = false
        isCreatingSpace = false
    }
}

private struct SidebarLocalSpaceRow: View {
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome
    @State private var hovering = false
    @State private var isDropTargeted = false

    private var project: VibraProject? {
        store.projects.first { $0.name.isEmpty }
    }

    private var isSelected: Bool {
        project?.id == store.selectedProject?.id
    }

    var body: some View {
        SidebarSpaceLabel(
            title: "Local checkout",
            detail: localDetail,
            count: store.ungroupedWorkspaces.count,
            accent: chrome.accent,
            selected: isSelected,
            hovering: hovering,
            systemImage: "laptopcomputer"
        )
        .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        .onTapGesture {
            guard let project else { return }
            if let workspace = project.selectedWorkspace ?? project.workspaces.first {
                store.selectWorkspace(workspace.id, in: project.id)
            } else {
                store.selectProject(project.id)
            }
        }
        .onHover { hovering = $0 }
        .dropDestination(for: String.self) { items, _ in
            guard let value = items.first, let id = UUID(uuidString: value) else { return false }
            withAnimation(.spring(response: 0.28, dampingFraction: 0.88)) {
                store.moveWorkspaceToUngrouped(id)
            }
            return true
        } isTargeted: { targeted in
            withAnimation(.easeOut(duration: 0.14)) { isDropTargeted = targeted }
        }
        .overlay {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .stroke(chrome.accent.opacity(isDropTargeted ? 0.78 : 0), lineWidth: 1)
                .padding(.horizontal, 1)
        }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }

    private var localDetail: String {
        guard let project else { return "No active agents" }
        let root = URL(fileURLWithPath: project.rootPath).lastPathComponent
        return root.isEmpty ? "This Mac" : "@ \(root)"
    }
}

private struct SidebarSpaceRow: View {
    let folder: TerminalWorkspaceFolder
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome
    let createSpace: (UUID) -> Void
    @State private var isRenaming = false
    @State private var name = ""
    @State private var hovering = false
    @State private var isDropTargeted = false

    var body: some View {
        Group {
            if isRenaming {
                renameField
            } else {
                SidebarSpaceLabel(
                    title: folder.project.name,
                    detail: "@ \(rootName)",
                    count: folder.project.workspaces.count,
                    accent: spaceAccent,
                    selected: folder.id == store.selectedProject?.id,
                    hovering: hovering,
                    systemImage: "folder"
                )
            }
        }
        .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        .onTapGesture {
            guard !isRenaming else { return }
            if let workspace = folder.project.selectedWorkspace ?? folder.project.workspaces.first {
                store.selectWorkspace(workspace.id, in: folder.id)
            } else {
                store.selectProject(folder.id)
            }
        }
        .onHover { hovering = $0 }
        .dropDestination(for: String.self) { items, _ in
            guard let value = items.first, let id = UUID(uuidString: value) else { return false }
            withAnimation(.spring(response: 0.28, dampingFraction: 0.86)) {
                store.moveWorkspace(id, to: folder.id)
            }
            return true
        } isTargeted: { targeted in
            withAnimation(.easeOut(duration: 0.14)) {
                isDropTargeted = targeted
            }
        }
        .overlay {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .stroke(spaceAccent.opacity(isDropTargeted ? 0.85 : 0), lineWidth: 1)
                .padding(.horizontal, 1)
        }
        .contextMenu {
            if let selectedWorkspaceID = store.selectedWorkspace?.id {
                Button("New Space with Selected Agent…") {
                    createSpace(selectedWorkspaceID)
                }
                Divider()
            }
            Button("Rename Space") {
                name = folder.project.name
                isRenaming = true
            }
            Button("Remove Space") {
                store.deleteFolder(folder.id)
            }
        }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }

    private var renameField: some View {
        HStack(spacing: 7) {
            Image(systemName: "folder")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(spaceAccent)
                .frame(width: 18)
            TextField("Space name", text: $name)
                .textFieldStyle(.plain)
                .font(.system(size: 11.5, weight: .semibold))
                .onSubmit(commitRename)
                .onExitCommand { isRenaming = false }
            Button { isRenaming = false } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .frame(width: 16, height: 18)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 8)
        .frame(height: 32)
        .background(chrome.workspaceSelection, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
    }

    private func commitRename() {
        store.renameFolder(folder.id, to: name)
        isRenaming = false
    }

    private var rootName: String {
        let name = URL(fileURLWithPath: folder.project.rootPath).lastPathComponent
        return name.isEmpty ? "local" : name
    }

    private var spaceAccent: Color {
        SidebarSpacePalette.color(for: folder.id)
    }
}

private struct SidebarSpaceLabel: View {
    let title: String
    let detail: String
    let count: Int
    let accent: Color
    let selected: Bool
    let hovering: Bool
    let systemImage: String
    @Environment(\.appChrome) private var chrome

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(accent)
                .frame(width: 6, height: 6)
                .shadow(color: accent.opacity(0.35), radius: 2)

            Image(systemName: systemImage)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(chrome.secondaryForeground.opacity(0.95))
                .frame(width: 15)

            Text(title)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(chrome.foreground.opacity(selected ? 1 : 0.9))
                .lineLimit(1)

            Spacer(minLength: 0)

            Text(detail)
                .font(.system(size: 9.5, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(selected ? 0.9 : 0.68))
                .lineLimit(1)

            Text("\(count)")
                .font(.system(size: 9, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(0.56))
        }
        .padding(.horizontal, 8)
        .frame(height: 30)
        .background {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .fill(selected ? chrome.workspaceSelection : hovering ? chrome.workspaceHover : .clear)
                .overlay(alignment: .leading) {
                    Capsule(style: .continuous)
                        .fill(accent)
                        .frame(width: 2)
                        .padding(.vertical, 7)
                        .opacity(selected ? 0.86 : 0)
                }
        }
    }
}

private enum SidebarSpacePalette {
    static func color(for id: UUID) -> Color {
        let colors: [Color] = [.mint, .pink, .cyan, .orange, .indigo, .green]
        let value = id.uuidString.unicodeScalars.reduce(0) { $0 + Int($1.value) }
        return colors[value % colors.count]
    }
}

private struct SidebarAgentRow: View {
    let located: LocatedTerminalWorkspace
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome
    let createSpace: () -> Void
    @ObservedObject private var session: TerminalSession
    @State private var hovering = false
    @State private var branchName: String?
    @State private var isRenaming = false
    @State private var renameDraft = ""
    @FocusState private var isRenameFocused: Bool

    init(
        located: LocatedTerminalWorkspace,
        store: WorkspaceStore,
        createSpace: @escaping () -> Void
    ) {
        self.located = located
        self.store = store
        self.createSpace = createSpace
        let session = located.workspace.selectedSession
            ?? located.workspace.tabs[0].sessions[0]
        _session = ObservedObject(wrappedValue: session)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 7) {
                SidebarAgentBadge(activity: session.agentActivity, selected: selected)
                    .frame(width: 18, height: 18)

                if isRenaming {
                    TextField("Agent name", text: $renameDraft)
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
            Button("Rename agent", action: beginRename)
            if located.workspace.titleSource == .manual {
                Button("Use task title automatically") {
                    store.useAutomaticWorkspaceTitle(located.id, in: located.projectID)
                }
            }
            Button("New Space with This Agent…", action: createSpace)
            openInEditorContextMenuItems(path: session.workingDirectory)
            Divider()
            if !currentSpaceName.isEmpty {
                Button("Remove from space") {
                    store.moveWorkspaceToUngrouped(located.id)
                }
            }
            if !store.workspaceFolders.isEmpty {
                Menu("Move to space") {
                    ForEach(store.workspaceFolders) { folder in
                        Button(folder.project.name) {
                            store.moveWorkspace(located.id, to: folder.id)
                        }
                    }
                }
            }
            Divider()
            Button("Close agent", role: .destructive) {
                store.closeWorkspace(located.id, in: located.projectID)
            }
        }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.spring(response: 0.24, dampingFraction: 0.9), value: selected)
    }

    private var selected: Bool { store.selectedWorkspace?.id == located.id }

    private var currentSpaceName: String {
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
            guard !sessionTitle.isEmpty,
                  sessionTitle != "Terminal",
                  sessionTitle != title,
                  CodingAgent.detect(commandLine: "", title: sessionTitle) == nil
            else {
                return nil
            }
            return sessionTitle
        case .ready(let agent):
            return "\(agent.displayName) · Ready"
        case .running(let agent, _):
            return "\(agent.displayName) · Working"
        case .needsAttention(let agent, _):
            return "\(agent.displayName) · Needs input"
        case .finished(let agent, let succeeded, _):
            return succeeded == false
                ? "\(agent.displayName) · Command failed"
                : "\(agent.displayName) · Finished"
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
            SidebarAgentBadge(activity: activity, selected: false)
                .frame(width: 18, height: 18)
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

private struct SidebarAgentBadge: View {
    let activity: AgentActivity
    let selected: Bool
    @Environment(\.appChrome) private var chrome

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            if let brandImage {
                Group {
                    if usesTemplateBrandImage {
                        Image(nsImage: brandImage)
                            .resizable()
                            .renderingMode(.template)
                            .foregroundStyle(markColor.opacity(selected ? 1 : 0.88))
                    } else {
                        Image(nsImage: brandImage)
                            .resizable()
                    }
                }
                .scaledToFit()
                .padding(1)
                .frame(width: 17, height: 17)
            } else {
                Text(markGlyph)
                    .font(.system(size: glyphSize, weight: .bold, design: .rounded))
                    .foregroundStyle(markColor.opacity(selected ? 1 : 0.88))
                    .lineLimit(1)
                    .minimumScaleFactor(0.55)
                    .frame(width: 16, height: 16)
            }

            if let statusColor {
                Circle()
                    .fill(statusColor)
                    .frame(width: 6, height: 6)
                    .overlay {
                        Circle()
                            .stroke(chrome.panel, lineWidth: 1.4)
                    }
                    .offset(x: 1.5, y: 1.5)
            }
        }
        .frame(width: 18, height: 18)
        .accessibilityLabel(accessibilityLabel)
    }

    private var agent: CodingAgent? { activity.agent }

    private var brandImage: NSImage? {
        guard let agent,
              let url = Bundle.module.url(
                forResource: brandAssetName(for: agent),
                withExtension: "svg"
              )
        else { return nil }
        return NSImage(contentsOf: url)
    }

    private var usesTemplateBrandImage: Bool {
        agent != .goose
    }

    private func brandAssetName(for agent: CodingAgent) -> String {
        switch agent {
        case .codex: "codex"
        case .claude: "claude"
        case .gemini: "gemini"
        case .opencode: "opencode"
        case .aider: "aider"
        case .goose: "goose"
        case .amp: "amp"
        case .cursor: "cursor"
        }
    }

    private var markGlyph: String {
        switch agent {
        case .codex: "✺"
        case .claude: "AI"
        case .gemini: "✦"
        case .opencode: "<>"
        case .aider: "A"
        case .goose: "G"
        case .amp: "ϟ"
        case .cursor: "◒"
        case nil: ">_"
        }
    }

    private var glyphSize: CGFloat {
        switch agent {
        case .claude, .opencode, .none: 7.5
        default: 10
        }
    }

    private var markColor: Color {
        switch agent {
        case .codex: .orange
        case .claude: .red
        case .gemini: .blue
        case .opencode: .cyan
        case .aider: .green
        case .goose: .teal
        case .amp: .yellow
        case .cursor: .purple
        case nil: chrome.secondaryForeground
        }
    }

    private var statusColor: Color? {
        switch activity {
        case .idle: nil
        case .ready: chrome.foreground.opacity(0.68)
        case .running: chrome.accent
        case .needsAttention: .orange
        case .finished(_, let succeeded, _): succeeded == false ? .red : .green
        }
    }

    private var accessibilityLabel: String {
        let name = agent?.displayName ?? "Terminal"
        switch activity {
        case .idle: return name
        case .ready: return "\(name), ready"
        case .running: return "\(name), working"
        case .needsAttention: return "\(name), needs attention"
        case .finished(_, let succeeded, _):
            return succeeded == false ? "\(name), failed" : "\(name), finished"
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
        .padding(.horizontal, 12)
        .frame(height: VibraLayout.panelHeaderHeight)
        .background(chrome.panelHeader)
    }

    @ViewBuilder
    private var terminalTabs: some View {
        if let project = store.selectedProject,
           let workspace = project.selectedWorkspace {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
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
                .padding(.vertical, 4)
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
                preferredTitle: workspace.tabs.count == 1
                    ? workspace.name
                    : session.taskTitle,
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
                action: { store.newWorkspace() }
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
    let preferredTitle: String?
    let fallbackTitle: String
    let paneCount: Int
    let selected: Bool
    let select: () -> Void
    let close: () -> Void
    @State private var hovering = false

    init(
        session: TerminalSession,
        preferredTitle: String?,
        fallbackTitle: String,
        paneCount: Int,
        selected: Bool,
        select: @escaping () -> Void,
        close: @escaping () -> Void
    ) {
        self.session = session
        _state = ObservedObject(wrappedValue: session.state)
        self.preferredTitle = preferredTitle
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
        .padding(.leading, 12)
        .padding(.trailing, 5)
        .frame(minWidth: 104)
        .frame(height: 30)
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
        .help(title)
        .accessibilityLabel(title)
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.easeOut(duration: 0.12), value: selected)
    }

    private var title: String {
        let preferred = preferredTitle?.trimmingCharacters(in: .whitespacesAndNewlines)
        if let preferred, !preferred.isEmpty { return preferred }
        if let taskTitle = session.taskTitle?.trimmingCharacters(in: .whitespacesAndNewlines),
           !taskTitle.isEmpty {
            return taskTitle
        }
        let reported = state.title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !reported.isEmpty,
              reported != "Terminal",
              CodingAgent.detect(commandLine: "", title: reported) == nil
        else { return fallbackTitle }
        return reported
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
