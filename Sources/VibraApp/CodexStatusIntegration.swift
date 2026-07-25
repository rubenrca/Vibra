import Foundation

enum AgentLifecycleState: String, Sendable {
    case ready
    case working
    case needsAttention = "needs-attention"
    case finished
    case inactive
}

struct AgentLifecycleSnapshot: Sendable {
    let state: AgentLifecycleState
    let observedAt: Date
}

enum AgentLifecycleStore {
    static func state(for sessionID: UUID) -> AgentLifecycleState? {
        snapshot(for: sessionID)?.state
    }

    static func snapshot(for sessionID: UUID) -> AgentLifecycleSnapshot? {
        let url = statusURL(for: sessionID)
        guard let data = try? Data(contentsOf: url),
              let value = String(data: data, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              let state = AgentLifecycleState(rawValue: value)
        else { return nil }
        let attributes = try? FileManager.default.attributesOfItem(atPath: url.path)
        let modifiedAt = attributes?[.modificationDate] as? Date ?? .distantPast
        return AgentLifecycleSnapshot(state: state, observedAt: modifiedAt)
    }

    static func clear(_ sessionID: UUID) {
        try? FileManager.default.removeItem(at: statusURL(for: sessionID))
    }

    private static func statusURL(for sessionID: UUID) -> URL {
        applicationSupportDirectory()
            .appendingPathComponent("agent-status", isDirectory: true)
            .appendingPathComponent("\(sessionID.uuidString).status")
    }

    private static func applicationSupportDirectory() -> URL {
        let base = (try? FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )) ?? FileManager.default.homeDirectoryForCurrentUser
        return base.appendingPathComponent("Vibra", isDirectory: true)
    }
}

enum CodexStatusIntegration {
    static let marker = "vibra-agent-status-v1"

    static var hooksURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".codex", isDirectory: true)
            .appendingPathComponent("hooks.json")
    }

    static func isInstalled(at url: URL = hooksURL) -> Bool {
        guard let data = try? Data(contentsOf: url),
              let object = try? JSONSerialization.jsonObject(with: data),
              let document = object as? [String: Any],
              let hooks = document["hooks"] as? [String: Any]
        else { return false }

        return requiredEvents.allSatisfy { event, _ in
            guard let groups = hooks[event] as? [[String: Any]] else { return false }
            return groups.contains(where: containsVibraHook)
        }
    }

    static func install(at url: URL = hooksURL) throws {
        var document: [String: Any] = [:]
        if FileManager.default.fileExists(atPath: url.path) {
            let data = try Data(contentsOf: url)
            guard let existing = try JSONSerialization.jsonObject(with: data)
                as? [String: Any] else {
                throw IntegrationError.invalidHooksFile
            }
            document = existing
        }

        var hooks = document["hooks"] as? [String: Any] ?? [:]
        for (event, state) in requiredEvents {
            var groups = hooks[event] as? [[String: Any]] ?? []
            groups.removeAll(where: containsVibraHook)
            groups.append([
                "hooks": [[
                    "type": "command",
                    "command": command(for: state),
                    "timeout": 3,
                ]],
            ])
            hooks[event] = groups
        }
        document["description"] = document["description"]
            ?? "Codex lifecycle hooks, including Vibra agent status integration."
        document["hooks"] = hooks

        let parent = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true)
        let data = try JSONSerialization.data(
            withJSONObject: document,
            options: [.prettyPrinted, .sortedKeys]
        )
        try data.write(to: url, options: .atomic)
    }

    private static let requiredEvents: [(String, AgentLifecycleState)] = [
        ("SessionStart", .ready),
        ("UserPromptSubmit", .working),
        ("PermissionRequest", .needsAttention),
        ("Stop", .finished),
        ("SessionEnd", .inactive),
    ]

    private static func command(for state: AgentLifecycleState) -> String {
        let script = """
        session=${VIBRA_SESSION_ID:-}; test -n "$session" || exit 0; \
        dir="$HOME/Library/Application Support/Vibra/agent-status"; \
        /bin/mkdir -p "$dir"; file="$dir/$session.status"; \
        /usr/bin/printf "%s\\n" "\(state.rawValue)" > "$file.tmp.$$"; \
        /bin/mv "$file.tmp.$$" "$file"
        """
        return "/bin/sh -c '\(script)' # \(marker)"
    }

    private static func containsVibraHook(_ group: [String: Any]) -> Bool {
        guard let handlers = group["hooks"] as? [[String: Any]] else { return false }
        return handlers.contains { handler in
            (handler["command"] as? String)?.contains(marker) == true
        }
    }
}

enum IntegrationError: LocalizedError {
    case invalidHooksFile

    var errorDescription: String? {
        switch self {
        case .invalidHooksFile:
            "~/.codex/hooks.json does not contain a valid JSON object."
        }
    }
}
