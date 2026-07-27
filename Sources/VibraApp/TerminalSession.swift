import AppKit
import Combine
import Darwin
import GhosttyTerminal

@MainActor
final class TerminalSession: ObservableObject, Identifiable {
    let id: UUID
    let state: TerminalViewState
    let terminalView: TerminalView
    let initialWorkingDirectory: String

    var onExit: ((TerminalSession) -> Void)?

    @Published private(set) var agentActivity: AgentActivity = .idle
    @Published private(set) var liveWorkingDirectory: String

    private(set) var isVisible = false
    private(set) var isClosed = false
    private var missedAgentPolls = 0
    private var detectedAgent: CodingAgent?
    private var latestAgentActivityAt = Date.distantPast
    private var cancellables: Set<AnyCancellable> = []

    init(id: UUID = UUID(), workingDirectory: String) {
        self.id = id
        initialWorkingDirectory = workingDirectory
        liveWorkingDirectory = workingDirectory
        AgentLifecycleStore.clear(id)

        let appearance = TerminalAppearance.current
        let state = TerminalViewState(
            configSource: appearance.configSource,
            theme: appearance.terminalTheme,
            terminalConfiguration: appearance.terminalConfiguration
        )
        self.state = state

        let terminalView = TerminalView(
            frame: NSRect(x: 0, y: 0, width: 900, height: 600)
        )
        self.terminalView = terminalView

        terminalView.delegate = state
        terminalView.configuration = TerminalSurfaceOptions(
            backend: .exec,
            workingDirectory: workingDirectory,
            envVars: ["VIBRA_SESSION_ID": id.uuidString]
        )
        terminalView.controller = state.controller
        terminalView.setSurfaceVisible(false)

        state.onClose = { [weak self] _ in
            guard let self, !self.isClosed else { return }
            self.onExit?(self)
        }

        state.$lastDesktopNotificationAt
            .dropFirst()
            .compactMap { $0 }
            .sink { [weak self] _ in
                Task { @MainActor [weak self] in
                    self?.handleDesktopNotification()
                }
            }
            .store(in: &cancellables)
    }

    var title: String {
        let reported = state.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return reported.isEmpty ? "Terminal" : reported
    }

    var workingDirectory: String {
        liveWorkingDirectory
    }

    func refreshWorkingDirectory() {
        let resolved: String?
        if let pid = terminalView.foregroundPid {
            resolved = ProcessWorkingDirectoryProbe.directory(for: pid)
        } else if let reported = state.workingDirectory, !reported.isEmpty {
            resolved = reported
        } else {
            resolved = nil
        }

        guard let resolved,
              FileManager.default.fileExists(atPath: resolved),
              resolved != liveWorkingDirectory
        else { return }
        liveWorkingDirectory = resolved
    }

    func setVisible(_ visible: Bool) {
        guard !isClosed, isVisible != visible else { return }
        isVisible = visible
        terminalView.setSurfaceVisible(visible)
    }

    /// Reconfigures colors, font, and cursor without recreating the PTY.
    /// Base config file (Ghostty) is fixed at session creation.
    func applyAppearance(_ appearance: TerminalAppearance) {
        guard !isClosed else { return }
        state.setTheme(appearance.terminalTheme)
        state.setTerminalConfiguration(appearance.terminalConfiguration)
    }

    /// The current libghostty wrapper suppresses wakeups for an occluded
    /// surface. A low-frequency tick keeps hidden PTYs and title/cwd events
    /// draining without refreshing or drawing their Metal surfaces.
    func tickWhileHidden() {
        guard !isClosed, !isVisible else { return }
        state.controller.tick()
    }

    func updateForegroundProcess(
        _ snapshot: ForegroundProcessSnapshot?,
        lifecycle: AgentLifecycleSnapshot?
    ) {
        guard !isClosed else { return }
        let detected = CodingAgent.detect(
            commandLine: snapshot?.commandLine ?? "",
            title: ""
        )

        if let detected {
            detectedAgent = detected
            missedAgentPolls = 0
            let transcriptLifecycle = snapshot?.lifecycle
            let newestLifecycle = [lifecycle, transcriptLifecycle]
                .compactMap { $0 }
                .max { $0.observedAt < $1.observedAt }
            if let newestLifecycle,
               newestLifecycle.observedAt > latestAgentActivityAt {
                apply(newestLifecycle, agent: detected)
                return
            }
            if agentActivity.agent != detected {
                agentActivity = .ready(agent: detected)
            }
            return
        }

        guard agentActivity.agent != nil else { return }
        missedAgentPolls += 1
        guard missedAgentPolls >= 2 else { return }
        missedAgentPolls = 0

        switch agentActivity {
        case .ready(let agent),
             .running(let agent, _),
             .needsAttention(let agent, _):
            let succeeded = state.lastCommandExitCode.map { $0 == 0 }
            agentActivity = .finished(agent: agent, succeeded: succeeded, at: Date())
        case .idle, .finished:
            break
        }
        detectedAgent = nil
    }

    func noteUserSubmittedInput() {
        guard let agent = detectedAgent else { return }
        switch agentActivity {
        case .running(let current, _) where current == agent:
            break
        default:
            let now = Date()
            latestAgentActivityAt = now
            agentActivity = .running(agent: agent, since: now)
        }
    }

    private func handleDesktopNotification() {
        let body = state.lastDesktopNotificationBody?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let notificationText = [state.lastDesktopNotificationTitle, body]
            .compactMap { $0 }
            .joined(separator: " ")
        guard let agent = agentActivity.agent
            ?? CodingAgent.detect(commandLine: "", title: notificationText) else { return }
        let normalized = notificationText.lowercased()
        if normalized.contains("approval")
            || normalized.contains("question")
            || normalized.contains("input")
            || normalized.contains("waiting") {
            latestAgentActivityAt = Date()
            agentActivity = .needsAttention(
                agent: agent,
                message: body?.isEmpty == false ? body : nil
            )
        } else {
            latestAgentActivityAt = Date()
            agentActivity = .finished(agent: agent, succeeded: nil, at: Date())
        }
    }

    private func apply(_ lifecycle: AgentLifecycleSnapshot, agent: CodingAgent) {
        latestAgentActivityAt = lifecycle.observedAt
        let state = lifecycle.state
        switch state {
        case .ready:
            agentActivity = .ready(agent: agent)
        case .working:
            if case .running(let current, _) = agentActivity, current == agent { return }
            agentActivity = .running(agent: agent, since: Date())
        case .needsAttention:
            agentActivity = .needsAttention(agent: agent, message: nil)
        case .finished:
            agentActivity = .finished(agent: agent, succeeded: nil, at: Date())
        case .inactive:
            agentActivity = .idle
        }
    }

    func shutdown() {
        guard !isClosed else { return }
        isClosed = true
        isVisible = false
        state.onClose = nil
        terminalView.setSurfaceVisible(false)
        terminalView.delegate = nil
        terminalView.controller = nil
        onExit = nil
        cancellables.removeAll()
        AgentLifecycleStore.clear(id)
    }
}

enum ProcessWorkingDirectoryProbe {
    nonisolated static func directory(for pid: pid_t) -> String? {
        var info = proc_vnodepathinfo()
        let expectedSize = MemoryLayout<proc_vnodepathinfo>.stride
        let result = proc_pidinfo(
            pid,
            PROC_PIDVNODEPATHINFO,
            0,
            &info,
            Int32(expectedSize)
        )
        guard result == Int32(expectedSize) else { return nil }

        return withUnsafePointer(to: &info.pvi_cdir.vip_path) { pathPointer in
            pathPointer.withMemoryRebound(
                to: CChar.self,
                capacity: Int(MAXPATHLEN)
            ) { characters in
                let path = String(cString: characters)
                return path.isEmpty ? nil : path
            }
        }
    }
}
