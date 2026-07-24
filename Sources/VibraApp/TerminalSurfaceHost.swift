import AppKit
import SwiftUI

struct TerminalSurfaceHost: NSViewRepresentable {
    let session: TerminalSession
    let isVisible: Bool
    let isFocused: Bool

    func makeNSView(context: Context) -> TerminalContainerView {
        TerminalContainerView(
            session: session,
            isVisible: isVisible,
            isFocused: isFocused
        )
    }

    func updateNSView(_ view: TerminalContainerView, context: Context) {
        view.update(session: session, isVisible: isVisible, isFocused: isFocused)
    }

    static func dismantleNSView(
        _ view: TerminalContainerView,
        coordinator: ()
    ) {
        view.unmount()
    }
}

final class TerminalContainerView: NSView {
    private weak var session: TerminalSession?
    private var isVisible: Bool
    private var isFocused: Bool
    private var lastFittedSize: NSSize = .zero

    init(session: TerminalSession, isVisible: Bool, isFocused: Bool) {
        self.session = session
        self.isVisible = isVisible
        self.isFocused = isFocused
        super.init(frame: .zero)
        wantsLayer = true
        mount(session.terminalView)
        applyVisibility()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        applyVisibility()
        focusIfNeeded()
    }

    override func layout() {
        super.layout()
        guard let terminal = session?.terminalView, terminal.superview === self else { return }
        if terminal.frame != bounds {
            terminal.frame = bounds
        }
        guard bounds.size != lastFittedSize else { return }
        lastFittedSize = bounds.size
        terminal.fitToSize()
    }

    /// SwiftUI keeps a representable's view alive whenever the surrounding
    /// structure is unchanged, so switching tabs or panes arrives as a new
    /// session on the existing container rather than a fresh one. Re-host the
    /// terminal in that case; otherwise the container keeps rendering — and
    /// holding — the session it was created with.
    func update(session: TerminalSession, isVisible: Bool, isFocused: Bool) {
        let sessionChanged = self.session !== session
        guard sessionChanged || self.isVisible != isVisible || self.isFocused != isFocused
        else { return }

        if sessionChanged {
            releaseTerminal()
            self.session = session
            mount(session.terminalView)
            lastFittedSize = .zero
            needsLayout = true
        }
        self.isVisible = isVisible
        self.isFocused = isFocused
        applyVisibility()
        focusIfNeeded()
    }

    func unmount() {
        isVisible = false
        isFocused = false
        releaseTerminal()
        session = nil
    }

    /// Hands the terminal back only when this container still owns it. Another
    /// container may already have adopted it during the same update pass.
    private func releaseTerminal() {
        guard let session, session.terminalView.superview === self else { return }
        session.setVisible(false)
        session.terminalView.removeFromSuperview()
    }

    private func mount(_ terminal: NSView) {
        terminal.removeFromSuperview()
        terminal.translatesAutoresizingMaskIntoConstraints = false
        addSubview(terminal)
        NSLayoutConstraint.activate([
            terminal.leadingAnchor.constraint(equalTo: leadingAnchor),
            terminal.trailingAnchor.constraint(equalTo: trailingAnchor),
            terminal.topAnchor.constraint(equalTo: topAnchor),
            terminal.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    private func applyVisibility() {
        let visible = isVisible && window != nil
        alphaValue = visible ? 1 : 0
        session?.setVisible(visible)
    }

    private func focusIfNeeded() {
        guard isFocused, isVisible, let window, let terminal = session?.terminalView else {
            return
        }
        DispatchQueue.main.async { [weak window, weak terminal] in
            guard let window, let terminal, terminal.window === window else { return }
            window.makeFirstResponder(terminal)
        }
    }
}
