import Foundation

struct GitFileChange: Identifiable, Equatable, Sendable {
    var id: String { path }

    let path: String
    let originalPath: String?
    let indexStatus: Character
    let worktreeStatus: Character
    let isUntracked: Bool
    let isConflict: Bool
    let additions: Int
    let deletions: Int

    var fileName: String {
        (path as NSString).lastPathComponent
    }

    var directory: String {
        let directory = (path as NSString).deletingLastPathComponent
        return directory == "." ? "" : directory
    }

    var hasStagedChanges: Bool {
        !isUntracked && indexStatus != "." && indexStatus != " "
    }

    var hasWorktreeChanges: Bool {
        isUntracked || worktreeStatus != "." && worktreeStatus != " "
    }

    var compactStatus: String {
        if isConflict { return "U" }
        if isUntracked { return "?" }
        if hasStagedChanges && hasWorktreeChanges { return "M²" }
        if hasStagedChanges { return String(indexStatus) }
        return String(worktreeStatus)
    }
}

struct GitRepositorySnapshot: Equatable, Sendable {
    let rootPath: String
    let branch: String
    let changes: [GitFileChange]
}

enum DiffLineKind: Equatable, Sendable {
    case metadata
    case hunk
    case addition
    case deletion
    case context
}

struct DiffLine: Identifiable, Equatable, Sendable {
    let id: Int
    let text: String
    let kind: DiffLineKind
    let oldLine: Int?
    let newLine: Int?
}

enum GitClient {
    private static let statusOutputLimit = 16 * 1024 * 1024
    private static let diffOutputLimit = 3 * 1024 * 1024

    nonisolated static func branch(from path: String) -> String? {
        let result = run(["branch", "--show-current"], in: path)
        guard result.status == 0 else { return nil }
        let branch = result.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        return branch.isEmpty ? nil : branch
    }

    /// Returns the repository top-level for `path`, or nil when not inside a git repo.
    nonisolated static func repositoryTopLevel(from path: String) -> String? {
        let topLevel = run(["rev-parse", "--show-toplevel"], in: path)
        guard topLevel.status == 0 else { return nil }
        let root = topLevel.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        return root.isEmpty ? nil : root
    }

    /// Parses unified diff text into line models (testable without invoking git).
    nonisolated static func parseDiffText(_ text: String) -> [DiffLine] {
        makeDiffLines(text)
    }

    nonisolated static func snapshot(from path: String) throws -> GitRepositorySnapshot {
        guard let root = repositoryTopLevel(from: path) else {
            throw GitClientError.notRepository
        }

        let status = run(
            ["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"],
            in: root,
            retainedOutputLimit: statusOutputLimit
        )
        guard status.status == 0 else {
            throw GitClientError.commandFailed(status.stderr)
        }
        guard !status.wasTruncated else {
            throw GitClientError.commandFailed(
                "The repository has too many changes to display safely."
            )
        }

        var snapshot = parseStatus(status.stdoutData, root: root)
        let worktreeStats = numstat(
            run(["diff", "--numstat", "-z", "--no-ext-diff"], in: root).stdoutData
        )
        let stagedStats = numstat(
            run(["diff", "--cached", "--numstat", "-z", "--no-ext-diff"], in: root).stdoutData
        )
        snapshot = GitRepositorySnapshot(
            rootPath: snapshot.rootPath,
            branch: snapshot.branch,
            changes: snapshot.changes.map { change in
                let unstaged = worktreeStats[change.path] ?? (0, 0)
                let staged = stagedStats[change.path] ?? (0, 0)
                let untracked = change.isUntracked
                    ? untrackedStats(path: change.path, root: root)
                    : (0, 0)
                return GitFileChange(
                    path: change.path,
                    originalPath: change.originalPath,
                    indexStatus: change.indexStatus,
                    worktreeStatus: change.worktreeStatus,
                    isUntracked: change.isUntracked,
                    isConflict: change.isConflict,
                    additions: unstaged.0 + staged.0 + untracked.0,
                    deletions: unstaged.1 + staged.1 + untracked.1
                )
            }
        )
        return snapshot
    }

    nonisolated static func diff(
        for change: GitFileChange,
        in root: String
    ) throws -> [DiffLine] {
        let args: [String]
        if change.isUntracked {
            let absolutePath = (root as NSString).appendingPathComponent(change.path)
            args = ["diff", "--no-index", "--unified=3", "--", "/dev/null", absolutePath]
        } else if change.hasStagedChanges && change.hasWorktreeChanges {
            let staged = run(
                ["--literal-pathspecs", "diff", "--cached", "--no-ext-diff", "--unified=3", "--", change.path],
                in: root,
                retainedOutputLimit: diffOutputLimit
            )
            let worktree = run(
                ["--literal-pathspecs", "diff", "--no-ext-diff", "--unified=3", "--", change.path],
                in: root,
                retainedOutputLimit: diffOutputLimit
            )
            guard staged.status == 0, worktree.status == 0 else {
                throw GitClientError.commandFailed(staged.stderr + worktree.stderr)
            }
            guard !staged.wasTruncated, !worktree.wasTruncated else {
                throw GitClientError.diffTooLarge
            }
            return makeDiffLines(staged.stdout + "\n" + worktree.stdout)
        } else if change.hasWorktreeChanges {
            args = ["--literal-pathspecs", "diff", "--no-ext-diff", "--unified=3", "--", change.path]
        } else {
            args = ["--literal-pathspecs", "diff", "--cached", "--no-ext-diff", "--unified=3", "--", change.path]
        }

        let result = run(args, in: root, retainedOutputLimit: diffOutputLimit)
        let acceptedStatus = result.status == 0 || change.isUntracked && result.status == 1
        guard acceptedStatus else {
            throw GitClientError.commandFailed(result.stderr)
        }
        guard !result.wasTruncated else {
            throw GitClientError.diffTooLarge
        }
        return makeDiffLines(result.stdout)
    }

    nonisolated static func stage(_ change: GitFileChange, in root: String) throws {
        try requireSuccess(
            run(["--literal-pathspecs", "add", "--", change.path], in: root)
        )
    }

    nonisolated static func unstage(_ change: GitFileChange, in root: String) throws {
        let hasHead = run(["rev-parse", "--verify", "HEAD"], in: root).status == 0
        let args = hasHead
            ? ["--literal-pathspecs", "restore", "--staged", "--", change.path]
            : ["--literal-pathspecs", "rm", "--cached", "-q", "--", change.path]
        try requireSuccess(run(args, in: root))
    }

    nonisolated static func stageAll(in root: String) throws {
        try requireSuccess(run(["add", "-A"], in: root))
    }

    nonisolated static func unstageAll(in root: String) throws {
        let hasHead = run(["rev-parse", "--verify", "HEAD"], in: root).status == 0
        let args = hasHead
            ? ["restore", "--staged", "."]
            : ["rm", "--cached", "-r", "-q", "."]
        try requireSuccess(run(args, in: root))
    }

    private nonisolated static func requireSuccess(_ output: GitCommandOutput) throws {
        guard output.status == 0 else {
            throw GitClientError.commandFailed(output.stderr)
        }
    }

    private nonisolated static func parseStatus(
        _ data: Data,
        root: String
    ) -> GitRepositorySnapshot {
        let records = data.split(separator: 0, omittingEmptySubsequences: true)
            .map { String(decoding: $0, as: UTF8.self) }
        var branch = "HEAD"
        var changes: [GitFileChange] = []
        var index = 0

        while index < records.count {
            let record = records[index]
            defer { index += 1 }

            if record.hasPrefix("# branch.head ") {
                let value = String(record.dropFirst("# branch.head ".count))
                branch = value == "(detached)" ? "Detached HEAD" : value
                continue
            }

            if record.hasPrefix("? ") {
                changes.append(
                    GitFileChange(
                        path: String(record.dropFirst(2)),
                        originalPath: nil,
                        indexStatus: "?",
                        worktreeStatus: "?",
                        isUntracked: true,
                        isConflict: false,
                        additions: 0,
                        deletions: 0
                    )
                )
                continue
            }

            if record.hasPrefix("1 ") {
                let fields = record.split(separator: " ", maxSplits: 8)
                guard fields.count == 9 else { continue }
                changes.append(
                    makeChange(path: String(fields[8]), xy: fields[1], originalPath: nil)
                )
                continue
            }

            if record.hasPrefix("2 ") {
                let fields = record.split(separator: " ", maxSplits: 9)
                guard fields.count == 10 else { continue }
                let originalPath = index + 1 < records.count ? records[index + 1] : nil
                if originalPath != nil { index += 1 }
                changes.append(
                    makeChange(
                        path: String(fields[9]),
                        xy: fields[1],
                        originalPath: originalPath
                    )
                )
                continue
            }

            if record.hasPrefix("u ") {
                let fields = record.split(separator: " ", maxSplits: 10)
                guard fields.count == 11 else { continue }
                changes.append(
                    makeChange(
                        path: String(fields[10]),
                        xy: fields[1],
                        originalPath: nil,
                        conflict: true
                    )
                )
            }
        }

        changes.sort {
            $0.path.localizedStandardCompare($1.path) == .orderedAscending
        }
        return GitRepositorySnapshot(rootPath: root, branch: branch, changes: changes)
    }

    private nonisolated static func makeChange(
        path: String,
        xy: Substring,
        originalPath: String?,
        conflict: Bool = false
    ) -> GitFileChange {
        let statuses = Array(xy)
        return GitFileChange(
            path: path,
            originalPath: originalPath,
            indexStatus: statuses.first ?? ".",
            worktreeStatus: statuses.dropFirst().first ?? ".",
            isUntracked: false,
            isConflict: conflict,
            additions: 0,
            deletions: 0
        )
    }

    private nonisolated static func makeDiffLines(_ text: String) -> [DiffLine] {
        var oldLine: Int?
        var newLine: Int?
        return text.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .map { offset, substring in
                let line = String(substring)
                let kind: DiffLineKind
                if line.hasPrefix("@@") {
                    kind = .hunk
                    let starts = hunkStarts(line)
                    oldLine = starts.old
                    newLine = starts.new
                } else if line.hasPrefix("+++") || line.hasPrefix("---")
                            || line.hasPrefix("diff ") || line.hasPrefix("index ") {
                    kind = .metadata
                } else if line.hasPrefix("+") {
                    kind = .addition
                } else if line.hasPrefix("-") {
                    kind = .deletion
                } else {
                    kind = .context
                }
                let displayedOld: Int?
                let displayedNew: Int?
                switch kind {
                case .addition:
                    displayedOld = nil
                    displayedNew = newLine
                    newLine = newLine.map { $0 + 1 }
                case .deletion:
                    displayedOld = oldLine
                    displayedNew = nil
                    oldLine = oldLine.map { $0 + 1 }
                case .context where !line.hasPrefix("\\"):
                    displayedOld = oldLine
                    displayedNew = newLine
                    oldLine = oldLine.map { $0 + 1 }
                    newLine = newLine.map { $0 + 1 }
                default:
                    displayedOld = nil
                    displayedNew = nil
                }
                return DiffLine(
                    id: offset,
                    text: line,
                    kind: kind,
                    oldLine: displayedOld,
                    newLine: displayedNew
                )
            }
    }

    private nonisolated static func hunkStarts(_ line: String) -> (old: Int?, new: Int?) {
        let fields = line.split(separator: " ")
        guard fields.count >= 3 else { return (nil, nil) }
        func start(_ field: Substring) -> Int? {
            Int(field.dropFirst().split(separator: ",", maxSplits: 1).first ?? "")
        }
        return (start(fields[1]), start(fields[2]))
    }

    private nonisolated static func numstat(_ data: Data) -> [String: (Int, Int)] {
        let records = data.split(separator: 0, omittingEmptySubsequences: false)
            .map { String(decoding: $0, as: UTF8.self) }
        var result: [String: (Int, Int)] = [:]
        var index = 0
        while index < records.count {
            let fields = records[index].split(
                separator: "\t",
                maxSplits: 2,
                omittingEmptySubsequences: false
            )
            guard fields.count == 3 else {
                index += 1
                continue
            }
            let additions = Int(fields[0]) ?? 0
            let deletions = Int(fields[1]) ?? 0
            var path = String(fields[2])
            if path.isEmpty, index + 2 < records.count {
                path = records[index + 2]
                index += 2
            }
            if !path.isEmpty {
                let current = result[path] ?? (0, 0)
                result[path] = (current.0 + additions, current.1 + deletions)
            }
            index += 1
        }
        return result
    }

    private nonisolated static func untrackedStats(
        path: String,
        root: String
    ) -> (Int, Int) {
        let url = URL(fileURLWithPath: root).appendingPathComponent(path)
        guard let values = try? url.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey]),
              values.isRegularFile == true,
              (values.fileSize ?? 0) <= 2 * 1024 * 1024,
              let data = try? Data(contentsOf: url),
              !data.contains(0)
        else { return (0, 0) }
        if data.isEmpty { return (0, 0) }
        let newlines = data.reduce(into: 0) { count, byte in
            if byte == 10 { count += 1 }
        }
        return (newlines + (data.last == 10 ? 0 : 1), 0)
    }

    private nonisolated static func run(
        _ arguments: [String],
        in directory: String,
        retainedOutputLimit: Int = 2 * 1024 * 1024
    ) -> GitCommandOutput {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        process.arguments = arguments
        process.currentDirectoryURL = URL(fileURLWithPath: directory, isDirectory: true)
        var environment = ProcessInfo.processInfo.environment
        environment["GIT_OPTIONAL_LOCKS"] = "0"
        environment["GIT_TERMINAL_PROMPT"] = "0"
        environment["LC_ALL"] = "C"
        process.environment = environment

        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        process.standardInput = FileHandle.nullDevice

        do {
            try process.run()
        } catch {
            return GitCommandOutput(
                status: -1,
                stdoutData: Data(),
                stderr: error.localizedDescription,
                wasTruncated: false
            )
        }

        let output = BoundedPipeCapture(limit: retainedOutputLimit)
        let errors = BoundedPipeCapture(limit: 256 * 1024)
        let readers = DispatchGroup()
        readers.enter()
        DispatchQueue.global(qos: .utility).async {
            output.read(from: stdout.fileHandleForReading)
            readers.leave()
        }
        readers.enter()
        DispatchQueue.global(qos: .utility).async {
            errors.read(from: stderr.fileHandleForReading)
            readers.leave()
        }

        process.waitUntilExit()
        readers.wait()
        return GitCommandOutput(
            status: process.terminationStatus,
            stdoutData: output.data,
            stderr: String(decoding: errors.data, as: UTF8.self),
            wasTruncated: output.wasTruncated
        )
    }
}

private struct GitCommandOutput: Sendable {
    let status: Int32
    let stdoutData: Data
    let stderr: String
    let wasTruncated: Bool

    var stdout: String {
        String(decoding: stdoutData, as: UTF8.self)
    }
}

private final class BoundedPipeCapture: @unchecked Sendable {
    private let limit: Int
    private(set) var data = Data()
    private(set) var wasTruncated = false

    init(limit: Int) {
        self.limit = limit
    }

    func read(from handle: FileHandle) {
        while true {
            let chunk: Data
            do {
                guard let next = try handle.read(upToCount: 64 * 1024), !next.isEmpty else {
                    return
                }
                chunk = next
            } catch {
                return
            }

            let remaining = limit - data.count
            if remaining > 0 {
                data.append(chunk.prefix(remaining))
            }
            if chunk.count > remaining {
                wasTruncated = true
            }
        }
    }
}

enum GitClientError: LocalizedError {
    case notRepository
    case commandFailed(String)
    case diffTooLarge

    var errorDescription: String? {
        switch self {
        case .notRepository:
            "The project is not inside a Git repository."
        case .commandFailed(let output):
            output.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ? "Git command failed."
                : output.trimmingCharacters(in: .whitespacesAndNewlines)
        case .diffTooLarge:
            "This diff is larger than Vibra's 3 MB preview limit."
        }
    }
}
