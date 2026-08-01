import Foundation
import Testing
@testable import VibraApp

@Test func processWorkingDirectoryProbeReadsTheLiveDirectory() throws {
    let reported = try #require(
        ProcessWorkingDirectoryProbe.directory(
            for: pid_t(ProcessInfo.processInfo.processIdentifier)
        )
    )
    #expect(
        URL(fileURLWithPath: reported).standardizedFileURL
            == URL(fileURLWithPath: FileManager.default.currentDirectoryPath).standardizedFileURL
    )
}

@Test func gitClientLoadsChangesDiffsAndStagesFiles() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-git-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }

    try runGit(["init", "-q", "-b", "main"], in: root)
    try runGit(["config", "user.name", "Vibra Tests"], in: root)
    try runGit(["config", "user.email", "tests@vibra.local"], in: root)

    let tracked = root.appendingPathComponent("tracked.swift")
    try "let value = 1\n".write(to: tracked, atomically: true, encoding: .utf8)
    try runGit(["add", "tracked.swift"], in: root)
    try runGit(["commit", "-q", "-m", "Initial"], in: root)

    try "let value = 2\nlet next = 3\n".write(
        to: tracked,
        atomically: true,
        encoding: .utf8
    )
    try "new file\n".write(
        to: root.appendingPathComponent("untracked.txt"),
        atomically: true,
        encoding: .utf8
    )

    var snapshot = try GitClient.snapshot(from: root.path)
    #expect(snapshot.branch == "main")
    #expect(GitClient.branch(from: root.path) == "main")
    #expect(snapshot.changes.map(\.path) == ["tracked.swift", "untracked.txt"])

    let trackedChange = try #require(
        snapshot.changes.first { $0.path == "tracked.swift" }
    )
    #expect(trackedChange.additions == 2)
    #expect(trackedChange.deletions == 1)
    let diff = try GitClient.diff(for: trackedChange, in: snapshot.rootPath)
    #expect(diff.contains { $0.kind == .addition && $0.text == "+let next = 3" })
    #expect(diff.contains { $0.kind == .deletion && $0.text == "-let value = 1" })
    #expect(diff.contains {
        $0.kind == .addition && $0.text == "+let next = 3" && $0.newLine == 2
    })
    #expect(diff.contains {
        $0.kind == .deletion && $0.text == "-let value = 1" && $0.oldLine == 1
    })

    let untracked = try #require(
        snapshot.changes.first { $0.path == "untracked.txt" }
    )
    #expect(untracked.additions == 1)
    #expect(untracked.deletions == 0)
    try GitClient.stage(untracked, in: snapshot.rootPath)
    snapshot = try GitClient.snapshot(from: root.path)
    let staged = try #require(snapshot.changes.first { $0.path == "untracked.txt" })
    #expect(staged.hasStagedChanges)
    #expect(!staged.isUntracked)

    try GitClient.unstage(staged, in: snapshot.rootPath)
    snapshot = try GitClient.snapshot(from: root.path)
    #expect(snapshot.changes.first { $0.path == "untracked.txt" }?.isUntracked == true)
}

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
        throw TestGitError.commandFailed(String(decoding: output, as: UTF8.self))
    }
}

private enum TestGitError: Error {
    case commandFailed(String)
}

@Test func codingAgentDetectionRecognizesCommonNativeAndNodeLaunchers() {
    #expect(CodingAgent.detect(commandLine: "/opt/homebrew/bin/codex --model gpt-5") == .codex)
    #expect(CodingAgent.detect(
        commandLine: "/usr/bin/node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js"
    ) == .claude)
    #expect(CodingAgent.detect(commandLine: "/Users/dev/.local/bin/opencode run") == .opencode)
    #expect(CodingAgent.detect(commandLine: "/bin/zsh -l") == nil)
    #expect(CodingAgent.detect(commandLine: "", title: "Codex") == .codex)
}

@Test func ghosttyConfigLocatorPrefersXDGThenStandardLocations() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-config-tests-\(UUID().uuidString)")
    let home = root.appendingPathComponent("home", isDirectory: true)
    let xdg = root.appendingPathComponent("xdg", isDirectory: true)
    let xdgConfig = xdg.appendingPathComponent("ghostty/config")
    let standardConfig = home.appendingPathComponent(".config/ghostty/config")
    try FileManager.default.createDirectory(
        at: xdgConfig.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    try FileManager.default.createDirectory(
        at: standardConfig.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    defer { try? FileManager.default.removeItem(at: root) }
    try "font-family = Mono\n".write(to: xdgConfig, atomically: true, encoding: .utf8)
    try "font-family = Other\n".write(to: standardConfig, atomically: true, encoding: .utf8)

    #expect(GhosttyConfigLocator.path(
        environment: ["XDG_CONFIG_HOME": xdg.path],
        homeDirectory: home
    ) == xdgConfig.path)
    #expect(GhosttyConfigLocator.path(environment: [:], homeDirectory: home) == standardConfig.path)
}

@Test func codexStatusIntegrationPreservesExistingHooksAndWritesLifecycleEvents() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-hooks-tests-\(UUID().uuidString)")
    let hooksURL = root.appendingPathComponent(".codex/hooks.json")
    try FileManager.default.createDirectory(
        at: hooksURL.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    defer { try? FileManager.default.removeItem(at: root) }

    let existing: [String: Any] = [
        "hooks": [
            "Stop": [[
                "hooks": [["type": "command", "command": "echo existing"]],
            ]],
        ],
    ]
    let existingData = try JSONSerialization.data(withJSONObject: existing)
    try existingData.write(to: hooksURL)

    try CodexStatusIntegration.install(at: hooksURL)
    #expect(CodexStatusIntegration.isInstalled(at: hooksURL))

    let data = try Data(contentsOf: hooksURL)
    let document = try #require(
        try JSONSerialization.jsonObject(with: data) as? [String: Any]
    )
    let hooks = try #require(document["hooks"] as? [String: Any])
    let stopGroups = try #require(hooks["Stop"] as? [[String: Any]])
    #expect(stopGroups.count == 2)
    #expect(["SessionStart", "UserPromptSubmit", "PermissionRequest", "Stop", "SessionEnd"]
        .allSatisfy { hooks[$0] != nil })

    let promptGroups = try #require(hooks["UserPromptSubmit"] as? [[String: Any]])
    let promptHandlers = try #require(promptGroups.last?["hooks"] as? [[String: Any]])
    let command = try #require(promptHandlers.first?["command"] as? String)
    let sessionID = UUID()
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sh")
    process.arguments = ["-c", command]
    var environment = ProcessInfo.processInfo.environment
    environment["HOME"] = root.path
    environment["VIBRA_SESSION_ID"] = sessionID.uuidString
    process.environment = environment
    try process.run()
    process.waitUntilExit()
    #expect(process.terminationStatus == 0)
    let statusURL = root
        .appendingPathComponent("Library/Application Support/Vibra/agent-status")
        .appendingPathComponent("\(sessionID.uuidString).status")
    #expect(try String(contentsOf: statusURL, encoding: .utf8)
        .trimmingCharacters(in: .whitespacesAndNewlines) == "working")
}

@Test func codexTranscriptProbeDetectsTheLatestTurnLifecycleEvent() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-codex-transcript-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let transcript = root.appendingPathComponent("rollout.jsonl")

    let events = [
        """
        {"timestamp":"2026-07-25T15:42:56.764Z","type":"event_msg","payload":{"type":"task_started"}}
        """,
        """
        {"timestamp":"2026-07-25T15:43:01.101Z","type":"response_item","payload":{"type":"message"}}
        """,
        """
        {"timestamp":"2026-07-25T15:43:05.252Z","type":"event_msg","payload":{"type":"task_complete"}}
        """,
    ].joined(separator: "\n")
    try events.write(to: transcript, atomically: true, encoding: .utf8)

    let lifecycle = try #require(CodexTranscriptProbe.recentLifecycle(at: transcript))
    #expect(lifecycle.state == .finished)
    #expect(lifecycle.observedAt.timeIntervalSince1970 > 0)
}

@Test func codexTranscriptProbeReadsOnlyAppendedLifecycleEvents() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-codex-cache-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let transcript = root.appendingPathComponent("rollout.jsonl")
    try """
    {"timestamp":"2026-07-25T15:43:05.252Z","type":"event_msg","payload":{"type":"task_complete"}}

    """.write(to: transcript, atomically: true, encoding: .utf8)

    #expect(CodexTranscriptProbe.cachedLifecycle(at: transcript)?.state == .finished)

    let handle = try FileHandle(forWritingTo: transcript)
    defer { try? handle.close() }
    try handle.seekToEnd()
    try handle.write(contentsOf: Data("""
    {"timestamp":"2026-07-25T15:44:00.000Z","type":"event_msg","payload":{"type":"task_started"}}

    """.utf8))
    try handle.synchronize()

    #expect(CodexTranscriptProbe.cachedLifecycle(at: transcript)?.state == .working)
}

@Test func codexTranscriptProbeUsesTheFirstPromptAsTheTaskTitle() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-codex-task-title-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let transcript = root.appendingPathComponent("rollout.jsonl")

    try [
        """
        {"timestamp":"2026-08-01T11:18:00.000Z","type":"event_msg","payload":{"type":"user_message","message":"Implement automatic task names for workspace tabs."}}
        """,
        """
        {"timestamp":"2026-08-01T11:18:01.000Z","type":"event_msg","payload":{"type":"task_started"}}
        """,
        """
        {"timestamp":"2026-08-01T11:19:01.000Z","type":"event_msg","payload":{"type":"user_message","message":"Also improve the tooltip."}}
        """,
    ].joined(separator: "\n").write(to: transcript, atomically: true, encoding: .utf8)

    #expect(CodexTranscriptProbe.cachedTaskTitle(at: transcript)
        == "Implement automatic task names for workspace tabs.")
}

@Test func codexTranscriptProbeIgnoresContextBlocksAndGreetingPrompts() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-codex-task-filter-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let transcript = root.appendingPathComponent("rollout.jsonl")

    try [
        """
        {"timestamp":"2026-08-01T16:38:20.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context> internal runtime data"}]}}
        """,
        """
        {"timestamp":"2026-08-01T16:38:22.000Z","type":"event_msg","payload":{"type":"user_message","message":"hola"}}
        """,
        """
        {"timestamp":"2026-08-01T16:38:25.000Z","type":"event_msg","payload":{"type":"user_message","message":"revisa gh"}}
        """,
    ].joined(separator: "\n").write(to: transcript, atomically: true, encoding: .utf8)

    #expect(CodexTranscriptProbe.cachedTaskTitle(at: transcript) == "revisa gh")
}

@MainActor
@Test func inlineDiffAndModalPresentationRemainIndependent() {
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

    model.toggleInline(change)
    #expect(model.expandedChangeID == change.id)
    #expect(model.selectedChangeID == change.id)
    #expect(!model.isDiffPresented)

    model.presentModal(change)
    #expect(model.expandedChangeID == change.id)
    #expect(model.isDiffPresented)
    model.dismissDiff()
    #expect(model.expandedChangeID == change.id)
    #expect(model.selectedChangeID == change.id)

    model.toggleInline(change)
    #expect(model.expandedChangeID == nil)
    #expect(model.selectedChangeID == nil)

    model.presentModal(change)
    #expect(model.expandedChangeID == nil)
    model.dismissDiff()
    #expect(model.selectedChangeID == nil)
}

@Test func gitChangesBuildFolderTreeWithAggregatedStats() throws {
    func change(_ path: String, additions: Int, deletions: Int) -> GitFileChange {
        GitFileChange(
            path: path,
            originalPath: nil,
            indexStatus: ".",
            worktreeStatus: "M",
            isUntracked: false,
            isConflict: false,
            additions: additions,
            deletions: deletions
        )
    }

    let tree = GitChangeTreeNode.makeTree(from: [
        change("README.md", additions: 1, deletions: 0),
        change("Sources/App/Main.swift", additions: 5, deletions: 2),
        change("Sources/Core/Model.swift", additions: 3, deletions: 1),
    ])

    #expect(tree.map(\.name) == ["Sources", "README.md"])
    let sources = try #require(tree.first)
    #expect(sources.additions == 8)
    #expect(sources.deletions == 3)
    #expect(sources.children.map(\.name) == ["App", "Core"])
    #expect(sources.children[0].children.first?.name == "Main.swift")
}
