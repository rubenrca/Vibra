import Foundation
import Testing
@testable import VibraApp

// MARK: - Agent completion notifications

@Test func agentCompletionTransitionOnlyEmitsForANewFinishedState() throws {
    let finishedAt = Date(timeIntervalSince1970: 1_800_000_000)
    let finished = AgentActivity.finished(
        agent: .codex,
        succeeded: true,
        at: finishedAt
    )
    let event = try #require(
        AgentCompletionEvent.transition(
            from: .running(agent: .codex, since: finishedAt.addingTimeInterval(-20)),
            to: finished
        )
    )
    #expect(event.agent == .codex)
    #expect(event.succeeded == true)
    #expect(event.finishedAt == finishedAt)
    #expect(AgentCompletionEvent.transition(from: finished, to: finished) == nil)
    #expect(
        AgentCompletionEvent.transition(
            from: finished,
            to: .ready(agent: .codex)
        ) == nil
    )
}

@Test func notificationsRequireARealApplicationBundle() {
    #expect(
        !AgentCompletionNotifier.isBundledApplication(
            bundleURL: URL(fileURLWithPath: "/tmp/Vibra/.build/debug"),
            bundleIdentifier: nil
        )
    )
    #expect(
        AgentCompletionNotifier.isBundledApplication(
            bundleURL: URL(fileURLWithPath: "/Applications/Vibra.app"),
            bundleIdentifier: "dev.vibra.app"
        )
    )
}

// MARK: - Panel root

@Test func panelRootStaysPinnedDuringInRepoCd() {
    let project = "/Users/dev/work/app"
    let nested = "/Users/dev/work/app/Sources/Feature"
    let repos: [String: String] = [
        project: project,
        nested: project,
        "/Users/dev/work/app/Tests": project,
    ]
    let resolution = PanelRootResolver.resolve(
        projectRoot: project,
        shellDirectory: nested,
        foregroundDirectory: nil,
        gitTopLevel: { path in
            repos[path] ?? repos.first { path.hasPrefix($0.key + "/") }?.value
        }
    )
    #expect(resolution.root == project)
    #expect(resolution.source == .shell)
}

@Test func panelRootRerootsToForegroundWorktree() {
    let project = "/Users/dev/work/app"
    let shell = "/Users/dev/work/app"
    let worktree = "/Users/dev/work/app-agent-wt"
    let resolution = PanelRootResolver.resolve(
        projectRoot: project,
        shellDirectory: shell,
        foregroundDirectory: worktree,
        gitTopLevel: { path in
            if path.hasPrefix(worktree) { return worktree }
            if path.hasPrefix(project) { return project }
            return nil
        }
    )
    #expect(resolution.root == worktree)
    #expect(resolution.source == .foregroundWorktree)
}

@Test func panelRootFallsBackOutsideRepository() {
    let project = "/Users/dev/not-a-repo"
    let shell = "/tmp/scratch"
    let resolution = PanelRootResolver.resolve(
        projectRoot: project,
        shellDirectory: shell,
        foregroundDirectory: nil,
        gitTopLevel: { _ in nil }
    )
    #expect(resolution.root == URL(fileURLWithPath: shell).standardizedFileURL.path)
    #expect(resolution.source == .fallback)
}

@Test func panelRootUsesProjectRepoWhenShellIsOutsideGit() {
    let project = "/Users/dev/work/app"
    let shell = "/tmp"
    let resolution = PanelRootResolver.resolve(
        projectRoot: project,
        shellDirectory: shell,
        foregroundDirectory: nil,
        gitTopLevel: { path in
            path.hasPrefix(project) ? project : nil
        }
    )
    #expect(resolution.root == project)
    #expect(resolution.source == .project)
}

// MARK: - Change list filter

@Test func changeListFilterMatchesPathAndFileName() {
    let change = GitFileChange(
        path: "Sources/App/Main.swift",
        originalPath: nil,
        indexStatus: ".",
        worktreeStatus: "M",
        isUntracked: false,
        isConflict: false,
        additions: 2,
        deletions: 1
    )
    #expect(ChangeListFilter.matches(change, query: ""))
    #expect(ChangeListFilter.matches(change, query: "  "))
    #expect(ChangeListFilter.matches(change, query: "main"))
    #expect(ChangeListFilter.matches(change, query: "Sources/App"))
    #expect(!ChangeListFilter.matches(change, query: "README"))
    let filtered = ChangeListFilter.apply(
        [
            change,
            GitFileChange(
                path: "README.md",
                originalPath: nil,
                indexStatus: ".",
                worktreeStatus: "M",
                isUntracked: false,
                isConflict: false,
                additions: 1,
                deletions: 0
            ),
        ],
        query: "swift"
    )
    #expect(filtered.map(\.path) == ["Sources/App/Main.swift"])
}

// MARK: - Word diff

@Test func wordDiffHighlightsPartialLineEdits() {
    let (oldSpans, newSpans) = WordDiff.spans(
        old: "let value = 1",
        new: "let value = 2"
    )
    #expect(oldSpans.contains { $0.kind == .equal && $0.text.contains("let value") })
    #expect(oldSpans.contains { $0.kind == .delete && $0.text.contains("1") })
    #expect(newSpans.contains { $0.kind == .insert && $0.text.contains("2") })
    #expect(newSpans.contains { $0.kind == .equal })
}

@Test func wordDiffPairingsAttachToAdjacentAddDeleteLines() throws {
    let text = """
    diff --git a/a.swift b/a.swift
    --- a/a.swift
    +++ b/a.swift
    @@ -1,2 +1,2 @@
    -let value = 1
    +let value = 2
     let next = 3
    """
    let lines = GitClient.parseDiffText(text)
    let pairings = WordDiff.pairings(for: lines)
    let deletion = try #require(lines.first { $0.kind == .deletion })
    let addition = try #require(lines.first { $0.kind == .addition })
    let delIndex = try #require(lines.firstIndex(where: { $0.id == deletion.id }))
    let addIndex = try #require(lines.firstIndex(where: { $0.id == addition.id }))
    #expect(pairings[delIndex] != nil)
    #expect(pairings[addIndex] != nil)
    let pair = try #require(pairings[delIndex])
    #expect(pair.oldSpans.contains { $0.kind == .delete })
    #expect(pair.newSpans.contains { $0.kind == .insert })
}

// MARK: - Split / unified presentation from real git diffs

@Test func splitAndUnifiedPresentationMapRealGitDiffOutput() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-diff-layout-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }

    try runGit(["init", "-q", "-b", "main"], in: root)
    try runGit(["config", "user.name", "Vibra Tests"], in: root)
    try runGit(["config", "user.email", "tests@vibra.local"], in: root)

    let file = root.appendingPathComponent("sample.swift")
    try """
    import Foundation
    let a = 1
    let b = 2
    let c = 3

    """.write(to: file, atomically: true, encoding: .utf8)
    try runGit(["add", "sample.swift"], in: root)
    try runGit(["commit", "-q", "-m", "Initial"], in: root)

    try """
    import Foundation
    let a = 10
    let b = 2
    let c = 30
    let d = 4

    """.write(to: file, atomically: true, encoding: .utf8)

    let snapshot = try GitClient.snapshot(from: root.path)
    let change = try #require(snapshot.changes.first { $0.path == "sample.swift" })
    let lines = try GitClient.diff(for: change, in: snapshot.rootPath)

    #expect(lines.contains { $0.kind == .addition })
    #expect(lines.contains { $0.kind == .deletion })
    #expect(lines.contains { $0.kind == .context })

    let unified = DiffPresentation.unifiedRows(from: lines)
    #expect(unified.allSatisfy { $0.kind != .metadata })
    #expect(unified.contains { $0.kind == .hunk })

    let split = DiffPresentation.splitRows(from: lines)
    #expect(!split.isEmpty)
    // Hunk header appears on both sides.
    #expect(split.contains { row in
        row.left?.kind == .hunk && row.right?.kind == .hunk
    })
    // Deletions land on the left; additions on the right.
    #expect(split.contains { $0.left?.kind == .deletion })
    #expect(split.contains { $0.right?.kind == .addition })
    // Context is mirrored.
    #expect(split.contains { row in
        row.left?.kind == .context && row.right?.kind == .context
    })

    // Dual gutters are present on content lines (old/new numbers).
    let content = lines.filter { $0.kind == .context || $0.kind == .addition || $0.kind == .deletion }
    #expect(content.contains { $0.oldLine != nil || $0.newLine != nil })
}

// MARK: - Syntax highlight

@Test func syntaxHighlighterDetectsLanguagesAndKeywords() {
    #expect(SyntaxHighlighter.language(forPath: "App.swift") == .swift)
    #expect(SyntaxHighlighter.language(forPath: "main.ts") == .typescript)
    #expect(SyntaxHighlighter.language(forPath: "index.js") == .javascript)
    #expect(SyntaxHighlighter.language(forPath: "package.json") == .json)
    #expect(SyntaxHighlighter.language(forPath: "README.md") == .markdown)
    #expect(SyntaxHighlighter.language(forPath: "setup.sh") == .shell)

    let tokens = SyntaxHighlighter.tokens(in: "let value = 42 // comment", language: .swift)
    #expect(tokens.contains { $0.kind == .keyword })
    #expect(tokens.contains { $0.kind == .number })
    #expect(tokens.contains { $0.kind == .comment })
}

// MARK: - Ports parse

@Test func sessionPortProbeParsesLsofOutput() {
    let sample = """
    COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME
    node    12345 me   23u  IPv4 0x1      0t0  TCP *:3000 (LISTEN)
    python  12345 me   12u  IPv4 0x2      0t0  TCP 127.0.0.1:8080 (LISTEN)
    vite    99999 me   18u  IPv6 0x3      0t0  TCP [::1]:5173 (LISTEN)
    """
    let ports = SessionPortProbe.parseLsof(sample, allowedPIDs: [12345])
    #expect(ports.map(\.port) == [3000, 8080])
    #expect(ports.allSatisfy { $0.pid == 12345 })
    #expect(ports.first?.url?.absoluteString == "http://localhost:3000")
    #expect(SessionPortProbe.extractPort(from: "[::1]:5173") == 5173)
    #expect(SessionPortProbe.extractPort(from: "*:3000") == 3000)
}

// MARK: - File type icons

@Test func fileTypeIconsAreExtensionAware() {
    #expect(FileTypeIcon.systemImage(forFileName: "Main.swift") == "swift")
    #expect(FileTypeIcon.systemImage(forFileName: "app.ts") == "curlybraces")
    #expect(FileTypeIcon.systemImage(forFileName: "data.json") == "curlybraces.square")
    #expect(FileTypeIcon.systemImage(forFileName: "README.md") == "doc.plaintext")
    #expect(FileTypeIcon.systemImage(forFileName: "run.sh") == "terminal")
    #expect(FileTypeIcon.systemImage(forFileName: "src", isDirectory: true) == "folder")
}

// MARK: - Diff layout preference

@MainActor
@Test func diffLayoutStylePersistsThroughUserDefaults() {
    let key = SettingsKeys.diffLayoutStyle
    let previous = UserDefaults.standard.string(forKey: key)
    defer {
        if let previous {
            UserDefaults.standard.set(previous, forKey: key)
        } else {
            UserDefaults.standard.removeObject(forKey: key)
        }
    }

    UserDefaults.standard.set(DiffLayoutStyle.split.rawValue, forKey: key)
    let model = GitSidebarModel()
    #expect(model.diffLayoutStyle == .split)

    model.setDiffLayoutStyle(.unified)
    #expect(model.diffLayoutStyle == .unified)
    #expect(UserDefaults.standard.string(forKey: key) == DiffLayoutStyle.unified.rawValue)
}

@MainActor
@Test func primaryPresentExpandsImprovedDiffInSidebar() {
    let change = GitFileChange(
        path: "Sources/App.swift",
        originalPath: nil,
        indexStatus: ".",
        worktreeStatus: "M",
        isUntracked: false,
        isConflict: false,
        additions: 3,
        deletions: 1
    )
    let model = GitSidebarModel()
    model.present(change)
    #expect(model.expandedChangeID == change.id)
    #expect(model.selectedChangeID == change.id)
    #expect(!model.isDiffPresented)

    model.dismissDiff()
    #expect(model.expandedChangeID == nil)
    #expect(model.selectedChangeID == nil)

    model.toggleInline(change)
    #expect(model.expandedChangeID == change.id)
    model.toggleInline(change)
    #expect(model.expandedChangeID == nil)
}

// Shared git helper (same as GitClientTests)
private func runGit(_ arguments: [String], in directory: URL) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
    process.arguments = arguments
    process.currentDirectoryURL = directory
    process.standardOutput = FileHandle.nullDevice
    let stderr = Pipe()
    process.standardError = stderr
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        let output = stderr.fileHandleForReading.readDataToEndOfFile()
        throw NSError(
            domain: "SidebarDiffUpgradeTests",
            code: Int(process.terminationStatus),
            userInfo: [NSLocalizedDescriptionKey: String(decoding: output, as: UTF8.self)]
        )
    }
}
