import Foundation

struct RepositoryFileNode: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let path: String
    let isDirectory: Bool
}

@MainActor
final class RepositoryFileTreeModel: ObservableObject {
    @Published private(set) var rootPath = ""
    @Published private(set) var rootNodes: [RepositoryFileNode] = []
    @Published private(set) var childrenByPath: [String: [RepositoryFileNode]] = [:]
    @Published private(set) var expandedPaths: Set<String> = []
    @Published private(set) var loadingPaths: Set<String> = []
    @Published private(set) var errorMessage: String?

    private var generation: UInt = 0

    func sync(root: String) {
        guard !root.isEmpty, root != rootPath else { return }
        generation &+= 1
        rootPath = root
        rootNodes = []
        childrenByPath = [:]
        expandedPaths = []
        loadingPaths = []
        errorMessage = nil
        loadDirectory(root, isRoot: true)
    }

    func toggle(_ node: RepositoryFileNode) {
        guard node.isDirectory else { return }
        if expandedPaths.remove(node.path) != nil { return }
        expandedPaths.insert(node.path)
        if childrenByPath[node.path] == nil {
            loadDirectory(node.path, isRoot: false)
        }
    }

    func children(of node: RepositoryFileNode) -> [RepositoryFileNode] {
        childrenByPath[node.path] ?? []
    }

    func refreshVisibleDirectories() {
        guard !rootPath.isEmpty else { return }
        loadDirectory(rootPath, isRoot: true)
        for path in expandedPaths where childrenByPath[path] != nil {
            loadDirectory(path, isRoot: false)
        }
    }

    private func loadDirectory(_ path: String, isRoot: Bool) {
        guard !loadingPaths.contains(path) else { return }
        loadingPaths.insert(path)
        let requestGeneration = generation
        Task { [weak self] in
            let result = await Task.detached(priority: .utility) {
                Self.readDirectory(path)
            }.value
            guard let self, self.generation == requestGeneration else { return }
            self.loadingPaths.remove(path)
            switch result {
            case .success(let nodes):
                if isRoot {
                    self.rootNodes = nodes
                } else {
                    self.childrenByPath[path] = nodes
                }
            case .failure(let message):
                if isRoot { self.errorMessage = message }
                self.childrenByPath[path] = []
            }
        }
    }

    private nonisolated static func readDirectory(
        _ path: String
    ) -> FileTreeLoadResult {
        do {
            let keys: [URLResourceKey] = [.isDirectoryKey, .isSymbolicLinkKey]
            let urls = try FileManager.default.contentsOfDirectory(
                at: URL(fileURLWithPath: path, isDirectory: true),
                includingPropertiesForKeys: keys,
                options: []
            )
            let nodes = try urls.prefix(2_000).compactMap { url -> RepositoryFileNode? in
                guard url.lastPathComponent != ".git" else { return nil }
                let values = try url.resourceValues(forKeys: Set(keys))
                let isDirectory = values.isDirectory == true && values.isSymbolicLink != true
                return RepositoryFileNode(
                    id: url.path,
                    name: url.lastPathComponent,
                    path: url.path,
                    isDirectory: isDirectory
                )
            }
            .sorted { lhs, rhs in
                if lhs.isDirectory != rhs.isDirectory { return lhs.isDirectory }
                return lhs.name.localizedStandardCompare(rhs.name) == .orderedAscending
            }
            return .success(nodes)
        } catch {
            return .failure(error.localizedDescription)
        }
    }
}

private enum FileTreeLoadResult: Sendable {
    case success([RepositoryFileNode])
    case failure(String)
}
