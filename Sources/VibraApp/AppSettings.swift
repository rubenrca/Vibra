import AppKit
import SwiftUI

enum SettingsKeys {
    static let projectSidebarVisible = "projectSidebarVisible"
    static let terminalSidebarWidth = "terminalSidebarWidth"
    static let gitSidebarVisible = "gitSidebarVisible"
    static let cmuxShortcutsEnabled = "cmuxShortcutsEnabled"
    static let gitAutoRefreshEnabled = "gitAutoRefreshEnabled"
    static let gitRefreshDelay = "gitRefreshDelay"
    static let tabFolderModelMigrated = "tabFolderModelMigrated"

    @MainActor static let defaults: [String: Any] = [
        cmuxShortcutsEnabled: true,
        gitAutoRefreshEnabled: true,
        gitRefreshDelay: 420,
    ]
}

struct AppSettingsView: View {
    @AppStorage(SettingsKeys.cmuxShortcutsEnabled)
    private var cmuxShortcutsEnabled = true
    @AppStorage(SettingsKeys.gitAutoRefreshEnabled)
    private var gitAutoRefreshEnabled = true
    @AppStorage(SettingsKeys.gitRefreshDelay)
    private var gitRefreshDelay = 420
    @State private var codexStatusInstalled = CodexStatusIntegration.isInstalled()
    @State private var codexStatusMessage: String?

    @EnvironmentObject private var updater: UpdaterModel

    var body: some View {
        TabView {
            Form {
                Section("Updates") {
                    Toggle(
                        "Check for updates automatically",
                        isOn: $updater.automaticallyChecksForUpdates
                    )
                    .disabled(!updater.isConfigured)

                    LabeledContent("Version", value: updater.currentVersion)

                    Button("Check Now…") {
                        updater.checkForUpdates()
                    }
                    .disabled(!updater.canCheckForUpdates)

                    if !updater.isConfigured {
                        Text("Updates are only available in a packaged Vibra.app.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }

                Section("Keyboard") {
                    Toggle("Enable cmux-style shortcuts", isOn: $cmuxShortcutsEnabled)
                    Text("Shortcuts are captured before the terminal receives the key event.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                Section("Agent Activity") {
                    LabeledContent("Codex status") {
                        Text(codexStatusInstalled ? "Connected" : "Basic detection")
                            .foregroundStyle(codexStatusInstalled ? Color.green : .secondary)
                    }
                    Button(codexStatusInstalled ? "Reinstall Status Hooks" : "Install Status Hooks") {
                        installCodexStatusHooks()
                    }
                    if let codexStatusMessage {
                        Text(codexStatusMessage)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else {
                        Text("Hooks distinguish idle, working, attention, and finished states.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }

                Section("Git Changes") {
                    Toggle("Refresh automatically", isOn: $gitAutoRefreshEnabled)
                    Picker("Refresh interval", selection: $gitRefreshDelay) {
                        Text("Fast").tag(250)
                        Text("Balanced").tag(420)
                        Text("Low activity").tag(800)
                    }
                    .disabled(!gitAutoRefreshEnabled)
                }
            }
            .formStyle(.grouped)
            .tabItem { Label("General", systemImage: "gearshape") }

            ShortcutReferenceView()
                .tabItem { Label("Shortcuts", systemImage: "keyboard") }
        }
        .frame(width: 520, height: 470)
    }

    private func installCodexStatusHooks() {
        do {
            try CodexStatusIntegration.install()
            codexStatusInstalled = true
            codexStatusMessage = "Installed. Start a new Codex session and use /hooks to trust it."
        } catch {
            codexStatusMessage = error.localizedDescription
        }
    }
}

private struct ShortcutReferenceView: View {
    private let groups: [(String, [(String, String)])] = [
        ("Terminals", [
            ("New sidebar tab", "⌘N"),
            ("New terminal tab", "⌘T"),
            ("New terminal window", "⇧⌘N"),
            ("Open directory in new tab", "⌘O"),
            ("Close focused pane", "⌘W"),
        ]),
        ("Panes", [
            ("Split right", "⌘D"),
            ("Split down", "⇧⌘D"),
            ("Focus adjacent pane", "⌥⌘←  →  ↑  ↓"),
        ]),
        ("Sidebars", [
            ("Toggle terminals", "⌘B"),
            ("Toggle right sidebar", "⌘R"),
        ]),
    ]

    var body: some View {
        ScrollView {
            VStack(spacing: 18) {
                ForEach(groups, id: \.0) { group in
                    VStack(spacing: 0) {
                        HStack {
                            Text(group.0)
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(.secondary)
                            Spacer()
                        }
                        .padding(.horizontal, 12)
                        .padding(.bottom, 7)

                        VStack(spacing: 0) {
                            ForEach(Array(group.1.enumerated()), id: \.offset) { index, item in
                                HStack {
                                    Text(item.0).font(.system(size: 12))
                                    Spacer()
                                    Text(item.1)
                                        .font(.system(size: 11, weight: .medium, design: .rounded))
                                        .foregroundStyle(.secondary)
                                }
                                .padding(.horizontal, 12)
                                .frame(height: 36)
                                if index < group.1.count - 1 { Divider().padding(.leading, 12) }
                            }
                        }
                        .background(Color.primary.opacity(0.045), in: RoundedRectangle(cornerRadius: 9))
                    }
                }
            }
            .padding(20)
        }
    }
}

struct KeyboardShortcutMonitor: NSViewRepresentable {
    let store: WorkspaceStore
    let newTerminalWindow: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(store: store, newTerminalWindow: newTerminalWindow)
    }

    func makeNSView(context: Context) -> NSView {
        context.coordinator.install()
        let view = NSView(frame: .zero)
        DispatchQueue.main.async { [weak view, weak coordinator = context.coordinator] in
            coordinator?.window = view?.window
        }
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.store = store
        context.coordinator.newTerminalWindow = newTerminalWindow
        context.coordinator.window = view.window
    }

    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        coordinator.uninstall()
    }

    @MainActor
    final class Coordinator {
        weak var store: WorkspaceStore?
        weak var window: NSWindow?
        var newTerminalWindow: () -> Void
        private var monitor: Any?

        init(store: WorkspaceStore, newTerminalWindow: @escaping () -> Void) {
            self.store = store
            self.newTerminalWindow = newTerminalWindow
        }

        func install() {
            guard monitor == nil else { return }
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self else { return event }
                if event.window === self.window {
                    self.store?.observeTerminalKeyEvent(event)
                }
                guard
                      UserDefaults.standard.bool(forKey: SettingsKeys.cmuxShortcutsEnabled),
                      self.handle(event) else { return event }
                return nil
            }
        }

        func uninstall() {
            guard let monitor else { return }
            NSEvent.removeMonitor(monitor)
            self.monitor = nil
        }

        private func handle(_ event: NSEvent) -> Bool {
            guard let store, event.window === window else { return false }
            let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
                .subtracting([.capsLock, .numericPad, .function])
            let key = event.charactersIgnoringModifiers?.lowercased() ?? ""

            if flags == [.command] {
                switch key {
                case "n": store.newWorkspace()
                case "t": store.newSession()
                case "d": store.splitSelected(.horizontal)
                case "w": store.closeSelectedSession()
                case "b": store.toggleTerminalSidebar()
                case "r": store.toggleGitSidebar()
                default: return false
                }
                return true
            }

            if flags == [.command, .shift] {
                switch key {
                case "n": newTerminalWindow()
                case "d": store.splitSelected(.vertical)
                default: return false
                }
                return true
            }

            guard flags == [.command, .option] else { return false }
            switch event.keyCode {
            case 123, 126: store.focusAdjacentPane(-1)
            case 124, 125: store.focusAdjacentPane(1)
            default: return false
            }
            return true
        }
    }
}
