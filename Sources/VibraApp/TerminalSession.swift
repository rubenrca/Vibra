import AppKit
import GhosttyTerminal

@MainActor
final class TerminalSession: Identifiable {
    let id: UUID
    let state: TerminalViewState
    let terminalView: TerminalView
    let initialWorkingDirectory: String

    var onExit: ((TerminalSession) -> Void)?

    private(set) var isVisible = false
    private(set) var isClosed = false

    init(id: UUID = UUID(), workingDirectory: String) {
        self.id = id
        initialWorkingDirectory = workingDirectory

        let configuration = TerminalConfiguration { builder in
            builder.withFontSize(13)
            builder.withWindowPaddingX(10)
            builder.withWindowPaddingY(8)
            builder.withCursorStyle(.block)
            builder.withCursorStyleBlink(true)
            builder.withCustom("scrollback-limit", "1048576")
            builder.withCustom("macos-option-as-alt", "true")
        }
        let state = TerminalViewState(
            configSource: .none,
            terminalConfiguration: configuration
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
    }

    var title: String {
        let reported = state.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return reported.isEmpty ? "Terminal" : reported
    }

    var workingDirectory: String {
        guard let reported = state.workingDirectory, !reported.isEmpty else {
            return initialWorkingDirectory
        }
        return reported
    }

    func setVisible(_ visible: Bool) {
        guard !isClosed, isVisible != visible else { return }
        isVisible = visible
        terminalView.setSurfaceVisible(visible)
    }

    /// The current libghostty wrapper suppresses wakeups for an occluded
    /// surface. A low-frequency tick keeps hidden PTYs and title/cwd events
    /// draining without refreshing or drawing their Metal surfaces.
    func tickWhileHidden() {
        guard !isClosed, !isVisible else { return }
        state.controller.tick()
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
    }
}
