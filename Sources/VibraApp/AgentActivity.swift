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
    case grok

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
        case .grok: "Grok"
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
        if executable == "grok" || executable == "xai-grok-pager" {
            return .grok
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
    let taskTitle: String?
}

struct TerminalProcess: Identifiable, Sendable {
    let pid: pid_t
    let name: String
    let depth: Int

    var id: pid_t { pid }
}

enum TerminalProcessTreeProbe {
    nonisolated static func processes(rootedAt foregroundPID: pid_t?) -> [TerminalProcess] {
        guard let foregroundPID else { return [] }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/ps")
        process.arguments = ["-axo", "pid=,ppid=,comm="]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return [] }

        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        let entries = String(decoding: data, as: UTF8.self).split(separator: "\n").compactMap {
            line -> ProcessEntry? in
            let fields = line.split(maxSplits: 2, whereSeparator: \Character.isWhitespace)
            guard fields.count == 3,
                  let pid = pid_t(fields[0]),
                  let parentPID = pid_t(fields[1])
            else { return nil }
            return ProcessEntry(pid: pid, parentPID: parentPID, command: String(fields[2]))
        }
        let entriesByPID = Dictionary(uniqueKeysWithValues: entries.map { ($0.pid, $0) })
        guard entriesByPID[foregroundPID] != nil else { return [] }

        let rootPID = shellAncestor(of: foregroundPID, entries: entriesByPID) ?? foregroundPID
        let children = Dictionary(grouping: entries, by: \.parentPID)
        var result: [TerminalProcess] = []
        appendProcessTree(
            rootPID,
            depth: 0,
            entries: entriesByPID,
            children: children,
            result: &result
        )
        return result
    }

    private struct ProcessEntry: Sendable {
        let pid: pid_t
        let parentPID: pid_t
        let command: String

        var name: String {
            URL(fileURLWithPath: command).lastPathComponent
        }
    }

    private nonisolated static func shellAncestor(
        of pid: pid_t,
        entries: [pid_t: ProcessEntry]
    ) -> pid_t? {
        var current = pid
        var visited: Set<pid_t> = []
        while let entry = entries[current], visited.insert(current).inserted {
            let executable = entry.name.lowercased()
            if ["sh", "bash", "zsh", "fish", "nu", "pwsh"].contains(executable) {
                return current
            }
            guard entry.parentPID > 1 else { break }
            current = entry.parentPID
        }
        return nil
    }

    private nonisolated static func appendProcessTree(
        _ pid: pid_t,
        depth: Int,
        entries: [pid_t: ProcessEntry],
        children: [pid_t: [ProcessEntry]],
        result: inout [TerminalProcess]
    ) {
        guard result.count < 16, let entry = entries[pid] else { return }
        result.append(TerminalProcess(pid: pid, name: entry.name, depth: depth))
        for child in (children[pid] ?? []).sorted(by: { $0.pid < $1.pid }) {
            appendProcessTree(
                child.pid,
                depth: depth + 1,
                entries: entries,
                children: children,
                result: &result
            )
        }
    }
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
                lifecycle: nil,
                taskTitle: nil
            )
        }

        let codexPIDs = Set(snapshots.compactMap { pid, snapshot in
            CodingAgent.detect(commandLine: snapshot.commandLine) == .codex ? pid : nil
        })
        let transcriptMetadata = CodexTranscriptProbe.metadata(for: codexPIDs)
        for (pid, metadata) in transcriptMetadata {
            guard let snapshot = snapshots[pid] else { continue }
            snapshots[pid] = ForegroundProcessSnapshot(
                commandLine: snapshot.commandLine,
                lifecycle: metadata.lifecycle,
                taskTitle: metadata.taskTitle
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
    private static let initialPromptByteLimit: UInt64 = 128 * 1024
    private static let pathRefreshInterval: TimeInterval = 12
    private static let cache = ProbeCache()

    struct Metadata: Sendable {
        let lifecycle: AgentLifecycleSnapshot?
        let taskTitle: String?
    }

    static func metadata(for pids: Set<pid_t>) -> [pid_t: Metadata] {
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
            let metadata = entry.value.compactMap(cachedMetadata(at:))
            let lifecycle = metadata.compactMap(\.lifecycle)
                .max { $0.observedAt < $1.observedAt }
            let taskTitle = metadata.compactMap(\.taskTitle).first
            if lifecycle != nil || taskTitle != nil {
                result[entry.key] = Metadata(lifecycle: lifecycle, taskTitle: taskTitle)
            }
        }
    }

    static func recentLifecycle(at url: URL) -> AgentLifecycleSnapshot? {
        transcriptUpdate(at: url, after: nil).lifecycle
    }

    static func cachedLifecycle(at url: URL) -> AgentLifecycleSnapshot? {
        cachedMetadata(at: url)?.lifecycle
    }

    static func cachedTaskTitle(at url: URL) -> String? {
        cachedMetadata(at: url)?.taskTitle
    }

    private static func cachedMetadata(at url: URL) -> Metadata? {
        let size = fileSize(at: url)
        let previous = cache.fileState(for: url)
        if previous?.size == size {
            return previous.map { Metadata(lifecycle: $0.lifecycle, taskTitle: $0.taskTitle) }
        }
        let update = transcriptUpdate(at: url, after: previous?.size)
        let initialTaskTitle = previous == nil ? firstTaskTitle(at: url) : nil
        let state = TranscriptFileState(
            size: update.size,
            lifecycle: update.lifecycle ?? previous?.lifecycle,
            taskTitle: previous?.taskTitle ?? initialTaskTitle ?? update.taskTitle
        )
        cache.store(fileState: state, for: url)
        return Metadata(lifecycle: state.lifecycle, taskTitle: state.taskTitle)
    }

    private static func transcriptUpdate(
        at url: URL,
        after previousSize: UInt64?
    ) -> (lifecycle: AgentLifecycleSnapshot?, taskTitle: String?, size: UInt64) {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return (nil, nil, 0) }
        defer { try? handle.close() }

        guard let end = try? handle.seekToEnd() else { return (nil, nil, 0) }
        let requestedStart = previousSize.flatMap { $0 <= end ? $0 : nil }
            ?? (end > recentByteLimit ? end - recentByteLimit : 0)
        let start = end - requestedStart > recentByteLimit
            ? end - recentByteLimit
            : requestedStart
        do {
            try handle.seek(toOffset: start)
            guard let data = try handle.readToEnd(), !data.isEmpty else {
                return (nil, nil, end)
            }
            var lines = data.split(separator: 0x0A, omittingEmptySubsequences: true)
            if start > 0, start != previousSize, !lines.isEmpty { lines.removeFirst() }

            let taskTitle = lines.lazy.compactMap { line -> String? in
                guard let event = try? JSONSerialization.jsonObject(with: Data(line))
                    as? [String: Any]
                else { return nil }
                return Self.taskTitle(in: event)
            }.first

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
                    taskTitle,
                    end
                )
            }
            return (nil, taskTitle, end)
        } catch {
            return (nil, nil, end)
        }
    }

    private static func taskTitle(in event: [String: Any]) -> String? {
        guard let payload = event["payload"] as? [String: Any] else { return nil }

        if event["type"] as? String == "event_msg",
           payload["type"] as? String == "user_message",
           let message = payload["message"] as? String {
            return TaskTitleFormatter.title(from: message)
        }

        guard event["type"] as? String == "response_item",
              payload["type"] as? String == "message",
              payload["role"] as? String == "user"
        else { return nil }

        if let message = payload["content"] as? String {
            return TaskTitleFormatter.title(from: message)
        }
        guard let content = payload["content"] as? [[String: Any]] else { return nil }
        let text = content.compactMap { item -> String? in
            guard let type = item["type"] as? String,
                  type == "input_text" || type == "text"
            else { return nil }
            return item["text"] as? String
        }.joined(separator: " ")
        return TaskTitleFormatter.title(from: text)
    }

    /// When Vibra attaches to a long-running Codex session, the recent tail
    /// may no longer include its opening prompt. Scan only the small leading
    /// portion once, preserving the original task rather than a later follow-up.
    private static func firstTaskTitle(at url: URL) -> String? {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
        defer { try? handle.close() }
        let count = Int(min(fileSize(at: url), initialPromptByteLimit))
        guard count > 0,
              let data = try? handle.read(upToCount: count),
              !data.isEmpty
        else { return nil }
        return data.split(separator: 0x0A, omittingEmptySubsequences: true).lazy
            .compactMap { line -> String? in
                guard let event = try? JSONSerialization.jsonObject(with: Data(line))
                    as? [String: Any]
                else { return nil }
                return Self.taskTitle(in: event)
            }
            .first
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
        let taskTitle: String?
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

private enum TaskTitleFormatter {
    private static let maximumLength = 58
    private static let greetings: Set<String> = [
        "hola", "hello", "hi", "hey", "buenas", "buen día", "buenos días",
        "good morning", "good afternoon", "good evening",
    ]
    private static let technicalContextPrefixes = [
        "<environment_context", "<permissions", "<collaboration_mode",
        "<apps_instructions", "<plugins_instructions", "<skills_instructions",
        "<multi_agent_mode",
    ]

    static func title(from rawPrompt: String) -> String? {
        let compact = rawPrompt
            .components(separatedBy: .whitespacesAndNewlines)
            .filter { !$0.isEmpty }
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !compact.isEmpty else { return nil }

        let normalized = compact.lowercased()
        guard !technicalContextPrefixes.contains(where: { normalized.hasPrefix($0) }) else {
            return nil
        }
        let greeting = normalized.trimmingCharacters(
            in: .whitespacesAndNewlines.union(.punctuationCharacters)
        )
        guard !greetings.contains(greeting) else { return nil }

        let characters = Array(compact)
        guard characters.count > maximumLength else { return compact }

        let prefix = String(characters.prefix(maximumLength))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return prefix + "…"
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
