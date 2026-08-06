import AppKit
import GhosttyTerminal
import SwiftUI

struct GitSidebarView: View {
    let fallbackRoot: String
    @ObservedObject var store: WorkspaceStore
    @ObservedObject var model: GitSidebarModel
    @StateObject private var fileTree = RepositoryFileTreeModel()
    @Environment(\.appChrome) private var chrome

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(chrome.quietBorder)
            content
        }
        .background(chrome.panel)
        .animation(.easeOut(duration: 0.2), value: model.expandedChangeID)
        .onAppear { syncFileTree() }
        .onChange(of: model.repositoryRoot) { _, _ in syncFileTree() }
        .onChange(of: model.changes) { _, _ in
            if store.rightSidebarMode == .files {
                fileTree.refreshVisibleDirectories()
            }
        }
    }

    private var header: some View {
        HStack(spacing: 5) {
            changesSummary
                .layoutPriority(1)

            Spacer(minLength: 0)
            modeButton(.changes, image: "arrow.triangle.branch")
            modeButton(.files, image: "folder")
            if model.isRefreshing {
                ProgressView()
                    .controlSize(.small)
                    .scaleEffect(0.65)
                    .frame(width: 18, height: 18)
            }
            headerButton("arrow.clockwise", help: "Refresh") { model.refresh() }
        }
        .padding(.horizontal, 11)
        .frame(height: VibraLayout.panelHeaderHeight)
        .background(chrome.panelHeader)
    }

    private var changesSummary: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(model.branch.isEmpty ? "Git" : model.branch)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(chrome.foreground)
                .lineLimit(1)
            HStack(spacing: 6) {
                if model.repositoryRoot.isEmpty {
                    Text("Not a git repository")
                } else {
                    Text("\(model.changes.count) changed")
                    if !model.changes.isEmpty {
                        Text("+\(model.additions)")
                            .foregroundStyle(.green)
                        Text("−\(model.deletions)")
                            .foregroundStyle(.red)
                    }
                }
            }
            .font(.system(size: 9.5, weight: .medium, design: .monospaced))
            .foregroundStyle(chrome.secondaryForeground)
        }
    }

    @ViewBuilder
    private var content: some View {
        if store.rightSidebarMode == .files {
            RepositoryFileTreeView(
                treeModel: fileTree,
                gitModel: model
            )
        } else if let error = model.errorMessage, model.repositoryRoot.isEmpty {
            sidebarMessage(error)
        } else if model.changes.isEmpty, !model.isRefreshing {
            sidebarMessage(
                model.repositoryRoot.isEmpty
                    ? "The current terminal directory is not inside a Git repository."
                    : "Working tree clean.",
                icon: model.repositoryRoot.isEmpty ? "arrow.triangle.branch" : "checkmark"
            )
        } else {
            VStack(spacing: 0) {
                changesFilterBar
                if model.filteredChanges.isEmpty {
                    sidebarMessage("No changes match the filter.", icon: "line.3.horizontal.decrease")
                } else {
                    ScrollView {
                        LazyVStack(spacing: 0) {
                            ForEach(model.filteredChanges) { change in
                                InlineGitDiffCard(
                                    change: change,
                                    repositoryRoot: model.repositoryRoot,
                                    model: model
                                )
                            }
                        }
                    }
                }
            }
        }
    }

    private var changesFilterBar: some View {
        HStack(spacing: 6) {
            Image(systemName: "line.3.horizontal.decrease")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(chrome.secondaryForeground)
            TextField(
                "Filter changes",
                text: Binding(
                    get: { model.changesFilter },
                    set: { model.changesFilter = $0 }
                )
            )
            .textFieldStyle(.plain)
            .font(.system(size: 11))
            if !model.changesFilter.isEmpty {
                Button {
                    model.changesFilter = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 11))
                        .foregroundStyle(chrome.secondaryForeground)
                }
                .buttonStyle(.plain)
                .help("Clear filter")
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(chrome.panelHeader.opacity(0.65))
        .overlay(alignment: .bottom) {
            Rectangle().fill(chrome.quietBorder).frame(height: 1)
        }
    }

    private func headerButton(
        _ systemImage: String,
        help: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 9.5, weight: .semibold))
                .frame(width: 22, height: 22)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .help(help)
    }

    private func sidebarMessage(_ text: String, icon: String? = nil) -> some View {
        VStack(spacing: 7) {
            if let icon {
                Image(systemName: icon)
                    .font(.system(size: 20, weight: .light))
                    .foregroundStyle(.tertiary)
            }
            Text(text)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 240)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }

    private func modeButton(_ mode: RightSidebarMode, image: String) -> some View {
        let selected = store.rightSidebarMode == mode
        return Button {
            store.selectRightSidebarMode(mode)
            if mode == .files { syncFileTree() }
        } label: {
            Image(systemName: image)
                .font(.system(size: 9.5, weight: .semibold))
                .frame(width: 23, height: 23)
                .background(
                    selected ? chrome.workspaceSelection : .clear,
                    in: RoundedRectangle(cornerRadius: 5, style: .continuous)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(selected ? chrome.foreground : chrome.secondaryForeground)
        .help(modeHelp(for: mode))
    }

    private func modeHelp(for mode: RightSidebarMode) -> String {
        switch mode {
        case .files: "Repository Files"
        case .changes: "Git Changes"
        }
    }

    private func syncFileTree() {
        let root = model.repositoryRoot.isEmpty ? fallbackRoot : model.repositoryRoot
        fileTree.sync(root: root)
    }

}

private struct InlineGitDiffCard: View {
    let change: GitFileChange
    let repositoryRoot: String
    @ObservedObject var model: GitSidebarModel
    @State private var hovering = false
    @Environment(\.appChrome) private var chrome

    private var expanded: Bool { model.expandedChangeID == change.id }

    var body: some View {
        VStack(spacing: 0) {
            header
            if expanded {
                Divider()
                actionBar
                Divider()
                DiffLinesCanvas(
                    lines: model.diffLines,
                    layout: model.diffLayoutStyle,
                    filePath: change.path,
                    minimumCodeWidth: model.diffLayoutStyle == .split ? 280 : 420,
                    minimumHeight: 160
                )
                .frame(minHeight: 180, maxHeight: 420)
                .background(chrome.recessed)
                .overlay {
                    if model.isLoadingDiff, model.selectedChangeID == change.id {
                        ProgressView().controlSize(.small)
                    }
                }
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .background(expanded ? chrome.workspaceSelection.opacity(0.55) : hovering ? chrome.workspaceHover : .clear)
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(chrome.quietBorder)
                .frame(height: 1)
        }
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.16), value: expanded)
        .animation(.easeOut(duration: 0.12), value: hovering)
        .contextMenu {
            if change.hasWorktreeChanges {
                Button("Stage") {
                    ensureSelected()
                    model.stageSelected()
                }
            }
            if change.hasStagedChanges {
                Button("Unstage") {
                    ensureSelected()
                    model.unstageSelected()
                }
            }
            Divider()
            openInEditorContextMenuItems(
                path: (repositoryRoot as NSString).appendingPathComponent(change.path)
            )
            Button("Reveal in Finder") { reveal() }
        }
    }

    private var header: some View {
        Button { model.toggleInline(change) } label: {
            HStack(spacing: 8) {
                Image(systemName: expanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .frame(width: 12)
                Text(change.compactStatus)
                    .font(.system(size: 9.5, weight: .bold, design: .monospaced))
                    .foregroundStyle(statusColor)
                    .frame(width: 18)
                Image(systemName: FileTypeIcon.systemImage(forFileName: change.fileName))
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .frame(width: 14)
                Text(change.path)
                    .font(.system(size: 11.5, weight: .medium))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 6)
                changeStats
            }
            .padding(.horizontal, 10)
            .frame(maxWidth: .infinity, minHeight: 36)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(expanded ? "Collapse diff" : "Show diff")
    }

    private var actionBar: some View {
        HStack(spacing: 8) {
            sidebarLayoutPicker

            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(change.path, forType: .string)
            } label: {
                Image(systemName: "doc.on.doc")
                    .font(.system(size: 10, weight: .medium))
            }
            .help("Copy Path")

            Button(action: reveal) {
                Image(systemName: "arrow.up.forward.square")
                    .font(.system(size: 10, weight: .medium))
            }
            .help("Reveal in Finder")

            OpenInEditorButton(
                path: (repositoryRoot as NSString).appendingPathComponent(change.path),
                compact: true
            )

            Spacer(minLength: 0)

            if change.hasStagedChanges {
                Button("Unstage") { model.unstageSelected() }
            }
            if change.hasWorktreeChanges {
                Button("Stage") { model.stageSelected() }
            }
        }
        .font(.system(size: 10, weight: .medium))
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 10)
        .frame(height: 34)
    }

    private var sidebarLayoutPicker: some View {
        HStack(spacing: 0) {
            layoutChip("Unified", style: .unified)
            layoutChip("Split", style: .split)
        }
        .background(chrome.foreground.opacity(0.06), in: RoundedRectangle(cornerRadius: 5, style: .continuous))
    }

    private func layoutChip(_ title: String, style: DiffLayoutStyle) -> some View {
        let selected = model.diffLayoutStyle == style
        return Button {
            model.setDiffLayoutStyle(style)
        } label: {
            Text(title)
                .font(.system(size: 9.5, weight: .medium))
                .padding(.horizontal, 7)
                .frame(height: 20)
                .background(
                    selected ? chrome.workspaceSelection : .clear,
                    in: RoundedRectangle(cornerRadius: 4, style: .continuous)
                )
                .foregroundStyle(selected ? chrome.foreground : chrome.secondaryForeground)
        }
        .buttonStyle(.plain)
        .help(style == .unified ? "Unified layout" : "Side-by-side layout")
    }

    private var changeStats: some View {
        HStack(spacing: 6) {
            if change.additions > 0 { Text("+\(change.additions)").foregroundStyle(.green) }
            if change.deletions > 0 { Text("−\(change.deletions)").foregroundStyle(.red) }
        }
        .font(.system(size: 9.5, weight: .semibold, design: .monospaced))
        .padding(.horizontal, 7)
        .frame(height: 24)
        .background(Color.primary.opacity(0.055), in: RoundedRectangle(cornerRadius: 4))
    }

    private var statusColor: Color {
        if change.isConflict { return .orange }
        if change.isUntracked { return .blue }
        if change.hasStagedChanges { return .green }
        return .orange
    }

    private func ensureSelected() {
        if model.selectedChangeID != change.id {
            model.toggleInline(change)
        }
    }

    private func reveal() {
        let path = (repositoryRoot as NSString).appendingPathComponent(change.path)
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }
}

private struct RepositoryFileTreeView: View {
    @ObservedObject var treeModel: RepositoryFileTreeModel
    @ObservedObject var gitModel: GitSidebarModel

    var body: some View {
        if treeModel.rootPath.isEmpty {
            fileMessage("No repository selected.")
        } else if let error = treeModel.errorMessage {
            fileMessage(error)
        } else if treeModel.rootNodes.isEmpty,
                  treeModel.loadingPaths.contains(treeModel.rootPath) {
            ProgressView().controlSize(.small)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 1) {
                    HStack(spacing: 7) {
                        Image(systemName: "folder.fill")
                            .foregroundStyle(.secondary)
                        Text(URL(fileURLWithPath: treeModel.rootPath).lastPathComponent)
                            .font(.system(size: 12, weight: .semibold))
                        Spacer()
                    }
                    .padding(.horizontal, 8)
                    .frame(height: 32)

                    ForEach(treeModel.rootNodes) { node in
                        RepositoryFileNodeView(
                            node: node,
                            depth: 1,
                            treeModel: treeModel,
                            gitModel: gitModel
                        )
                    }
                }
                .padding(6)
            }
        }
    }

    private func fileMessage(_ message: String) -> some View {
        Text(message)
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.center)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(24)
    }
}

private struct RepositoryFileNodeView: View {
    let node: RepositoryFileNode
    let depth: Int
    @ObservedObject var treeModel: RepositoryFileTreeModel
    @ObservedObject var gitModel: GitSidebarModel
    @Environment(\.appChrome) private var chrome

    var body: some View {
        VStack(spacing: 1) {
            Button(action: activate) {
                HStack(spacing: 6) {
                    if node.isDirectory {
                        Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                            .font(.system(size: 8, weight: .semibold))
                            .foregroundStyle(.tertiary)
                            .frame(width: 10)
                    } else {
                        Color.clear.frame(width: 10)
                    }
                    Image(systemName: fileIcon)
                        .font(.system(size: 11))
                        .foregroundStyle(iconColor)
                        .frame(width: 15)
                    Text(node.name)
                        .font(.system(size: 11.5, weight: node.isDirectory ? .medium : .regular))
                        .foregroundStyle(fileChange == nil ? .primary : statusColor)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if treeModel.loadingPaths.contains(node.path) {
                        ProgressView().controlSize(.mini).scaleEffect(0.65)
                    }
                    if let fileChange {
                        HStack(spacing: 4) {
                            if fileChange.additions > 0 {
                                Text("+\(fileChange.additions)").foregroundStyle(.green)
                            }
                            if fileChange.deletions > 0 {
                                Text("−\(fileChange.deletions)").foregroundStyle(.red)
                            }
                        }
                        .font(.system(size: 8.5, weight: .medium, design: .monospaced))
                    } else if node.isDirectory, hasChangedDescendants {
                        Circle().fill(chrome.accent).frame(width: 4, height: 4)
                    }
                }
                .padding(.leading, CGFloat(depth * 13) + 3)
                .padding(.trailing, 7)
                .frame(height: 28)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .contextMenu {
                openInEditorContextMenuItems(path: node.path)
                Button("Reveal in Finder") {
                    NSWorkspace.shared.activateFileViewerSelecting([
                        URL(fileURLWithPath: node.path)
                    ])
                }
            }

            if node.isDirectory, isExpanded {
                ForEach(treeModel.children(of: node)) { child in
                    RepositoryFileNodeView(
                        node: child,
                        depth: depth + 1,
                        treeModel: treeModel,
                        gitModel: gitModel
                    )
                }
            }
        }
    }

    private var isExpanded: Bool { treeModel.expandedPaths.contains(node.path) }

    private var relativePath: String {
        let root = treeModel.rootPath.hasSuffix("/")
            ? treeModel.rootPath
            : treeModel.rootPath + "/"
        return node.path.hasPrefix(root) ? String(node.path.dropFirst(root.count)) : node.path
    }

    private var fileChange: GitFileChange? {
        gitModel.changes.first { $0.path == relativePath }
    }

    private var hasChangedDescendants: Bool {
        let prefix = relativePath + "/"
        return gitModel.changes.contains { $0.path.hasPrefix(prefix) }
    }

    private var fileIcon: String {
        if node.isDirectory { return isExpanded ? "folder.fill" : "folder" }
        return FileTypeIcon.systemImage(forFileName: node.name, isDirectory: false)
    }

    private var iconColor: Color {
        if node.isDirectory { return Color(red: 0.42, green: 0.52, blue: 0.66) }
        if node.name.hasSuffix(".swift") { return .orange }
        if node.name.hasSuffix(".md") { return .blue }
        return .secondary
    }

    private var statusColor: Color {
        guard let fileChange else { return .primary }
        if fileChange.isUntracked { return .blue }
        if fileChange.hasStagedChanges { return .green }
        return .orange
    }

    private func activate() {
        if node.isDirectory {
            treeModel.toggle(node)
        } else if let fileChange {
            gitModel.present(fileChange)
        }
    }
}

struct GitChangeTreeNode: Identifiable {
    let id: String
    let name: String
    let path: String
    let change: GitFileChange?
    let children: [GitChangeTreeNode]
    let additions: Int
    let deletions: Int

    var isFolder: Bool { change == nil }

    static func makeTree(from changes: [GitFileChange]) -> [GitChangeTreeNode] {
        let root = GitChangeTreeBuilderNode(name: "", path: "")
        for change in changes {
            let components = change.path.split(separator: "/").map(String.init)
            guard let fileName = components.last else { continue }
            var parent = root
            var currentPath = ""
            for directory in components.dropLast() {
                currentPath = currentPath.isEmpty ? directory : currentPath + "/" + directory
                parent = parent.folder(named: directory, path: currentPath)
            }
            parent.files.append(
                GitChangeTreeNode(
                    id: "file:\(change.path)",
                    name: fileName,
                    path: change.path,
                    change: change,
                    children: [],
                    additions: change.additions,
                    deletions: change.deletions
                )
            )
        }
        return root.frozenChildren()
    }
}

private final class GitChangeTreeBuilderNode {
    let name: String
    let path: String
    var folders: [String: GitChangeTreeBuilderNode] = [:]
    var files: [GitChangeTreeNode] = []

    init(name: String, path: String) {
        self.name = name
        self.path = path
    }

    func folder(named name: String, path: String) -> GitChangeTreeBuilderNode {
        if let folder = folders[name] { return folder }
        let folder = GitChangeTreeBuilderNode(name: name, path: path)
        folders[name] = folder
        return folder
    }

    func frozenChildren() -> [GitChangeTreeNode] {
        let folderNodes = folders.values.map { folder -> GitChangeTreeNode in
            let children = folder.frozenChildren()
            return GitChangeTreeNode(
                id: "folder:\(folder.path)",
                name: folder.name,
                path: folder.path,
                change: nil,
                children: children,
                additions: children.reduce(0) { $0 + $1.additions },
                deletions: children.reduce(0) { $0 + $1.deletions }
            )
        }
        .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        let fileNodes = files.sorted {
            $0.name.localizedStandardCompare($1.name) == .orderedAscending
        }
        return folderNodes + fileNodes
    }
}

private struct GitChangeTreeNodeView: View {
    let node: GitChangeTreeNode
    let depth: Int
    let repositoryRoot: String
    @ObservedObject var model: GitSidebarModel
    @Environment(\.appChrome) private var chrome
    @State private var expanded = true

    var body: some View {
        if let change = node.change {
            fileRow(change)
        } else {
            VStack(spacing: 1) {
                folderRow
                if expanded {
                    ForEach(node.children) { child in
                        GitChangeTreeNodeView(
                            node: child,
                            depth: depth + 1,
                            repositoryRoot: repositoryRoot,
                            model: model
                        )
                    }
                }
            }
        }
    }

    private var folderRow: some View {
        Button { expanded.toggle() } label: {
            HStack(spacing: 6) {
                Image(systemName: expanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(.tertiary)
                    .frame(width: 10)
                Image(systemName: expanded ? "folder.fill" : "folder")
                    .font(.system(size: 11))
                    .foregroundStyle(chrome.accent.opacity(0.82))
                    .frame(width: 15)
                Text(node.name)
                    .font(.system(size: 11.5, weight: .medium))
                    .lineLimit(1)
                Spacer(minLength: 4)
                changeStats(additions: node.additions, deletions: node.deletions)
            }
            .padding(.leading, CGFloat(depth * 13) + 5)
            .padding(.trailing, 7)
            .frame(height: 30)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func fileRow(_ change: GitFileChange) -> some View {
        Button { model.present(change) } label: {
            HStack(spacing: 6) {
                Text(change.compactStatus)
                    .font(.system(size: 9.5, weight: .bold, design: .monospaced))
                    .foregroundStyle(statusColor(change))
                    .frame(width: 20)
                Image(systemName: FileTypeIcon.systemImage(forFileName: change.fileName))
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .frame(width: 13)
                Text(node.name)
                    .font(.system(size: 11.5))
                    .lineLimit(1)
                Spacer(minLength: 4)
                changeStats(additions: change.additions, deletions: change.deletions)
                if change.hasStagedChanges {
                    Circle().fill(Color.green).frame(width: 5, height: 5)
                        .help("Includes staged changes")
                }
            }
            .padding(.leading, CGFloat(depth * 13) + 5)
            .padding(.trailing, 7)
            .frame(height: 30)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu {
            if change.hasWorktreeChanges {
                Button("Stage") {
                    model.present(change)
                    model.stageSelected()
                }
            }
            if change.hasStagedChanges {
                Button("Unstage") {
                    model.present(change)
                    model.unstageSelected()
                }
            }
            Divider()
            openInEditorContextMenuItems(
                path: (repositoryRoot as NSString).appendingPathComponent(change.path)
            )
            Button("Reveal in Finder") { reveal(change) }
        }
    }

    private func changeStats(additions: Int, deletions: Int) -> some View {
        HStack(spacing: 5) {
            if additions > 0 { Text("+\(additions)").foregroundStyle(.green) }
            if deletions > 0 { Text("−\(deletions)").foregroundStyle(.red) }
        }
        .font(.system(size: 9, weight: .medium, design: .monospaced))
    }

    private func statusColor(_ change: GitFileChange) -> Color {
        if change.isConflict { return .orange }
        if change.isUntracked { return .blue }
        if change.hasStagedChanges { return .green }
        return .orange
    }

    private func reveal(_ change: GitFileChange) {
        let path = (repositoryRoot as NSString).appendingPathComponent(change.path)
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }
}


struct GitContextSync: View {
    @ObservedObject var session: TerminalSession
    let fallbackRoot: String
    @ObservedObject var model: GitSidebarModel

    var body: some View {
        Color.clear
            .onAppear { sync() }
            .onChange(of: session.liveWorkingDirectory) { _, _ in sync() }
            .onChange(of: session.foregroundProcessID) { _, _ in sync() }
            .task(id: session.id) {
                // Periodic foreground cwd refresh so worktree re-root stays live.
                while !Task.isCancelled {
                    session.refreshWorkingDirectory()
                    sync()
                    try? await Task.sleep(for: .seconds(2))
                }
            }
    }

    private func sync() {
        session.refreshWorkingDirectory()
        let shell = session.workingDirectory
        let foregroundDir: String?
        if let pid = session.foregroundProcessID,
           let foreground = ProcessWorkingDirectoryProbe.directory(for: pid),
           !foreground.isEmpty,
           foreground != shell
        {
            foregroundDir = foreground
        } else {
            foregroundDir = nil
        }
        let resolution = PanelRootResolver.resolve(
            projectRoot: fallbackRoot,
            shellDirectory: shell.isEmpty ? fallbackRoot : shell,
            foregroundDirectory: foregroundDir,
            gitTopLevel: { GitClient.repositoryTopLevel(from: $0) }
        )
        model.sync(root: resolution.root, source: resolution.source)
    }
}
