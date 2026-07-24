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
                LazyVStack(spacing: 4) {
                    ForEach(store.projects) { project in
                        projectRow(project)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 12)
            }
            Spacer(minLength: 0)
            Button {
                store.chooseProject()
            } label: {
                Label("Open Project", systemImage: "plus")
                    .font(.system(size: 12, weight: .medium))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 10)
                    .frame(height: 32)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .padding(8)
        }
        .frame(width: 216)
        .background(.ultraThinMaterial)
    }

    private func projectRow(_ project: VibraProject) -> some View {
        let selected = store.selectedProjectID == project.id
        return HStack(spacing: 3) {
            Button {
                store.selectProject(project.id)
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: selected ? "folder.fill" : "folder")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(selected ? VibraPalette.accent : .secondary)
                        .frame(width: 16)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(project.name)
                            .font(.system(size: 12, weight: selected ? .semibold : .regular))
                            .foregroundStyle(.primary)
                            .lineLimit(1)
                        Text("\(project.tabs.count) \(project.tabs.count == 1 ? "tab" : "tabs")")
                            .font(.system(size: 9.5))
                            .foregroundStyle(.tertiary)
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .frame(maxWidth: .infinity)

            Button {
                store.closeProject(project.id)
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .semibold))
                    .frame(width: 20, height: 24)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("Close \(project.name)")
        }
        .padding(.leading, 9)
        .padding(.trailing, 5)
        .frame(height: 40)
        .background {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(selected ? Color.primary.opacity(0.075) : .clear)
        }
        .contextMenu {
            Button("Close Project", role: .destructive) {
                store.closeProject(project.id)
            }
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
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 3) {
                        ForEach(Array(project.tabs.enumerated()), id: \.element.id) { index, tab in
                            SessionTab(
                                tab: tab,
                                selected: project.selectedTabID == tab.id,
                                defaultTitle: project.tabs.count == 1
                                    ? "Terminal"
                                    : "Terminal \(index + 1)",
                                select: {
                                    store.selectTab(tab.id, in: project.id)
                                },
                                close: {
                                    store.closeTab(tab.id, in: project.id)
                                }
                            )
                        }
                    }
                    .padding(.horizontal, 8)
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
        .padding(.leading, store.selectedProject == nil ? 78 : 0)
        .frame(height: VibraLayout.panelHeaderHeight)
        .background(.bar)
    }
}

private struct SessionTab: View {
    let tab: TerminalTab
    let selected: Bool
    let defaultTitle: String
    let select: () -> Void
    let close: () -> Void

    @ObservedObject private var state: TerminalViewState
    @State private var hovering = false

    init(
        tab: TerminalTab,
        selected: Bool,
        defaultTitle: String,
        select: @escaping () -> Void,
        close: @escaping () -> Void
    ) {
        self.tab = tab
        self.selected = selected
        self.defaultTitle = defaultTitle
        self.select = select
        self.close = close
        _state = ObservedObject(wrappedValue: tab.selectedSession!.state)
    }

    var body: some View {
        HStack(spacing: 7) {
            Text(tabTitle)
                .font(.system(size: 11.5, weight: selected ? .medium : .regular))
                .lineLimit(1)
                .frame(maxWidth: 160)
            Button(action: close) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .semibold))
                    .frame(width: 18, height: 20)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .opacity(hovering || selected ? 0.72 : 0)
        }
        .padding(.leading, 10)
        .padding(.trailing, 4)
        .frame(minWidth: 92)
        .frame(height: 30)
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
                .fill(selected ? VibraPalette.accent.opacity(0.9) : .clear)
                .frame(height: 2)
                .padding(.horizontal, 9)
        }
        .contentShape(RoundedRectangle(cornerRadius: 5, style: .continuous))
        .onTapGesture(perform: select)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.easeOut(duration: 0.12), value: selected)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Terminal \(tabTitle)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var tabTitle: String {
        let reported = state.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return reported.isEmpty || reported == "Terminal" ? defaultTitle : reported
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
    static let accent = Color(red: 0.49, green: 0.36, blue: 0.96)
    static let canvas = Color(nsColor: .windowBackgroundColor)
}

enum VibraLayout {
    static let panelHeaderHeight: CGFloat = 44
}
