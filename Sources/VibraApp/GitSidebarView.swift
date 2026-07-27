import AppKit
import GhosttyTerminal
import SwiftUI

struct GitSidebarView: View {
    let fallbackRoot: String
    let session: TerminalSession?
    @ObservedObject var store: WorkspaceStore
    @ObservedObject var model: GitSidebarModel
    @StateObject private var fileTree = RepositoryFileTreeModel()

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
        }
        .frame(width: model.expandedChangeID == nil ? 320 : 540)
        .background(.regularMaterial)
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
        HStack(spacing: 7) {
            VStack(alignment: .leading, spacing: 1) {
                Text(headerTitle)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                if store.rightSidebarMode == .changes {
                    HStack(spacing: 6) {
                        Text(summary)
                        if !model.changes.isEmpty {
                            Text("+\(model.additions)")
                                .foregroundStyle(.green)
                            Text("−\(model.deletions)")
                                .foregroundStyle(.red)
                        }
                    }
                    .font(.system(size: 9.5, design: .monospaced))
                    .foregroundStyle(.tertiary)
                } else {
                    Text(model.repositoryRoot)
                        .font(.system(size: 9.5))
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
            Spacer(minLength: 0)
            modeButton(.files, image: "folder")
            modeButton(.changes, image: "arrow.triangle.branch")
            if model.isRefreshing, store.rightSidebarMode == .changes {
                ProgressView()
                    .controlSize(.small)
                    .scaleEffect(0.65)
                    .frame(width: 18, height: 18)
            }
            if store.rightSidebarMode == .changes {
                headerButton("arrow.clockwise", help: "Refresh") { model.refresh() }
            }
            headerButton("xmark", help: "Close Git Sidebar") { store.toggleGitSidebar() }
        }
        .padding(.horizontal, 11)
        .frame(height: VibraLayout.panelHeaderHeight)
    }

    @ViewBuilder
    private var content: some View {
        if store.rightSidebarMode == .files {
            RepositoryFileTreeView(
                treeModel: fileTree,
                gitModel: model
            )
        } else if let error = model.errorMessage, model.repositoryRoot.isEmpty {
            sidebarMessage(error, icon: "exclamationmark.triangle")
        } else if model.changes.isEmpty, !model.isRefreshing {
            sidebarMessage(
                model.repositoryRoot.isEmpty
                    ? "The current terminal directory is not inside a Git repository."
                    : "Working tree clean.",
                icon: model.repositoryRoot.isEmpty ? "arrow.triangle.branch" : "checkmark"
            )
        } else {
            ScrollView {
                LazyVStack(spacing: 8) {
                    ForEach(model.changes) { change in
                        InlineGitDiffCard(
                            change: change,
                            repositoryRoot: model.repositoryRoot,
                            model: model
                        )
                    }
                }
                .padding(8)
            }
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

    private func sidebarMessage(_ text: String, icon: String) -> some View {
        VStack(spacing: 7) {
            Image(systemName: icon)
                .font(.system(size: 20, weight: .light))
                .foregroundStyle(.tertiary)
            Text(text)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 240)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }

    private var summary: String {
        guard !model.repositoryRoot.isEmpty else { return "Repository" }
        return "\(model.changes.count) changed"
    }

    private var headerTitle: String {
        if store.rightSidebarMode == .changes {
            return model.branch.isEmpty ? "Git Changes" : model.branch
        }
        guard !model.repositoryRoot.isEmpty else { return "Files" }
        return URL(fileURLWithPath: model.repositoryRoot).lastPathComponent
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
                    selected ? Color.primary.opacity(0.09) : .clear,
                    in: RoundedRectangle(cornerRadius: 5)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(selected ? VibraPalette.accent : .secondary)
        .help(mode == .files ? "Repository Files" : "Git Changes")
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

    private var expanded: Bool { model.expandedChangeID == change.id }

    var body: some View {
        VStack(spacing: 0) {
            header
            if expanded {
                Divider()
                actionBar
                Divider()
                GitDiffLinesView(
                    model: model,
                    axes: .horizontal,
                    minimumCodeWidth: 680,
                    minimumHeight: 110
                )
                .background(Color(nsColor: .textBackgroundColor).opacity(0.48))
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .background(Color.primary.opacity(expanded ? 0.052 : hovering ? 0.038 : 0.024))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .stroke(Color.primary.opacity(expanded ? 0.15 : 0.09), lineWidth: 1)
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
            Button("Open Large Diff") { model.presentModal(change) }
            Button("Reveal in Finder") { reveal() }
        }
    }

    private var header: some View {
        HStack(spacing: 4) {
            Button { model.toggleInline(change) } label: {
                HStack(spacing: 9) {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .frame(width: 12)
                    Text(change.compactStatus)
                        .font(.system(size: 9.5, weight: .bold, design: .monospaced))
                        .foregroundStyle(statusColor)
                        .frame(width: 18)
                    Text(change.path)
                        .font(.system(size: 11.5, weight: .medium))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 6)
                    changeStats
                }
                .padding(.leading, 11)
                .frame(maxWidth: .infinity, minHeight: 46)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Button { model.presentModal(change) } label: {
                Image(systemName: "arrow.up.left.and.arrow.down.right")
                    .font(.system(size: 10.5, weight: .medium))
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("Open Large Diff")
            .padding(.trailing, 7)
        }
    }

    private var actionBar: some View {
        HStack(spacing: 8) {
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(change.path, forType: .string)
            } label: {
                Label("Copy path", systemImage: "doc.on.doc")
            }
            .help("Copy Path")

            Button(action: reveal) {
                Label("Reveal", systemImage: "arrow.up.forward.square")
            }
            .help("Reveal in Finder")

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
        .padding(.horizontal, 12)
        .frame(height: 34)
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
                        Circle().fill(VibraPalette.accent).frame(width: 4, height: 4)
                    }
                }
                .padding(.leading, CGFloat(depth * 13) + 3)
                .padding(.trailing, 7)
                .frame(height: 28)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .contextMenu {
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
        switch URL(fileURLWithPath: node.name).pathExtension.lowercased() {
        case "swift": return "swift"
        case "md": return "text.document"
        case "json": return "curlybraces"
        case "yml", "yaml": return "list.bullet.rectangle"
        case "png", "jpg", "jpeg", "gif", "webp": return "photo"
        default: return "doc"
        }
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
                    .foregroundStyle(VibraPalette.accent.opacity(0.82))
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
                Image(systemName: "doc")
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

struct GitDiffModalView: View {
    @ObservedObject var model: GitSidebarModel
    @FocusState private var receivesKeyboardInput: Bool
    @State private var fileExpanded = true

    var body: some View {
        ZStack {
            Color.black.opacity(0.72)
                .ignoresSafeArea()
                .onTapGesture { model.dismissDiff() }

            VStack(spacing: 0) {
                workspaceHeader
                Divider().overlay(Color.white.opacity(0.09))
                reviewHeader
                diffCard
                    .padding(.horizontal, 22)
                    .padding(.bottom, 22)
            }
            .background(Color(red: 0.045, green: 0.047, blue: 0.052))
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .stroke(Color.white.opacity(0.15), lineWidth: 1)
            }
            .shadow(color: .black.opacity(0.58), radius: 42, y: 20)
            .frame(maxWidth: 1180, maxHeight: 820)
            .padding(22)
        }
        .focusable()
        .focused($receivesKeyboardInput)
        .onAppear { receivesKeyboardInput = true }
        .onExitCommand { model.dismissDiff() }
        .onKeyPress(.escape) {
            model.dismissDiff()
            return .handled
        }
    }

    private var workspaceHeader: some View {
        HStack(spacing: 10) {
            Text(displayRoot)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.head)
            Text(":")
                .foregroundStyle(.tertiary)
            Text(model.branch.isEmpty ? "HEAD" : model.branch)
                .font(.system(size: 12, weight: .semibold))
            Image(systemName: "doc")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            Text("\(model.changes.count)")
                .font(.system(size: 11, weight: .medium, design: .monospaced))
                .foregroundStyle(.secondary)
            Text("·").foregroundStyle(.tertiary)
            stat("+\(model.additions)", color: .green)
            stat("−\(model.deletions)", color: .red)
            Spacer(minLength: 0)
            Button { model.dismissDiff() } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 13, weight: .medium))
                    .frame(width: 28, height: 28)
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("Close Diff (Esc)")
        }
        .padding(.horizontal, 18)
        .frame(height: 48)
    }

    private var reviewHeader: some View {
        HStack(spacing: 10) {
            Image(systemName: "arrow.left.arrow.right")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
            Text("Uncommitted changes")
                .font(.system(size: 14, weight: .semibold))
            Spacer()
            if let change = model.selectedChange {
                if change.hasStagedChanges {
                    actionButton("Unstage", action: model.unstageSelected)
                }
                if change.hasWorktreeChanges {
                    actionButton("Stage", action: model.stageSelected)
                }
            }
        }
        .padding(.horizontal, 22)
        .frame(height: 54)
    }

    private var diffCard: some View {
        VStack(spacing: 0) {
            fileHeader
            if fileExpanded {
                Divider().overlay(Color.white.opacity(0.08))
                GitDiffLinesView(
                    model: model,
                    axes: [.horizontal, .vertical],
                    minimumCodeWidth: 860,
                    minimumHeight: 180
                )
                .background(Color(red: 0.025, green: 0.027, blue: 0.030))
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(Color.white.opacity(0.12), lineWidth: 1)
        }
    }

    private var fileHeader: some View {
        HStack(spacing: 10) {
            Button { fileExpanded.toggle() } label: {
                Image(systemName: fileExpanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .frame(width: 18, height: 24)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            if let change = model.selectedChange {
                Text(change.fileName)
                    .font(.system(size: 13, weight: .medium))
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(change.path, forType: .string)
                } label: {
                    Image(systemName: "doc.on.doc")
                        .font(.system(size: 10))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Copy Path")
                Spacer(minLength: 0)
                HStack(spacing: 7) {
                    stat("+\(change.additions)", color: .green)
                    stat("−\(change.deletions)", color: .red)
                }
                .padding(.horizontal, 10)
                .frame(height: 28)
                .background(Color.black.opacity(0.32), in: RoundedRectangle(cornerRadius: 4))
                Button {
                    let path = (model.repositoryRoot as NSString)
                        .appendingPathComponent(change.path)
                    NSWorkspace.shared.activateFileViewerSelecting([
                        URL(fileURLWithPath: path)
                    ])
                } label: {
                    Image(systemName: "arrow.up.forward.square")
                        .font(.system(size: 12))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Reveal in Finder")
            } else {
                Text("Diff").font(.system(size: 13, weight: .medium))
                Spacer()
            }
        }
        .padding(.horizontal, 16)
        .frame(height: 52)
        .background(Color.white.opacity(0.075))
    }

    private func stat(_ text: String, color: Color) -> some View {
        Text(text)
            .font(.system(size: 10, weight: .semibold, design: .monospaced))
            .foregroundStyle(color)
    }

    private func actionButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(title, action: action)
            .buttonStyle(.bordered)
            .controlSize(.small)
            .tint(.secondary)
    }

    private var displayRoot: String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        if model.repositoryRoot == home { return "~" }
        if model.repositoryRoot.hasPrefix(home + "/") {
            return "~/" + model.repositoryRoot.dropFirst(home.count + 1)
        }
        return model.repositoryRoot
    }

}

private struct GitDiffLinesView: View {
    @ObservedObject var model: GitSidebarModel
    let axes: Axis.Set
    let minimumCodeWidth: CGFloat
    let minimumHeight: CGFloat

    var body: some View {
        Group {
            if model.isLoadingDiff {
                ProgressView().controlSize(.small)
                    .frame(maxWidth: .infinity, minHeight: minimumHeight)
            } else if presentedLines.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "doc.text").foregroundStyle(.tertiary)
                    Text("No textual diff to display.")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, minHeight: minimumHeight)
            } else {
                ScrollView(axes) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(presentedLines) { presented in
                            if let omitted = presented.omittedLines {
                                omittedRow(omitted, fallback: presented.line.text)
                            } else {
                                diffRow(presented.line)
                            }
                        }
                    }
                    .fixedSize(horizontal: true, vertical: false)
                }
                .defaultScrollAnchor(.topLeading)
            }
        }
    }

    private func diffRow(_ line: DiffLine) -> some View {
        HStack(spacing: 0) {
            Rectangle()
                .fill(accentColor(line.kind))
                .frame(width: 3)
            lineNumber(line.oldLine)
            lineNumber(line.newLine)
            Text(sign(for: line.kind))
                .foregroundStyle(lineColor(line.kind))
                .frame(width: 22)
            Text(verbatim: displayText(line))
                .foregroundStyle(lineColor(line.kind))
                .padding(.trailing, 16)
                .frame(minWidth: minimumCodeWidth, maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
        }
        .font(.system(size: 11.5, design: .monospaced))
        .frame(minHeight: 22)
        .background(lineBackground(line.kind))
    }

    private func omittedRow(_ count: Int, fallback: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "ellipsis")
                .font(.system(size: 10, weight: .semibold))
            Text(count > 0 ? "\(count) unmodified lines" : fallback)
        }
        .font(.system(size: 10.5, design: .monospaced))
        .foregroundStyle(.secondary)
        .padding(.leading, 88)
        .frame(
            minWidth: minimumCodeWidth + 100,
            maxWidth: .infinity,
            minHeight: 28,
            alignment: .leading
        )
        .background(Color.primary.opacity(0.035))
    }

    private func lineNumber(_ value: Int?) -> some View {
        Text(value.map(String.init) ?? "")
            .font(.system(size: 9.5, design: .monospaced))
            .foregroundStyle(.quaternary)
            .frame(width: 46, alignment: .trailing)
            .padding(.trailing, 8)
            .background(Color.black.opacity(0.08))
    }

    private func displayText(_ line: DiffLine) -> String {
        switch line.kind {
        case .addition, .deletion:
            return String(line.text.dropFirst())
        case .context where line.oldLine != nil:
            return line.text.hasPrefix(" ") ? String(line.text.dropFirst()) : line.text
        default:
            return line.text.isEmpty ? " " : line.text
        }
    }

    private func sign(for kind: DiffLineKind) -> String {
        switch kind {
        case .addition: "+"
        case .deletion: "−"
        default: ""
        }
    }

    private func lineColor(_ kind: DiffLineKind) -> Color {
        switch kind {
        case .metadata: .secondary
        case .hunk: VibraPalette.accent
        case .addition: .green
        case .deletion: .red
        case .context: .primary
        }
    }

    private func lineBackground(_ kind: DiffLineKind) -> Color {
        switch kind {
        case .addition: Color.green.opacity(0.16)
        case .deletion: Color.red.opacity(0.14)
        case .hunk: Color.primary.opacity(0.035)
        default: .clear
        }
    }

    private func accentColor(_ kind: DiffLineKind) -> Color {
        switch kind {
        case .addition: .green
        case .deletion: .red
        default: .clear
        }
    }

    private var presentedLines: [PresentedDiffLine] {
        var previousNewEnd = 0
        var result: [PresentedDiffLine] = []
        for line in model.diffLines {
            if line.kind == .metadata { continue }
            if line.kind == .hunk, let range = newRange(from: line.text) {
                let omitted = max(0, range.start - previousNewEnd - 1)
                result.append(
                    PresentedDiffLine(
                        id: line.id,
                        line: line,
                        omittedLines: omitted
                    )
                )
                previousNewEnd = range.start + max(range.count, 1) - 1
            } else {
                result.append(PresentedDiffLine(id: line.id, line: line, omittedLines: nil))
            }
        }
        return result
    }

    private func newRange(from hunk: String) -> (start: Int, count: Int)? {
        guard let field = hunk.split(separator: " ").first(where: { $0.hasPrefix("+") })
        else { return nil }
        let values = field.dropFirst().split(separator: ",", maxSplits: 1)
        guard let start = Int(values[0]) else { return nil }
        return (start, values.count > 1 ? Int(values[1]) ?? 1 : 1)
    }
}

private struct PresentedDiffLine: Identifiable {
    let id: Int
    let line: DiffLine
    let omittedLines: Int?
}

struct GitContextSync: View {
    @ObservedObject var session: TerminalSession
    let fallbackRoot: String
    @ObservedObject var model: GitSidebarModel

    var body: some View {
        Color.clear
            .onAppear { sync(session.workingDirectory) }
            .onChange(of: session.liveWorkingDirectory) { _, directory in sync(directory) }
    }

    private func sync(_ directory: String?) {
        model.sync(root: directory.flatMap { $0.isEmpty ? nil : $0 } ?? fallbackRoot)
    }
}
