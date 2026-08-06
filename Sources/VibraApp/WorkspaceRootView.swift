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
        var savedGitWidth = UserDefaults.standard.double(forKey: SettingsKeys.gitSidebarWidth)
        // One-time narrow of the previous 420pt default so the denser changes
        // list leaves more room for the full-area center diff.
        if !UserDefaults.standard.bool(forKey: SettingsKeys.gitSidebarWidthNarrowed) {
            if savedGitWidth <= 0 || abs(savedGitWidth - 420) < 0.5 {
                savedGitWidth = 300
                UserDefaults.standard.set(300.0, forKey: SettingsKeys.gitSidebarWidth)
            }
            UserDefaults.standard.set(true, forKey: SettingsKeys.gitSidebarWidthNarrowed)
        }
        _terminalSidebarWidth = State(initialValue: savedWidth > 0 ? savedWidth : 270)
        _gitSidebarWidth = State(initialValue: savedGitWidth > 0 ? savedGitWidth : 300)
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
            + (gitModel.expandedChangeID == nil ? 0 : 160)
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
        min(max(width, 240), 560)
    }
}

private struct TerminalSidebar: View {
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 7) {
                HStack(spacing: 8) {
                    Text("Sessions")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(chrome.foreground.opacity(0.82))
                    Spacer(minLength: 0)
                    Text("\(store.sidebarWorkspaces.count)")
                        .font(.system(size: 9, weight: .semibold, design: .rounded))
                        .foregroundStyle(chrome.secondaryForeground.opacity(0.9))
                        .frame(minWidth: 18, minHeight: 18)
                        .background(
                            chrome.elevated.opacity(chrome.isDark ? 0.72 : 0.9),
                            in: RoundedRectangle(cornerRadius: 5, style: .continuous)
                        )
                }
                .padding(.leading, 5)
                .padding(.trailing, 3)
                .padding(.bottom, 1)

                ForEach(store.sidebarWorkspaces) { located in
                    SidebarAgentRow(located: located, store: store)
                }
            }
            .padding(.horizontal, 9)
            .padding(.top, 11)
            .padding(.bottom, 12)
            .animation(
                .spring(response: 0.26, dampingFraction: 0.9),
                value: store.sidebarWorkspaces.map(\.id)
            )
        }
        .frame(maxWidth: .infinity)
        .background(chrome.panel)
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
            title: "Local",
            detail: localDetail,
            count: store.ungroupedWorkspaces.count,
            selected: isSelected,
            hovering: hovering,
            systemImage: "desktopcomputer"
        )
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
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
            RoundedRectangle(cornerRadius: 8, style: .continuous)
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
                    selected: folder.id == store.selectedProject?.id,
                    hovering: hovering,
                    systemImage: "folder"
                )
            }
        }
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
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
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .stroke(chrome.accent.opacity(isDropTargeted ? 0.85 : 0), lineWidth: 1)
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
                .foregroundStyle(chrome.secondaryForeground)
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
        .background(chrome.workspaceSelection, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func commitRename() {
        store.renameFolder(folder.id, to: name)
        isRenaming = false
    }

    private var rootName: String {
        let name = URL(fileURLWithPath: folder.project.rootPath).lastPathComponent
        return name.isEmpty ? "local" : name
    }

}

private struct SidebarSpaceLabel: View {
    let title: String
    let detail: String
    let count: Int
    let selected: Bool
    let hovering: Bool
    let systemImage: String
    @Environment(\.appChrome) private var chrome

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: systemImage)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(
                    selected
                        ? chrome.foreground.opacity(0.9)
                        : chrome.secondaryForeground.opacity(0.84)
                )
                .frame(width: 14)

            Text(title)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(chrome.foreground.opacity(selected ? 1 : 0.86))
                .lineLimit(1)

            Spacer(minLength: 0)

            Text(detail)
                .font(.system(size: 9.5, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(selected ? 0.9 : 0.68))
                .lineLimit(1)

            Text("\(count)")
                .font(.system(size: 9, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(0.52))
                .frame(minWidth: 12, alignment: .trailing)
        }
        .padding(.horizontal, 9)
        .frame(height: 32)
        .background {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(selected ? chrome.workspaceSelection : hovering ? chrome.workspaceHover : .clear)
                .overlay(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 1, style: .continuous)
                        .fill(chrome.accent)
                        .frame(width: 2)
                        .padding(.vertical, 8)
                        .opacity(selected ? 0.78 : 0)
                }
        }
    }
}

private struct SidebarAgentRow: View {
    let located: LocatedTerminalWorkspace
    @ObservedObject var store: WorkspaceStore
    @Environment(\.appChrome) private var chrome
    @ObservedObject private var session: TerminalSession
    @State private var hovering = false
    @State private var isDropTargeted = false
    @State private var branchName: String?
    @State private var isRenaming = false
    @State private var renameDraft = ""
    @FocusState private var isRenameFocused: Bool

    init(
        located: LocatedTerminalWorkspace,
        store: WorkspaceStore
    ) {
        self.located = located
        self.store = store
        let session = located.workspace.selectedSession
            ?? located.workspace.tabs[0].sessions[0]
        _session = ObservedObject(wrappedValue: session)
    }

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            SidebarAgentBadge(activity: workspaceActivity, selected: selected)
                .frame(width: 18, height: 18)
                .padding(5)
                .background(
                    selected
                        ? chrome.accent.opacity(chrome.isDark ? 0.13 : 0.1)
                        : chrome.foreground.opacity(chrome.isDark ? 0.035 : 0.025),
                    in: RoundedRectangle(cornerRadius: 7, style: .continuous)
                )

            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    if isRenaming {
                        TextField("Session name", text: $renameDraft)
                            .textFieldStyle(.plain)
                            .font(.system(size: 12.5, weight: .semibold))
                            .focused($isRenameFocused)
                            .onSubmit(commitRename)
                            .onExitCommand(perform: cancelRename)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        Text(title)
                            .font(.system(size: 12.5, weight: .semibold))
                            .foregroundStyle(chrome.foreground.opacity(selected ? 1 : 0.9))
                            .lineLimit(1)
                            .truncationMode(.tail)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .layoutPriority(1)
                    }
                }

                HStack(spacing: 6) {
                    if let activitySummary {
                        Text(activitySummary)
                            .font(.system(size: 9.5, weight: .semibold))
                            .foregroundStyle(activitySummaryColor)
                            .lineLimit(1)

                        metadataSeparator
                    }

                    if let branchName, !branchName.isEmpty {
                        Image(systemName: "arrow.triangle.branch")
                            .font(.system(size: 8, weight: .semibold))
                        Text(branchName)
                            .lineLimit(1)
                    }

                    if branchName?.isEmpty == false {
                        metadataSeparator
                    }

                    Text(directoryLabel)
                        .lineLimit(1)
                        .truncationMode(.middle)

                    Spacer(minLength: 3)

                    if located.workspace.tabs.count > 1 {
                        HStack(spacing: 3) {
                            Image(systemName: "rectangle.stack")
                                .font(.system(size: 7.5, weight: .semibold))
                            Text("\(located.workspace.tabs.count)")
                        }
                        .foregroundStyle(chrome.secondaryForeground.opacity(0.72))
                    }
                }
                .font(.system(size: 9, weight: .medium, design: .monospaced))
                .foregroundStyle(chrome.secondaryForeground.opacity(selected ? 0.82 : 0.62))
            }
        }
        .padding(.leading, 9)
        .padding(.trailing, 10)
        .padding(.vertical, 9)
        .frame(minHeight: rowHeight)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .topTrailing) {
            if hovering && !isRenaming {
                Button {
                    store.closeWorkspace(located.id, in: located.projectID)
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 8, weight: .bold))
                        .frame(width: 20, height: 20)
                }
                .buttonStyle(.plain)
                .foregroundStyle(chrome.secondaryForeground.opacity(0.78))
                .background(
                    chrome.elevated.opacity(0.94),
                    in: RoundedRectangle(cornerRadius: 5, style: .continuous)
                )
                .padding(6)
                .transition(.opacity.combined(with: .scale(scale: 0.92)))
            }
        }
        .background {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(
                    selected
                        ? chrome.elevated.opacity(chrome.isDark ? 0.9 : 0.96)
                        : hovering
                            ? chrome.workspaceHover
                            : chrome.elevated.opacity(chrome.isDark ? 0.22 : 0.32)
                )
                .overlay {
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(
                            isDropTargeted
                                ? chrome.accent.opacity(0.8)
                                : selected
                                    ? chrome.strongBorder.opacity(0.8)
                                    : chrome.quietBorder.opacity(0.35),
                            lineWidth: isDropTargeted ? 1 : 0.5
                        )
                }
        }
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
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
                activity: workspaceActivity
            )
        }
        .dropDestination(for: String.self) { items, location in
            guard let value = items.first,
                  let workspaceID = UUID(uuidString: value),
                  workspaceID != located.id
            else { return false }

            let position: TabDropPosition = location.y > rowHeight / 2 ? .after : .before
            withAnimation(.spring(response: 0.24, dampingFraction: 0.88)) {
                store.reorderSidebarWorkspace(
                    workspaceID,
                    relativeTo: located.id,
                    position: position
                )
            }
            return true
        } isTargeted: { targeted in
            withAnimation(.easeOut(duration: 0.12)) {
                isDropTargeted = targeted
            }
        }
        .contextMenu {
            Button("Rename session", action: beginRename)
            if located.workspace.titleSource == .manual {
                Button("Use task title automatically") {
                    store.useAutomaticWorkspaceTitle(located.id, in: located.projectID)
                }
            }
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
            Button("Close session", role: .destructive) {
                store.closeWorkspace(located.id, in: located.projectID)
            }
        }
        .help("\(title)\n\(abbreviatedPath)")
        .accessibilityLabel(accessibilityLabel)
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.spring(response: 0.24, dampingFraction: 0.9), value: selected)
        .animation(.easeOut(duration: 0.12), value: isDropTargeted)
        .animation(.easeOut(duration: 0.16), value: workspaceActivity)
    }

    private var selected: Bool { store.selectedWorkspace?.id == located.id }

    private var workspaceActivity: AgentActivity { located.workspace.agentActivity }

    private var rowHeight: CGFloat { 58 }

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

    private var directoryLabel: String {
        abbreviatedPath
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
        switch workspaceActivity {
        case .idle:
            return nil
        case .ready:
            return "Ready"
        case .running:
            return "Working"
        case .needsAttention:
            return "Needs input"
        case .finished(_, let succeeded, _):
            return succeeded == false ? "Failed" : "Finished"
        }
    }

    private var metadataSeparator: some View {
        Circle()
            .fill(chrome.secondaryForeground.opacity(0.42))
            .frame(width: 2.5, height: 2.5)
    }

    private var activitySummaryColor: Color {
        switch workspaceActivity {
        case .running:
            chrome.accent.opacity(selected ? 1 : 0.88)
        case .needsAttention:
            Color.orange.opacity(selected ? 1 : 0.9)
        case .finished(_, let succeeded, _):
            succeeded == false
                ? Color.red.opacity(0.92)
                : Color.green.opacity(selected ? 0.9 : 0.76)
        case .idle, .ready:
            chrome.secondaryForeground.opacity(selected ? 0.92 : 0.76)
        }
    }

    private var accessibilityLabel: String {
        [title, activitySummary, branchName, abbreviatedPath]
            .compactMap { value in
                guard let value, !value.isEmpty else { return nil }
                return value
            }
            .joined(separator: ", ")
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
            AgentBrandMark(
                agent: activity.agent,
                color: selected ? chrome.foreground : chrome.secondaryForeground,
                size: 17
            )
            .frame(width: 18, height: 18)

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

/// Agent logos deliberately inherit the active terminal theme. The mark tells
/// agents apart; colour is reserved for selection and live status.
private struct AgentBrandMark: View {
    let agent: CodingAgent?
    let color: Color
    let size: CGFloat

    var body: some View {
        Group {
            if let agent, let brandImage = AgentMarkAsset.image(for: agent) {
                Image(nsImage: brandImage)
                    .resizable()
                    .renderingMode(agent == .grok ? .original : .template)
                    .scaledToFit()
                    .foregroundStyle(color)
                    .padding(0.5)
            } else {
                Image(systemName: "terminal")
                    .font(.system(size: max(8, size * 0.68), weight: .medium))
                    .foregroundStyle(color)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

private enum AgentMarkAsset {
    static func image(for agent: CodingAgent) -> NSImage? {
        let assetName = name(for: agent)
        guard let url = Bundle.module.url(forResource: assetName, withExtension: "svg")
            ?? Bundle.module.url(forResource: assetName, withExtension: "png") else { return nil }
        return NSImage(contentsOf: url)
    }

    private static func name(for agent: CodingAgent) -> String {
        switch agent {
        case .codex: "codex"
        case .claude: "claude"
        case .gemini: "gemini"
        case .opencode: "opencode"
        case .aider: "aider"
        case .goose: "goose"
        case .amp: "amp"
        case .cursor: "cursor"
        case .grok: "grok"
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
                tabID: tab.id,
                session: session,
                preferredTitle: workspace.tabs.count == 1
                    ? workspace.name
                    : session.taskTitle,
                fallbackTitle: workspace.tabs.count == 1 ? "Terminal" : "Terminal \(index + 1)",
                paneCount: tab.sessions.count,
                selected: workspace.selectedTabID == tab.id,
                select: { store.selectTab(tab.id, in: project.id) },
                close: { store.closeTab(tab.id, in: project.id) },
                reorder: { sourceID, position in
                    store.reorderTab(
                        sourceID,
                        in: project.id,
                        relativeTo: tab.id,
                        position: position
                    )
                }
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
                if store.selectedProject != nil {
                    // Prefer the active console cwd over project.rootPath.
                    // Ungrouped projects pin rootPath to $HOME, which is wrong for IDE open.
                    OpenInEditorButton(
                        compact: true,
                        hasPath: store.selectedSession != nil
                            || store.selectedProject?.rootPath != nil,
                        resolvePath: editorTargetPath
                    )
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

    /// Directory the IDE button should open: live console cwd when available.
    private func editorTargetPath() -> String? {
        if let session = store.selectedSession {
            session.refreshWorkingDirectory()
            return session.workingDirectory
        }
        return store.selectedProject?.rootPath
    }

    private var windowControls: some View {
        HStack(spacing: 2) {
            WindowChromeButton(
                systemImage: store.isTerminalSidebarVisible ? "sidebar.left" : "sidebar.right",
                help: "Toggle Terminal Sidebar · ⌘B",
                action: store.toggleTerminalSidebar
            )

            WindowChromeButton(
                systemImage: "plus",
                help: "New tab · ⌘N",
                action: { store.newWorkspace() }
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
    let tabID: UUID
    @ObservedObject var session: TerminalSession
    @ObservedObject private var state: TerminalViewState
    @Environment(\.appChrome) private var chrome
    let preferredTitle: String?
    let fallbackTitle: String
    let paneCount: Int
    let selected: Bool
    let select: () -> Void
    let close: () -> Void
    let reorder: (UUID, TabDropPosition) -> Void
    @State private var hovering = false
    @State private var isDropTargeted = false

    init(
        tabID: UUID,
        session: TerminalSession,
        preferredTitle: String?,
        fallbackTitle: String,
        paneCount: Int,
        selected: Bool,
        select: @escaping () -> Void,
        close: @escaping () -> Void,
        reorder: @escaping (UUID, TabDropPosition) -> Void
    ) {
        self.tabID = tabID
        self.session = session
        _state = ObservedObject(wrappedValue: session.state)
        self.preferredTitle = preferredTitle
        self.fallbackTitle = fallbackTitle
        self.paneCount = paneCount
        self.selected = selected
        self.select = select
        self.close = close
        self.reorder = reorder
    }

    var body: some View {
        HStack(spacing: 6) {
            HeaderTabAgentMark(activity: session.agentActivity, selected: selected)
            Text(title)
                .font(.system(size: 11.5, weight: selected ? .semibold : .medium))
                .foregroundStyle(chrome.foreground.opacity(selected ? 1 : 0.78))
                .lineLimit(1)
                .frame(maxWidth: 156)
            if paneCount > 1 {
                HStack(spacing: 3) {
                    Image(systemName: "square.split.2x1")
                        .font(.system(size: 7.5, weight: .semibold))
                    Text("\(paneCount)")
                        .font(.system(size: 8.5, weight: .semibold, design: .rounded))
                }
                .foregroundStyle(chrome.secondaryForeground.opacity(0.75))
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
        .frame(height: 31)
        .background {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(
                    selected
                        ? chrome.elevated.opacity(chrome.isDark ? 0.88 : 0.95)
                        : hovering ? chrome.workspaceHover : .clear
                )
                .overlay {
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .stroke(
                            isDropTargeted
                                ? chrome.accent.opacity(0.72)
                                : selected ? chrome.quietBorder : .clear,
                            lineWidth: isDropTargeted ? 1.25 : 1
                        )
                }
                .overlay(alignment: .bottom) {
                    Capsule()
                        .fill(chrome.accent)
                        .frame(height: 2)
                        .padding(.horizontal, 12)
                        .opacity(selected ? 0.9 : 0)
                }
        }
        .contentShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .onTapGesture(perform: select)
        .onHover { hovering = $0 }
        .draggable(tabID.uuidString) {
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .padding(.horizontal, 11)
                .padding(.vertical, 7)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        }
        .dropDestination(for: String.self) { items, location in
            guard let value = items.first,
                  let draggedTabID = UUID(uuidString: value),
                  draggedTabID != tabID
            else { return false }

            let position: TabDropPosition = location.x > 72 ? .after : .before
            withAnimation(.spring(response: 0.24, dampingFraction: 0.88)) {
                reorder(draggedTabID, position)
            }
            return true
        } isTargeted: { targeted in
            withAnimation(.easeOut(duration: 0.12)) {
                isDropTargeted = targeted
            }
        }
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

private struct HeaderTabAgentMark: View {
    let activity: AgentActivity
    let selected: Bool
    @Environment(\.appChrome) private var chrome

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            AgentBrandMark(
                agent: activity.agent,
                color: selected ? chrome.foreground : chrome.secondaryForeground,
                size: 12
            )
            .frame(width: 13, height: 13)

            if let statusColor {
                Circle()
                    .fill(statusColor)
                    .frame(width: 4.5, height: 4.5)
                    .overlay {
                        Circle()
                            .stroke(chrome.panelHeader, lineWidth: 1)
                    }
                    .offset(x: 1, y: 1)
            }
        }
        .frame(width: 13, height: 13)
        .accessibilityLabel(accessibilityLabel)
    }

    private var statusColor: Color? {
        switch activity {
        case .idle, .finished(_, true, _), .finished(_, nil, _): nil
        case .ready: chrome.foreground.opacity(0.62)
        case .running, .needsAttention: chrome.accent
        case .finished(_, false, _): .red
        }
    }

    private var accessibilityLabel: String {
        let name = activity.agent?.displayName ?? "Terminal"
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
