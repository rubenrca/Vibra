import Foundation

struct ListeningPort: Identifiable, Equatable, Sendable {
    let port: Int
    let pid: pid_t
    let processName: String

    var id: String { "\(pid):\(port)" }

    var url: URL? {
        URL(string: "http://localhost:\(port)")
    }
}

/// Discovers TCP listen sockets belonging to a session process tree.
enum SessionPortProbe {
    /// Runs `lsof` and returns listening ports owned by any of `pids`.
    nonisolated static func listeningPorts(for pids: Set<pid_t>) -> [ListeningPort] {
        guard !pids.isEmpty else { return [] }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
        process.arguments = ["-nP", "-iTCP", "-sTCP:LISTEN"]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            return []
        }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        let text = String(decoding: data, as: UTF8.self)
        return parseLsof(text, allowedPIDs: pids)
    }

    /// Parses `lsof -nP -iTCP -sTCP:LISTEN` tabular output. Exposed for tests.
    nonisolated static func parseLsof(_ text: String, allowedPIDs: Set<pid_t>) -> [ListeningPort] {
        var result: [ListeningPort] = []
        var seen: Set<String> = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let fields = line.split(whereSeparator: \Character.isWhitespace).map(String.init)
            // COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
            // NAME may be "*:3000" or "*:3000 (LISTEN)" → last token is often "(LISTEN)".
            guard fields.count >= 9 else { continue }
            guard fields[0] != "COMMAND" else { continue }
            guard let pid = pid_t(fields[1]), allowedPIDs.contains(pid) else { continue }
            let nameField: String
            if let tcpIndex = fields.firstIndex(where: { $0 == "TCP" || $0 == "UDP" }),
               tcpIndex + 1 < fields.count
            {
                nameField = fields[tcpIndex + 1]
            } else if let candidate = fields.last(where: { extractPort(from: $0) != nil }) {
                nameField = candidate
            } else {
                continue
            }
            guard let port = extractPort(from: nameField) else { continue }
            let processName = fields[0]
            let key = "\(pid):\(port)"
            guard seen.insert(key).inserted else { continue }
            result.append(ListeningPort(port: port, pid: pid, processName: processName))
        }
        return result.sorted {
            if $0.port != $1.port { return $0.port < $1.port }
            return $0.pid < $1.pid
        }
    }

    nonisolated static func extractPort(from name: String) -> Int? {
        // Examples: *:3000, 127.0.0.1:8080, [::1]:5173, *:3000 (LISTEN)
        let cleaned = name.split(separator: " ").first.map(String.init) ?? name
        if let range = cleaned.range(of: #":(\d+)$"#, options: .regularExpression) {
            let digits = cleaned[range].dropFirst()
            return Int(digits)
        }
        return nil
    }
}
