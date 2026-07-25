import AppKit
import SwiftUI

@main
struct VibraApp: App {
    @NSApplicationDelegateAdaptor(VibraApplicationDelegate.self)
    private var applicationDelegate

    @StateObject private var updater = UpdaterModel()

    init() {
        UserDefaults.standard.register(defaults: SettingsKeys.defaults)
    }

    var body: some Scene {
        WindowGroup("Vibra") {
            WorkspaceRootView()
        }
        .defaultSize(width: 1080, height: 720)
        .windowStyle(.hiddenTitleBar)
        .commands {
            VibraCommands(updater: updater)
        }

        Settings {
            AppSettingsView()
                .environmentObject(updater)
        }
        .windowResizability(.contentSize)
    }
}

final class VibraApplicationDelegate: NSObject, NSApplicationDelegate {
    private var applicationShortcutMonitor: Any?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
        applicationShortcutMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) {
            event in
            let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
                .subtracting([.capsLock, .numericPad, .function])
            guard flags == [.command],
                  event.charactersIgnoringModifiers?.lowercased() == "r",
                  event.window?.identifier?.rawValue != "com_apple_SwiftUI_Settings_window"
            else { return event }
            NotificationCenter.default.post(name: .showGitSidebarRequested, object: nil)
            return nil
        }
    }
}

extension Notification.Name {
    static let showGitSidebarRequested = Notification.Name("showGitSidebarRequested")
}

private struct VibraCommands: Commands {
    @FocusedObject private var workspace: WorkspaceStore?

    @ObservedObject var updater: UpdaterModel

    var body: some Commands {
        CommandGroup(after: .appInfo) {
            Button("Check for Updates…") {
                updater.checkForUpdates()
            }
            .disabled(!updater.canCheckForUpdates)
        }

        CommandGroup(replacing: .appSettings) {
            SettingsLink {
                Text("Settings…")
            }
            .keyboardShortcut(",", modifiers: .command)
        }

        CommandGroup(replacing: .newItem) {
            Button("New Workspace") {
                workspace?.newWorkspace()
            }
            .keyboardShortcut("n", modifiers: .command)

            Button("New Tab") {
                workspace?.newSession()
            }
            .keyboardShortcut("t", modifiers: .command)
            .disabled(workspace?.selectedProject == nil)

            Button("Open Project…") {
                workspace?.chooseProject()
            }
            .keyboardShortcut("o", modifiers: .command)
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
        }

        CommandGroup(after: .sidebar) {
            Button("Toggle Project Sidebar") {
                workspace?.toggleProjectSidebar()
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
}
