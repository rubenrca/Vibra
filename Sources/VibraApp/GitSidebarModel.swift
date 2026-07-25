import Foundation

@MainActor
final class GitSidebarModel: ObservableObject {
    @Published private(set) var repositoryRoot = ""
    @Published private(set) var branch = ""
    @Published private(set) var changes: [GitFileChange] = []
    @Published private(set) var selectedChangeID: String?
    @Published private(set) var expandedChangeID: String?
    @Published private(set) var diffLines: [DiffLine] = []
    @Published private(set) var isRefreshing = false
    @Published private(set) var isLoadingDiff = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var operationMessage: String?
    @Published var isDiffPresented = false

    private var requestedRoot = ""
    private var watchedRoot = ""
    private var watcher: RepositoryWatcher?
    private var refreshTask: Task<Void, Never>?
    private var diffTask: Task<Void, Never>?
    private var debounceTask: Task<Void, Never>?
    private var generation: UInt = 0
    private var refreshPending = false

    var additions: Int { changes.reduce(0) { $0 + $1.additions } }
    var deletions: Int { changes.reduce(0) { $0 + $1.deletions } }

    var selectedChange: GitFileChange? {
        changes.first { $0.id == selectedChangeID }
    }

    func sync(root: String) {
        guard requestedRoot != root else { return }
        requestedRoot = root
        generation &+= 1
        refreshTask?.cancel()
        refreshTask = nil
        diffTask?.cancel()
        debounceTask?.cancel()
        refreshPending = false
        repositoryRoot = ""
        branch = ""
        changes = []
        selectedChangeID = nil
        expandedChangeID = nil
        diffLines = []
        errorMessage = nil
        operationMessage = nil
        isDiffPresented = false
        watcher?.stop()
        watcher = nil
        watchedRoot = ""
        refresh(showActivity: true)
    }

    func refresh(showActivity: Bool = true) {
        let root = requestedRoot
        guard !root.isEmpty else { return }
        guard refreshTask == nil else {
            refreshPending = true
            return
        }
        let requestGeneration = generation
        if showActivity { isRefreshing = true }
        refreshTask = Task { [weak self] in
            let result = await Task.detached(priority: .utility) {
                Result { try GitClient.snapshot(from: root) }
            }.value
            guard let self, !Task.isCancelled, self.generation == requestGeneration else {
                return
            }
            if self.isRefreshing { self.isRefreshing = false }
            self.refreshTask = nil
            switch result {
            case .success(let snapshot):
                if self.errorMessage != nil { self.errorMessage = nil }
                let snapshotChanged = self.repositoryRoot != snapshot.rootPath
                    || self.branch != snapshot.branch
                    || self.changes != snapshot.changes
                if snapshotChanged {
                    self.repositoryRoot = snapshot.rootPath
                    self.branch = snapshot.branch
                    self.changes = snapshot.changes
                }
                self.installWatcher(for: snapshot.rootPath)

                if !snapshot.changes.contains(where: { $0.id == self.selectedChangeID }) {
                    self.selectedChangeID = nil
                    self.expandedChangeID = nil
                    self.isDiffPresented = false
                    self.diffLines = []
                }
                if snapshotChanged,
                   self.isDiffPresented || self.expandedChangeID != nil {
                    self.loadSelectedDiff()
                }
            case .failure(let error):
                self.repositoryRoot = ""
                self.branch = ""
                self.changes = []
                self.selectedChangeID = nil
                self.expandedChangeID = nil
                self.diffLines = []
                self.errorMessage = error.localizedDescription
                self.isDiffPresented = false
                self.watcher?.stop()
                self.watcher = nil
                self.watchedRoot = ""
            }
            if self.refreshPending {
                self.refreshPending = false
                self.refresh(showActivity: false)
            }
        }
    }

    func present(_ change: GitFileChange) {
        presentModal(change)
    }

    func toggleInline(_ change: GitFileChange) {
        if expandedChangeID == change.id {
            expandedChangeID = nil
            guard !isDiffPresented else { return }
            selectedChangeID = nil
            diffTask?.cancel()
            diffLines = []
            isLoadingDiff = false
            return
        }

        expandedChangeID = change.id
        select(change)
    }

    func presentModal(_ change: GitFileChange) {
        if expandedChangeID != change.id {
            expandedChangeID = nil
        }
        select(change)
        isDiffPresented = true
    }

    func dismissDiff() {
        isDiffPresented = false
        guard expandedChangeID == nil else { return }
        selectedChangeID = nil
        diffTask?.cancel()
        diffLines = []
        isLoadingDiff = false
    }

    func stageSelected() {
        guard let change = selectedChange else { return }
        perform(message: "Staged \(change.fileName)") { root in
            try GitClient.stage(change, in: root)
        }
    }

    func unstageSelected() {
        guard let change = selectedChange else { return }
        perform(message: "Unstaged \(change.fileName)") { root in
            try GitClient.unstage(change, in: root)
        }
    }

    func stageAll() {
        perform(message: "Staged all changes") { root in
            try GitClient.stageAll(in: root)
        }
    }

    func unstageAll() {
        perform(message: "Unstaged all changes") { root in
            try GitClient.unstageAll(in: root)
        }
    }

    private func perform(
        message: String,
        operation: @escaping @Sendable (String) throws -> Void
    ) {
        let root = repositoryRoot
        guard !root.isEmpty else { return }
        operationMessage = nil
        errorMessage = nil
        Task { [weak self] in
            let result = await Task.detached(priority: .userInitiated) {
                Result { try operation(root) }
            }.value
            guard let self else { return }
            switch result {
            case .success:
                self.operationMessage = message
                self.refresh(showActivity: false)
            case .failure(let error):
                self.errorMessage = error.localizedDescription
            }
        }
    }

    private func select(_ change: GitFileChange) {
        operationMessage = nil
        guard selectedChangeID != change.id else { return }
        selectedChangeID = change.id
        loadSelectedDiff()
    }

    private func loadSelectedDiff() {
        diffTask?.cancel()
        diffLines = []
        guard let change = selectedChange, !repositoryRoot.isEmpty else {
            isLoadingDiff = false
            return
        }
        let root = repositoryRoot
        let requestGeneration = generation
        isLoadingDiff = true
        diffTask = Task { [weak self] in
            let result = await Task.detached(priority: .utility) {
                Result { try GitClient.diff(for: change, in: root) }
            }.value
            guard let self, !Task.isCancelled, self.generation == requestGeneration,
                  self.selectedChangeID == change.id else { return }
            self.isLoadingDiff = false
            switch result {
            case .success(let lines):
                self.diffLines = lines
            case .failure(let error):
                self.diffLines = []
                self.errorMessage = error.localizedDescription
            }
        }
    }

    private func scheduleRefresh() {
        guard UserDefaults.standard.bool(forKey: SettingsKeys.gitAutoRefreshEnabled) else {
            return
        }
        debounceTask?.cancel()
        let configuredDelay = UserDefaults.standard.integer(forKey: SettingsKeys.gitRefreshDelay)
        let delay = configuredDelay > 0 ? configuredDelay : 420
        debounceTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(delay))
            guard !Task.isCancelled else { return }
            self?.refresh(showActivity: false)
        }
    }

    private func installWatcher(for root: String) {
        guard !root.isEmpty, watchedRoot != root else { return }
        watcher?.stop()
        watchedRoot = root
        watcher = RepositoryWatcher(path: root) { [weak self] _ in
            DispatchQueue.main.async {
                self?.scheduleRefresh()
            }
        }
        watcher?.start()
    }
}
