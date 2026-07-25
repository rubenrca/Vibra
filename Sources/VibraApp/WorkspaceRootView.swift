import Combine
import GhosttyTerminal
import SwiftUI
import VibraCore

struct WorkspaceRootView: View {
    @StateObject private var store = WorkspaceStore()
    @StateObject private var gitModel = GitSidebarModel()

    var body: some View {
        HStack(spacing: 0) {
            if store.isProjectSidebarVisible {
                ProjectSidebar(store: store)
                Divider()
            }
            WorkspaceDetail(store: store, gitModel: gitModel)
            if store.isGitSidebarVisible, let project = store.selectedProject {
                Divider()
                GitSidebarView(
                    fallbackRoot: project.rootPath,
                    session: project.selectedSession,
                    store: store,
                    model: gitModel
                )
            }
        }
        .frame(minWidth: 760, minHeight: 500)
        .background(VibraPalette.canvas)
        .background {
            KeyboardShortcutMonitor(store: store)
                .frame(width: 0, height: 0)
        }
        .background {
            if let project = store.selectedProject,
               let session = project.selectedSession {
                GitContextSync(
                    state: session.state,
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
        .onReceive(NotificationCenter.default.publisher(for: .showGitSidebarRequested)) { _ in
            store.toggleGitSidebar()
        }
        .onReceive(
            NotificationCenter.default.publisher(
                for: NSApplication.willTerminateNotification
            )
        ) { _ in
            store.shutdownAll()
        }
    }
}

private struct ProjectSidebar: View {
    @ObservedObject var store: WorkspaceStore

    var body: some View {
        VStack(spacing: 0) {
            Color.clear.frame(height: VibraLayout.panelHeaderHeight)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 13) {
                    ForEach(store.projects) { project in
                        ProjectSidebarSection(project: project, store: store)
                    }
                }
                .padding(.horizontal, 7)
                .padding(.top, 5)
                .padding(.bottom, 14)
            }
            Spacer(minLength: 0)
            Button {
                store.newWorkspace()
            } label: {
                Label("New workspace", systemImage: "plus")
                    .font(.system(size: 12, weight: .semibold))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 10)
                    .frame(height: 34)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 6))
            .padding(7)
        }
        .frame(width: 242)
        .background(.thinMaterial)
    }
}

private struct ProjectSidebarSection: View {
    let project: VibraProject
    @ObservedObject var store: WorkspaceStore
    @State private var hoveringHeader = false

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            projectHeader
            ForEach(project.workspaces) { workspace in
                SidebarWorkspaceRow(
                    workspace: workspace,
                    selected: store.selectedProjectID == project.id
                        && project.selectedWorkspaceID == workspace.id,
                    store: store,
                    select: { store.selectWorkspace(workspace.id, in: project.id) },
                    close: { store.closeWorkspace(workspace.id, in: project.id) }
                )
            }
        }
    }

    private var projectHeader: some View {
        HStack(spacing: 7) {
            Image(systemName: "folder.fill")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 14)
            Text(project.name)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Spacer(minLength: 4)
            Button {
                store.selectProject(project.id)
                store.newWorkspace()
            } label: {
                Image(systemName: "plus")
                    .font(.system(size: 9, weight: .semibold))
                    .frame(width: 18, height: 18)
            }
            .buttonStyle(.plain)
            .opacity(hoveringHeader ? 0.8 : 0)
            .help("New workspace in \(project.name)")
        }
        .padding(.horizontal, 9)
        .frame(height: 24)
        .contentShape(Rectangle())
        .onTapGesture { store.selectProject(project.id) }
        .onHover { hoveringHeader = $0 }
        .contextMenu {
            Button("Close Project", role: .destructive) {
                store.closeProject(project.id)
            }
        }
    }

}

private struct SidebarWorkspaceRow: View {
    let workspace: TerminalWorkspace
    let selected: Bool
    @ObservedObject var store: WorkspaceStore
    let select: () -> Void
    let close: () -> Void

    @State private var hovering = false

    var body: some View {
        HStack(spacing: 8) {
            SidebarActivityGlyph(activity: workspace.agentActivity, selected: selected)
                .frame(width: 15)
            VStack(alignment: .leading, spacing: 2) {
                Text(workspace.name)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(selected ? Color.white : .primary)
                    .lineLimit(1)
                Text(detail)
                    .font(.system(size: 9.75, weight: .regular))
                    .foregroundStyle(selected ? Color.white.opacity(0.72) : detailColor)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            Button(action: close) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .frame(width: 18, height: 22)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(selected ? Color.white.opacity(0.72) : .secondary)
            .opacity(hovering ? 1 : 0)
        }
        .padding(.leading, 9)
        .padding(.trailing, 6)
        .frame(height: 47)
        .background {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(
                    selected
                        ? VibraPalette.accent
                        : hovering ? Color.primary.opacity(0.055) : .clear
                )
        }
        .contentShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .onTapGesture(perform: select)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.easeOut(duration: 0.12), value: selected)
        .contextMenu {
            Button("New tab") {
                select()
                store.newSession()
            }
            Divider()
            Button("Close workspace", role: .destructive, action: close)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(workspace.name), \(detail)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var detail: String {
        let tabCount = workspace.tabs.count
        let tabs = "\(tabCount) \(tabCount == 1 ? "tab" : "tabs")"
        switch workspace.agentActivity {
        case .idle:
            return tabs
        case .ready(let agent):
            return "\(agent.displayName) idle · \(tabs)"
        case .running(let agent, _):
            return "\(agent.displayName) working · \(tabs)"
        case .needsAttention(let agent, _):
            return "\(agent.displayName) needs attention · \(tabs)"
        case .finished(let agent, let succeeded, _):
            return succeeded == false
                ? "\(agent.displayName) error · \(tabs)"
                : "\(agent.displayName) finished · \(tabs)"
        }
    }

    private var detailColor: Color {
        switch workspace.agentActivity {
        case .idle: .secondary
        case .ready: .secondary
        case .running: VibraPalette.accent
        case .needsAttention: .orange
        case .finished(_, let succeeded, _): succeeded == false ? .red : .green
        }
    }
}

private struct SidebarActivityGlyph: View {
    let activity: AgentActivity
    let selected: Bool

    var body: some View {
        switch activity {
        case .idle:
            Image(systemName: "terminal")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(selected ? Color.white.opacity(0.68) : .secondary)
        case .ready:
            Image(systemName: "pause.circle.fill")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(selected ? Color.white.opacity(0.78) : .secondary)
        case .running:
            ProgressView()
                .controlSize(.mini)
                .scaleEffect(0.72)
                .tint(selected ? .white : VibraPalette.accent)
        case .needsAttention:
            Image(systemName: "bell.badge.fill")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(selected ? Color.white : .orange)
        case .finished(_, let succeeded, _):
            Image(systemName: succeeded == false ? "xmark.circle.fill" : "checkmark.circle.fill")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(selected ? Color.white : succeeded == false ? .red : .green)
        }
    }
}

private struct WorkspaceDetail: View {
    @ObservedObject var store: WorkspaceStore
    @ObservedObject var gitModel: GitSidebarModel

    var body: some View {
        VStack(spacing: 0) {
            SessionHeader(store: store, gitModel: gitModel)
            Divider()
            terminalCanvas
        }
        .background(VibraPalette.canvas)
    }

    @ViewBuilder
    private var terminalCanvas: some View {
        if store.projects.isEmpty {
            EmptyWorkspaceView(
                title: "Open a project",
                detail: "Choose a folder to start a terminal workspace.",
                buttonTitle: "Open Project",
                action: store.chooseProject
            )
        } else if store.selectedSession == nil {
            EmptyWorkspaceView(
                title: "No terminal sessions",
                detail: "Start a session in \(store.selectedProject?.name ?? "this project").",
                buttonTitle: "New Terminal",
                action: store.newSession
            )
        } else {
            TerminalCanvas(store: store)
            .padding(2)
        }
    }
}

private struct TerminalCanvas: View {
    @ObservedObject var store: WorkspaceStore

    var body: some View {
        ZStack {
            hiddenSessions
            if let project = store.selectedProject {
                splitLayout(project)
            }
        }
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
                focus: { store.focusSession($0, in: project.id) }
            )
        }
    }
}

private struct PaneTreeView: View {
    let layout: PaneLayoutSnapshot
    let sessions: [TerminalSession]
    let selectedSessionID: UUID?
    let focus: (UUID) -> Void

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
                            focus: focus
                        )
                        PaneTreeView(
                            layout: second,
                            sessions: sessions,
                            selectedSessionID: selectedSessionID,
                            focus: focus
                        )
                    }
                    .background(Color.primary.opacity(0.18))
                )
            }
            return AnyView(
                VStack(spacing: 1) {
                    PaneTreeView(
                        layout: first,
                        sessions: sessions,
                        selectedSessionID: selectedSessionID,
                        focus: focus
                    )
                    PaneTreeView(
                        layout: second,
                        sessions: sessions,
                        selectedSessionID: selectedSessionID,
                        focus: focus
                    )
                }
                .background(Color.primary.opacity(0.18))
            )
        }
    }
}

private struct TerminalPane: View {
    let session: TerminalSession
    let focused: Bool
    let focus: () -> Void
    @ObservedObject private var state: TerminalViewState

    init(session: TerminalSession, focused: Bool, focus: @escaping () -> Void) {
        self.session = session
        self.focused = focused
        self.focus = focus
        _state = ObservedObject(wrappedValue: session.state)
    }

    var body: some View {
        TerminalSurfaceHost(session: session, isVisible: true, isFocused: focused)
            .onChange(of: state.isFocused) { _, isFocused in
                if isFocused && !focused { focus() }
            }
    }
}

private struct SessionHeader: View {
    @ObservedObject var store: WorkspaceStore
    @ObservedObject var gitModel: GitSidebarModel

    var body: some View {
        HStack(spacing: 8) {
            if let project = store.selectedProject {
                HStack(spacing: 7) {
                    Image(systemName: "folder")
                        .font(.system(size: 10.5, weight: .medium))
                        .foregroundStyle(.secondary)
                    Text(project.name)
                        .font(.system(size: 11.5, weight: .semibold))
                        .lineLimit(1)
                        .frame(maxWidth: 120)
                    if let workspace = project.selectedWorkspace,
                       project.workspaces.count > 1 {
                        Text("/")
                            .foregroundStyle(.tertiary)
                        Text(workspace.name)
                            .font(.system(size: 10.5, weight: .medium))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .frame(maxWidth: 100)
                    }
                }
                .padding(.leading, 2)

                Divider()
                    .frame(height: 18)

                if let workspace = project.selectedWorkspace {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 3) {
                            ForEach(Array(workspace.tabs.enumerated()), id: \.element.id) {
                                index, tab in
                                headerTab(tab, index: index, in: workspace, project: project)
                            }

                            Button {
                                store.newSession()
                            } label: {
                                Image(systemName: "plus")
                                    .font(.system(size: 9, weight: .semibold))
                                    .frame(width: 24, height: 24)
                                    .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .foregroundStyle(.secondary)
                            .background(
                                Color.primary.opacity(0.045),
                                in: RoundedRectangle(cornerRadius: 5, style: .continuous)
                            )
                            .help("New terminal tab")
                        }
                        .padding(.vertical, 4)
                    }
                }
            }
            Spacer(minLength: 0)
            if store.selectedProject != nil {
                Button {
                    store.showGitSidebar()
                } label: {
                    HStack(spacing: 5) {
                        Text("+\(gitModel.additions)").foregroundStyle(.green)
                        Text("−\(gitModel.deletions)").foregroundStyle(.red)
                    }
                    .font(.system(size: 9.5, weight: .medium, design: .monospaced))
                    .padding(.horizontal, 7)
                    .frame(height: 24)
                    .background(Color.primary.opacity(0.055), in: RoundedRectangle(cornerRadius: 6))
                }
                .buttonStyle(.plain)
                .help("Open Git Changes · Toggle sidebar with ⌘R")
            }
        }
        .padding(.trailing, 10)
        .padding(.leading, store.isProjectSidebarVisible ? 9 : 78)
        .frame(height: VibraLayout.panelHeaderHeight)
        .background(.bar)
    }

    @ViewBuilder
    private func headerTab(
        _ tab: TerminalTab,
        index: Int,
        in workspace: TerminalWorkspace,
        project: VibraProject
    ) -> some View {
        let selected = workspace.selectedTabID == tab.id
        let fallbackTitle = workspace.tabs.count == 1 ? "Terminal" : "Terminal \(index + 1)"
        let select = { store.selectTab(tab.id, in: project.id) }
        let close = { store.closeTab(tab.id, in: project.id) }

        if let session = tab.selectedSession ?? tab.sessions.first {
            HeaderTerminalTab(
                session: session,
                fallbackTitle: fallbackTitle,
                paneCount: tab.sessions.count,
                selected: selected,
                select: select,
                close: close
            )
        } else {
            HeaderTerminalTabButton(
                title: fallbackTitle,
                activity: .idle,
                paneCount: 0,
                selected: selected,
                select: select,
                close: close
            )
        }
    }
}

private struct HeaderTerminalTab: View {
    @ObservedObject var session: TerminalSession
    @ObservedObject private var state: TerminalViewState
    let fallbackTitle: String
    let paneCount: Int
    let selected: Bool
    let select: () -> Void
    let close: () -> Void

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
        HeaderTerminalTabButton(
            title: title,
            activity: session.agentActivity,
            paneCount: paneCount,
            selected: selected,
            select: select,
            close: close
        )
    }

    private var title: String {
        let reported = state.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return reported.isEmpty || reported == "Terminal" ? fallbackTitle : reported
    }
}

private struct HeaderTerminalTabButton: View {
    let title: String
    let activity: AgentActivity
    let paneCount: Int
    let selected: Bool
    let select: () -> Void
    let close: () -> Void

    @State private var hovering = false

    var body: some View {
        HStack(spacing: 6) {
            HeaderTabActivityGlyph(activity: activity, selected: selected)
            Text(title)
                .font(.system(size: 11, weight: selected ? .semibold : .regular))
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
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .opacity(hovering || selected ? 0.72 : 0)
        }
        .padding(.leading, 9)
        .padding(.trailing, 3)
        .frame(minWidth: 88)
        .frame(height: 28)
        .background {
            RoundedRectangle(cornerRadius: 5, style: .continuous)
                .fill(
                    selected
                        ? Color.primary.opacity(0.085)
                        : hovering ? Color.primary.opacity(0.04) : .clear
                )
        }
        .overlay(alignment: .bottom) {
            Capsule()
                .fill(selected ? VibraPalette.accent : .clear)
                .frame(height: 2)
                .padding(.horizontal, 8)
        }
        .contentShape(RoundedRectangle(cornerRadius: 5, style: .continuous))
        .onTapGesture(perform: select)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.easeOut(duration: 0.12), value: selected)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Terminal tab \(title)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

private struct HeaderTabActivityGlyph: View {
    let activity: AgentActivity
    let selected: Bool

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
                .tint(selected ? VibraPalette.accent : .secondary)
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
                .tint(VibraPalette.accent)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

enum VibraPalette {
    static let accent = Color(
        nsColor: NSColor(name: nil) { appearance in
            let dark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            return NSColor(
                srgbRed: 0,
                green: dark ? 145.0 / 255.0 : 136.0 / 255.0,
                blue: 1,
                alpha: 1
            )
        }
    )
    static let canvas = Color(nsColor: .windowBackgroundColor)
}

enum VibraLayout {
    static let panelHeaderHeight: CGFloat = 40
}
