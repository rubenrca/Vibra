import Foundation
import Testing
@testable import VibraApp

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
