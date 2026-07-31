import Foundation
import Testing
@testable import VibraApp

private struct MockApplicationLookup: ApplicationLookup {
    let mapping: [String: URL]

    func urlForApplication(withBundleIdentifier id: String) -> URL? {
        mapping[id]
    }
}

@Test func installedEditorsFiltersByBundleIdentifier() {
    let cursor = URL(fileURLWithPath: "/Applications/Cursor.app")
    let zed = URL(fileURLWithPath: "/Applications/Zed.app")
    let lookup = MockApplicationLookup(mapping: [
        "com.todesktop.230313mzl4w4u92": cursor,
        "dev.zed.Zed": zed,
    ])

    let installed = ExternalEditorLauncher.installedEditors(lookup: lookup)
    #expect(installed.map(\.kind) == [.cursor, .zed])
    #expect(installed[0].appURL == cursor)
    #expect(installed[1].appURL == zed)
}

@Test func preferredEditorFallsBackWhenMissing() {
    let vscode = URL(fileURLWithPath: "/Applications/Visual Studio Code.app")
    let lookup = MockApplicationLookup(mapping: [
        "com.microsoft.VSCode": vscode,
    ])
    let suite = "vibra.tests.external-editor.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suite)!
    defer { defaults.removePersistentDomain(forName: suite) }

    ExternalEditorLauncher.setPreferredKind(.cursor, defaults: defaults)
    #expect(ExternalEditorLauncher.preferredKind(defaults: defaults) == .cursor)

    // Preferred Cursor is not installed → first available (VS Code).
    let resolved = ExternalEditorLauncher.resolvedEditor(lookup: lookup, defaults: defaults)
    #expect(resolved?.kind == .vscode)
}

@Test func preferredEditorUsesInstalledMatch() {
    let cursor = URL(fileURLWithPath: "/Applications/Cursor.app")
    let vscode = URL(fileURLWithPath: "/Applications/Visual Studio Code.app")
    let lookup = MockApplicationLookup(mapping: [
        "com.todesktop.230313mzl4w4u92": cursor,
        "com.microsoft.VSCode": vscode,
    ])
    let suite = "vibra.tests.external-editor.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suite)!
    defer { defaults.removePersistentDomain(forName: suite) }

    ExternalEditorLauncher.setPreferredKind(.vscode, defaults: defaults)
    let resolved = ExternalEditorLauncher.resolvedEditor(lookup: lookup, defaults: defaults)
    #expect(resolved?.kind == .vscode)
    #expect(resolved?.appURL == vscode)
}

@MainActor
@Test func openMissingPathThrows() {
    let path = "/tmp/vibra-does-not-exist-\(UUID().uuidString)"
    let lookup = MockApplicationLookup(mapping: [
        "dev.zed.Zed": URL(fileURLWithPath: "/Applications/Zed.app"),
    ])
    #expect(throws: ExternalEditorError.pathMissing(path)) {
        try ExternalEditorLauncher.open(path: path, lookup: lookup)
    }
}

@MainActor
@Test func openWithNoEditorsThrows() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-editor-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }

    let lookup = MockApplicationLookup(mapping: [:])
    #expect(throws: ExternalEditorError.noEditorsInstalled) {
        try ExternalEditorLauncher.open(path: root.path, lookup: lookup)
    }
}

@Test func knownEditorDisplayNamesAreStable() {
    #expect(KnownExternalEditor.cursor.displayName == "Cursor")
    #expect(KnownExternalEditor.vscode.shortName == "VS Code")
    #expect(KnownExternalEditor.zed.bundleIdentifiers.contains("dev.zed.Zed"))
}
