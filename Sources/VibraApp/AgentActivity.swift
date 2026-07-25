import Foundation

enum CodingAgent: String, CaseIterable, Sendable {
    case codex
    case claude
    case gemini
    case opencode
    case aider
    case goose
    case amp
    case cursor

    var displayName: String {
        switch self {
        case .codex: "Codex"
        case .claude: "Claude"
        case .gemini: "Gemini"
        case .opencode: "OpenCode"
        case .aider: "Aider"
        case .goose: "Goose"
        case .amp: "Amp"
        case .cursor: "Cursor Agent"
        }
    }

    static func detect(commandLine: String, title: String = "") -> CodingAgent? {
        let command = commandLine.lowercased()
        let executable = command
            .split(whereSeparator: \Character.isWhitespace)
            .first
            .map(String.init)?
            .split(separator: "/")
            .last
            .map(String.init) ?? ""

        if executable == "codex"
            || executable.hasPrefix("codex-")
            || command.contains("/@openai/codex")
            || command.contains("/codex/bin/codex") {
            return .codex
        }
        if executable == "claude"
            || command.contains("/@anthropic-ai/claude-code/")
            || command.contains("/claude-code/cli.js") {
            return .claude
        }
        if executable == "gemini" || command.contains("/@google/gemini-cli/") {
            return .gemini
        }
        if executable == "opencode" || executable.hasPrefix("opencode-") {
            return .opencode
        }
        if executable == "aider" || executable.hasPrefix("aider-") {
            return .aider
        }
        if executable == "goose" || executable.hasPrefix("goose-") {
            return .goose
        }
        if executable == "amp" || command.contains("/@sourcegraph/amp") {
            return .amp
        }
        if executable == "cursor-agent" || command.contains("/cursor-agent") {
            return .cursor
        }

        let normalizedTitle = title.lowercased()
        return allCases.first { agent in
            switch agent {
            case .opencode: normalizedTitle.contains("opencode")
            case .cursor: normalizedTitle.contains("cursor agent")
            default: normalizedTitle.contains(agent.rawValue)
            }
        }
    }
}

enum AgentActivity: Equatable, Sendable {
    case idle
    case ready(agent: CodingAgent)
    case running(agent: CodingAgent, since: Date)
    case needsAttention(agent: CodingAgent, message: String?)
    case finished(agent: CodingAgent, succeeded: Bool?, at: Date)

    var agent: CodingAgent? {
        switch self {
        case .idle: nil
        case .ready(let agent): agent
        case .running(let agent, _),
             .needsAttention(let agent, _),
             .finished(let agent, _, _): agent
        }
    }
}

struct ForegroundProcessSnapshot: Sendable {
    let commandLine: String
    let lifecycle: AgentLifecycleSnapshot?
}

enum AgentProcessProbe {
    static func snapshots(for pids: Set<pid_t>) -> [pid_t: ForegroundProcessSnapshot] {
        guard !pids.isEmpty else { return [:] }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/ps")
        process.arguments = [
            "-ww",
            "-p", pids.sorted().map(String.init).joined(separator: ","),
            "-o", "pid=,command=",
        ]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice

        do {
            try process.run()
        } catch {
            return [:]
        }

        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        let text = String(decoding: data, as: UTF8.self)
        var snapshots = text.split(separator: "\n").reduce(
            into: [pid_t: ForegroundProcessSnapshot]()
        ) { result, line in
            let fields = line.split(maxSplits: 1, whereSeparator: \Character.isWhitespace)
            guard fields.count == 2, let pid = pid_t(fields[0]) else { return }
            result[pid] = ForegroundProcessSnapshot(
                commandLine: String(fields[1]),
                lifecycle: nil
            )
        }

        let codexPIDs = Set(snapshots.compactMap { pid, snapshot in
            CodingAgent.detect(commandLine: snapshot.commandLine) == .codex ? pid : nil
        })
        let transcriptLifecycle = CodexTranscriptProbe.lifecycle(for: codexPIDs)
        for (pid, lifecycle) in transcriptLifecycle {
            guard let snapshot = snapshots[pid] else { continue }
            snapshots[pid] = ForegroundProcessSnapshot(
                commandLine: snapshot.commandLine,
                lifecycle: lifecycle
            )
        }
        return snapshots
    }
}

/// Codex keeps the active rollout transcript open for the lifetime of its TUI.
/// Its terminal process remains foregrounded between turns, so the most recent
/// task_started/task_complete event is a useful read-only fallback when hooks
/// have not been installed. Hooks still take precedence when they are newer.
enum CodexTranscriptProbe {
    private static let recentByteLimit: UInt64 = 512 * 1024
    private static let pathRefreshInterval: TimeInterval = 12
    private static let cache = ProbeCache()

    static func lifecycle(for pids: Set<pid_t>) -> [pid_t: AgentLifecycleSnapshot] {
        guard !pids.isEmpty else { return [:] }
        let paths: [pid_t: [URL]]
        if let cached = cache.paths(for: pids, maxAge: pathRefreshInterval) {
            paths = cached
        } else {
            let discovered = rolloutPaths(for: pids)
            cache.store(paths: discovered, for: pids)
            paths = discovered
        }
        return paths.reduce(into: [:]) { result, entry in
            let lifecycle = entry.value.compactMap(cachedLifecycle(at:))
                .max { $0.observedAt < $1.observedAt }
            if let lifecycle { result[entry.key] = lifecycle }
        }
    }

    static func recentLifecycle(at url: URL) -> AgentLifecycleSnapshot? {
        lifecycleUpdate(at: url, after: nil).lifecycle
    }

    static func cachedLifecycle(at url: URL) -> AgentLifecycleSnapshot? {
        let size = fileSize(at: url)
        let previous = cache.fileState(for: url)
        if previous?.size == size {
            return previous?.lifecycle
        }
        let update = lifecycleUpdate(at: url, after: previous?.size)
        let state = TranscriptFileState(
            size: update.size,
            lifecycle: update.lifecycle ?? previous?.lifecycle
        )
        cache.store(fileState: state, for: url)
        return state.lifecycle
    }

    private static func lifecycleUpdate(
        at url: URL,
        after previousSize: UInt64?
    ) -> (lifecycle: AgentLifecycleSnapshot?, size: UInt64) {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return (nil, 0) }
        defer { try? handle.close() }

        guard let end = try? handle.seekToEnd() else { return (nil, 0) }
        let requestedStart = previousSize.flatMap { $0 <= end ? $0 : nil }
            ?? (end > recentByteLimit ? end - recentByteLimit : 0)
        let start = end - requestedStart > recentByteLimit
            ? end - recentByteLimit
            : requestedStart
        do {
            try handle.seek(toOffset: start)
            guard let data = try handle.readToEnd(), !data.isEmpty else { return (nil, end) }
            var lines = data.split(separator: 0x0A, omittingEmptySubsequences: true)
            if start > 0, start != previousSize, !lines.isEmpty { lines.removeFirst() }

            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            for line in lines.reversed() {
                guard let object = try? JSONSerialization.jsonObject(with: Data(line)),
                      let event = object as? [String: Any],
                      event["type"] as? String == "event_msg",
                      let payload = event["payload"] as? [String: Any],
                      let eventType = payload["type"] as? String,
                      eventType == "task_started" || eventType == "task_complete",
                      let timestamp = event["timestamp"] as? String,
                      let date = formatter.date(from: timestamp)
                else { continue }
                return (
                    AgentLifecycleSnapshot(
                        state: eventType == "task_started" ? .working : .finished,
                        observedAt: date
                    ),
                    end
                )
            }
        } catch {
            return (nil, end)
        }
        return (nil, end)
    }

    private static func fileSize(at url: URL) -> UInt64 {
        let attributes = try? FileManager.default.attributesOfItem(atPath: url.path)
        return (attributes?[.size] as? NSNumber)?.uint64Value ?? 0
    }

    private static func rolloutPaths(for pids: Set<pid_t>) -> [pid_t: [URL]] {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
        process.arguments = [
            "-a",
            "-p", pids.sorted().map(String.init).joined(separator: ","),
            "-Fn",
        ]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice

        do {
            try process.run()
        } catch {
            return [:]
        }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()

        var currentPID: pid_t?
        var result: [pid_t: [URL]] = [:]
        for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
            guard let prefix = line.first else { continue }
            let value = String(line.dropFirst())
            if prefix == "p" {
                currentPID = pid_t(value)
            } else if prefix == "n",
                      let currentPID,
                      value.contains("/.codex/sessions/"),
                      value.hasSuffix(".jsonl") {
                result[currentPID, default: []].append(URL(fileURLWithPath: value))
            }
        }
        return result
    }

    private struct TranscriptFileState {
        let size: UInt64
        let lifecycle: AgentLifecycleSnapshot?
    }

    private final class ProbeCache: @unchecked Sendable {
        private let lock = NSLock()
        private var discoveredPIDs: Set<pid_t> = []
        private var pathsByPID: [pid_t: [URL]] = [:]
        private var pathsUpdatedAt = Date.distantPast
        private var files: [URL: TranscriptFileState] = [:]

        func paths(for pids: Set<pid_t>, maxAge: TimeInterval) -> [pid_t: [URL]]? {
            lock.lock()
            defer { lock.unlock() }
            guard pids == discoveredPIDs,
                  Date().timeIntervalSince(pathsUpdatedAt) < maxAge else { return nil }
            return pathsByPID
        }

        func store(paths: [pid_t: [URL]], for pids: Set<pid_t>) {
            lock.lock()
            discoveredPIDs = pids
            pathsByPID = paths
            pathsUpdatedAt = Date()
            let activePaths = Set(paths.values.flatMap { $0 })
            files = files.filter { activePaths.contains($0.key) }
            lock.unlock()
        }

        func fileState(for url: URL) -> TranscriptFileState? {
            lock.lock()
            defer { lock.unlock() }
            return files[url]
        }

        func store(fileState: TranscriptFileState, for url: URL) {
            lock.lock()
            files[url] = fileState
            lock.unlock()
        }
    }
}

enum GhosttyConfigLocator {
    static func path(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser,
        fileManager: FileManager = .default
    ) -> String? {
        var candidates: [URL] = []
        if let xdgConfigHome = environment["XDG_CONFIG_HOME"], !xdgConfigHome.isEmpty {
            candidates.append(
                URL(fileURLWithPath: xdgConfigHome, isDirectory: true)
                    .appendingPathComponent("ghostty/config")
            )
        }
        candidates.append(homeDirectory.appendingPathComponent(".config/ghostty/config"))
        candidates.append(
            homeDirectory.appendingPathComponent(
                "Library/Application Support/com.mitchellh.ghostty/config"
            )
        )
        return candidates.first { fileManager.fileExists(atPath: $0.path) }?.path
    }
}
