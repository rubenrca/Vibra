import AppKit
import SwiftUI

enum VibraWindowID {
    static let terminal = "terminal"
}

@main
struct VibraApp: App {
    @NSApplicationDelegateAdaptor(VibraApplicationDelegate.self)
    private var applicationDelegate

    @StateObject private var updater = UpdaterModel()

    init() {
        UserDefaults.standard.register(defaults: SettingsKeys.defaults)
        TerminalAppearance.migrateToMatchGhosttyIfNeeded()
    }

    var body: some Scene {
        WindowGroup("Vibra") {
            WorkspaceRootView()
        }
        .defaultSize(width: 1080, height: 720)
        .windowStyle(.hiddenTitleBar)
        // Register commands once for the whole app. Attaching the same
        // Commands builder to multiple WindowGroups duplicates every menu item.
        .commands {
            VibraCommands(updater: updater)
        }

        WindowGroup("Terminal", id: VibraWindowID.terminal) {
            WorkspaceRootView(mode: .terminal)
        }
        .defaultSize(width: 900, height: 620)
        .windowStyle(.hiddenTitleBar)

        Settings {
            AppSettingsView()
                .environmentObject(updater)
                .environment(\.appChrome, ChromeThemeController.shared.theme)
        }
        .windowResizability(.contentSize)
    }
}

final class VibraApplicationDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
        // Safe to read appearance now that NSApplication is fully up.
        ChromeThemeController.shared.refresh()
    }
}

private struct VibraCommands: Commands {
    @FocusedObject private var workspace: WorkspaceStore?
    @Environment(\.openWindow) private var openWindow

    @ObservedObject var updater: UpdaterModel

    var body: some Commands {
        CommandGroup(after: .appInfo) {
            Button("Check for Updates…") {
                updater.checkForUpdates()
            }
            .disabled(!updater.canCheckForUpdates)
        }

        CommandGroup(replacing: .newItem) {
            Button("New Tab") {
                workspace?.newWorkspace()
            }
            .keyboardShortcut("n", modifiers: .command)

            Button("New Terminal Tab") {
                workspace?.newSession()
            }
            .keyboardShortcut("t", modifiers: .command)
            .disabled(workspace?.selectedProject == nil)

            Button("New Terminal Window") {
                openWindow(id: VibraWindowID.terminal)
            }
            .keyboardShortcut("n", modifiers: [.command, .shift])

            Button("Open Folder in New Tab…") {
                workspace?.chooseFolder()
            }
            .keyboardShortcut("o", modifiers: .command)

            Divider()

            Button(openInEditorMenuTitle) {
                openSelectedProjectInEditor()
            }
            .keyboardShortcut("e", modifiers: [.command, .shift])
            .disabled(workspace?.selectedProject == nil
                || ExternalEditorLauncher.installedEditors().isEmpty)
        }

        CommandGroup(replacing: .saveItem) {}

        CommandGroup(after: .newItem) {
            Button("Split Right") {
                workspace?.splitSelected(.horizontal)
            }
            .keyboardShortcut("d", modifiers: .command)
            .disabled(workspace?.selectedSession == nil)

            Button("Split Down") {
                workspace?.splitSelected(.vertical)
            }
            .keyboardShortcut("d", modifiers: [.command, .shift])
            .disabled(workspace?.selectedSession == nil)

            Divider()

            Button("Close Terminal") {
                workspace?.closeSelectedSession()
            }
            .keyboardShortcut("w", modifiers: .command)
            .disabled(workspace?.selectedSession == nil)

            Button("Close Workspace") {
                workspace?.closeSelectedWorkspace()
            }
            .keyboardShortcut("w", modifiers: [.command, .shift])
            .disabled(workspace?.selectedWorkspace == nil)
        }

        CommandGroup(after: .sidebar) {
            Button("Toggle Terminal Sidebar") {
                workspace?.toggleTerminalSidebar()
            }
            .keyboardShortcut("b", modifiers: .command)

            Button("Toggle Right Sidebar") {
                workspace?.toggleGitSidebar()
            }
            .keyboardShortcut("r", modifiers: .command)
            .disabled(workspace?.selectedProject == nil)

            Divider()

            Button("Focus Previous Pane") {
                workspace?.focusAdjacentPane(-1)
            }
            .keyboardShortcut(.leftArrow, modifiers: [.command, .option])

            Button("Focus Next Pane") {
                workspace?.focusAdjacentPane(1)
            }
            .keyboardShortcut(.rightArrow, modifiers: [.command, .option])

            Button("Focus Pane Above") {
                workspace?.focusAdjacentPane(-1)
            }
            .keyboardShortcut(.upArrow, modifiers: [.command, .option])

            Button("Focus Pane Below") {
                workspace?.focusAdjacentPane(1)
            }
            .keyboardShortcut(.downArrow, modifiers: [.command, .option])
        }
    }

    private var openInEditorMenuTitle: String {
        if let preferred = ExternalEditorLauncher.resolvedEditor() {
            return "Open Project in \(preferred.shortName)"
        }
        return "Open Project in Editor"
    }

    private func openSelectedProjectInEditor() {
        guard let path = workspace?.selectedProject?.rootPath else { return }
        _ = try? ExternalEditorLauncher.open(path: path)
    }
}
