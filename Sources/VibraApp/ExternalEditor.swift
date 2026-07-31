import AppKit
import Foundation
import SwiftUI

/// Well-known external editors Vibra can open a project or file in.
/// Detection is by bundle identifier so renames of the .app still work.
enum KnownExternalEditor: String, CaseIterable, Identifiable, Sendable {
    case cursor
    case vscode
    case vscodeInsiders
    case windsurf
    case zed
    case xcode
    case sublimeText
    case vscodium

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .cursor: "Cursor"
        case .vscode: "Visual Studio Code"
        case .vscodeInsiders: "VS Code Insiders"
        case .windsurf: "Windsurf"
        case .zed: "Zed"
        case .xcode: "Xcode"
        case .sublimeText: "Sublime Text"
        case .vscodium: "VSCodium"
        }
    }

    /// Short label for compact UI (menus, buttons).
    var shortName: String {
        switch self {
        case .cursor: "Cursor"
        case .vscode: "VS Code"
        case .vscodeInsiders: "Insiders"
        case .windsurf: "Windsurf"
        case .zed: "Zed"
        case .xcode: "Xcode"
        case .sublimeText: "Sublime"
        case .vscodium: "VSCodium"
        }
    }

    var systemImage: String {
        switch self {
        case .xcode: "hammer.fill"
        default: "chevron.left.forwardslash.chevron.right"
        }
    }

    /// Bundle IDs in preference order (stable primary first).
    var bundleIdentifiers: [String] {
        switch self {
        case .cursor:
            // Cursor ships under a Todesktop-generated id.
            ["com.todesktop.230313mzl4w4u92"]
        case .vscode:
            ["com.microsoft.VSCode"]
        case .vscodeInsiders:
            ["com.microsoft.VSCodeInsiders"]
        case .windsurf:
            ["com.exafunction.windsurf", "com.windsurf.ide", "com.codeium.windsurf"]
        case .zed:
            ["dev.zed.Zed"]
        case .xcode:
            ["com.apple.dt.Xcode"]
        case .sublimeText:
            ["com.sublimetext.4", "com.sublimetext.3"]
        case .vscodium:
            ["com.vscodium", "com.visualstudio.code.oss"]
        }
    }
}

struct InstalledExternalEditor: Identifiable, Equatable, Sendable {
    let kind: KnownExternalEditor
    let appURL: URL

    var id: String { kind.rawValue }
    var displayName: String { kind.displayName }
    var shortName: String { kind.shortName }
    var systemImage: String { kind.systemImage }
}

/// Resolves application URLs. Injectable for tests.
protocol ApplicationLookup: Sendable {
    func urlForApplication(withBundleIdentifier id: String) -> URL?
}

struct SystemApplicationLookup: ApplicationLookup {
    func urlForApplication(withBundleIdentifier id: String) -> URL? {
        NSWorkspace.shared.urlForApplication(withBundleIdentifier: id)
    }
}

enum ExternalEditorError: LocalizedError, Equatable {
    case pathMissing(String)
    case noEditorsInstalled
    case editorNotInstalled(KnownExternalEditor)

    var errorDescription: String? {
        switch self {
        case .pathMissing(let path):
            "Nothing to open at “\(path)”."
        case .noEditorsInstalled:
            "No supported editor is installed. Install Cursor, VS Code, Zed, or another IDE."
        case .editorNotInstalled(let editor):
            "\(editor.displayName) is not installed."
        }
    }
}

/// Detects installed editors and opens project folders or files in them.
enum ExternalEditorLauncher {
    // MARK: - Discovery

    static func installedEditors(
        lookup: some ApplicationLookup = SystemApplicationLookup()
    ) -> [InstalledExternalEditor] {
        KnownExternalEditor.allCases.compactMap { kind in
            guard let appURL = applicationURL(for: kind, lookup: lookup) else { return nil }
            return InstalledExternalEditor(kind: kind, appURL: appURL)
        }
    }

    static func applicationURL(
        for kind: KnownExternalEditor,
        lookup: some ApplicationLookup = SystemApplicationLookup()
    ) -> URL? {
        for bundleID in kind.bundleIdentifiers {
            if let url = lookup.urlForApplication(withBundleIdentifier: bundleID) {
                return url
            }
        }
        return nil
    }

    // MARK: - Preference

    static func preferredKind(
        defaults: UserDefaults = .standard
    ) -> KnownExternalEditor? {
        guard let raw = defaults.string(forKey: SettingsKeys.preferredExternalEditor),
              !raw.isEmpty
        else { return nil }
        return KnownExternalEditor(rawValue: raw)
    }

    static func setPreferredKind(
        _ kind: KnownExternalEditor?,
        defaults: UserDefaults = .standard
    ) {
        if let kind {
            defaults.set(kind.rawValue, forKey: SettingsKeys.preferredExternalEditor)
        } else {
            defaults.removeObject(forKey: SettingsKeys.preferredExternalEditor)
        }
    }

    /// Preferred editor if still installed; otherwise the first installed editor.
    static func resolvedEditor(
        lookup: some ApplicationLookup = SystemApplicationLookup(),
        defaults: UserDefaults = .standard
    ) -> InstalledExternalEditor? {
        let installed = installedEditors(lookup: lookup)
        if let preferred = preferredKind(defaults: defaults),
           let match = installed.first(where: { $0.kind == preferred }) {
            return match
        }
        return installed.first
    }

    // MARK: - Open

    @discardableResult
    @MainActor
    static func open(
        path: String,
        with editor: InstalledExternalEditor? = nil,
        lookup: some ApplicationLookup = SystemApplicationLookup(),
        defaults: UserDefaults = .standard,
        workspace: NSWorkspace = .shared
    ) throws -> InstalledExternalEditor {
        let target = URL(fileURLWithPath: path).standardizedFileURL
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: target.path, isDirectory: &isDirectory) else {
            throw ExternalEditorError.pathMissing(path)
        }

        let resolved: InstalledExternalEditor
        if let editor {
            resolved = editor
        } else if let fallback = resolvedEditor(lookup: lookup, defaults: defaults) {
            resolved = fallback
        } else {
            throw ExternalEditorError.noEditorsInstalled
        }

        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        workspace.open(
            [target],
            withApplicationAt: resolved.appURL,
            configuration: configuration,
            completionHandler: nil
        )
        return resolved
    }

    @discardableResult
    @MainActor
    static func open(
        path: String,
        kind: KnownExternalEditor,
        lookup: some ApplicationLookup = SystemApplicationLookup(),
        defaults: UserDefaults = .standard,
        workspace: NSWorkspace = .shared
    ) throws -> InstalledExternalEditor {
        guard let appURL = applicationURL(for: kind, lookup: lookup) else {
            throw ExternalEditorError.editorNotInstalled(kind)
        }
        return try open(
            path: path,
            with: InstalledExternalEditor(kind: kind, appURL: appURL),
            lookup: lookup,
            defaults: defaults,
            workspace: workspace
        )
    }
}

// MARK: - SwiftUI helpers

/// Codex-style pill: dark code glyph + chevron, menu of installed editors.
struct OpenInEditorButton: View {
    let path: String?
    /// When true, only the icon pill (header). When false, includes a short label.
    var compact: Bool = true
    @Environment(\.colorScheme) private var colorScheme
    @State private var lastError: String?
    @State private var isHovering = false

    private var editors: [InstalledExternalEditor] {
        ExternalEditorLauncher.installedEditors()
    }

    private var preferred: InstalledExternalEditor? {
        ExternalEditorLauncher.resolvedEditor()
    }

    private var isEnabled: Bool {
        path != nil && !editors.isEmpty
    }

    var body: some View {
        Menu {
            menuContent
        } label: {
            pillLabel
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .disabled(!isEnabled)
        .opacity(isEnabled ? 1 : 0.45)
        .help(helpText)
        .onHover { isHovering = $0 }
        .animation(.easeOut(duration: 0.12), value: isHovering)
        .alert("Could not open editor", isPresented: Binding(
            get: { lastError != nil },
            set: { if !$0 { lastError = nil } }
        )) {
            Button("OK", role: .cancel) { lastError = nil }
        } message: {
            Text(lastError ?? "")
        }
    }

    @ViewBuilder
    private var menuContent: some View {
        if let path, !editors.isEmpty {
            if let preferred {
                Button("Open in \(preferred.displayName)") {
                    open(path, with: preferred)
                }
                if editors.count > 1 {
                    Divider()
                    ForEach(editors.filter { $0.id != preferred.id }) { editor in
                        Button(editor.displayName) {
                            open(path, with: editor)
                        }
                    }
                }
            } else {
                ForEach(editors) { editor in
                    Button(editor.displayName) {
                        open(path, with: editor)
                    }
                }
            }
        } else if path == nil {
            Text("No project selected")
        } else {
            Text("No supported editor installed")
        }
    }

    private var pillLabel: some View {
        HStack(spacing: 6) {
            // Dark code tile — same idea as Codex’s filled editor glyph.
            ZStack {
                RoundedRectangle(cornerRadius: 5, style: .continuous)
                    .fill(tileFill)
                Image(systemName: "chevron.left.forwardslash.chevron.right")
                    .font(.system(size: 8.5, weight: .bold))
                    .foregroundStyle(tileGlyph)
            }
            .frame(width: 18, height: 18)

            if !compact {
                Text(preferred?.shortName ?? "Editor")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.primary.opacity(0.85))
                    .lineLimit(1)
            }

            Image(systemName: "chevron.down")
                .font(.system(size: 8, weight: .semibold))
                .foregroundStyle(.secondary)
        }
        .padding(.leading, 5)
        .padding(.trailing, 8)
        .frame(height: 28)
        .background(pillBackground, in: Capsule(style: .continuous))
        .overlay {
            Capsule(style: .continuous)
                .strokeBorder(pillStroke, lineWidth: 1)
        }
        .contentShape(Capsule(style: .continuous))
    }

    private var tileFill: Color {
        colorScheme == .dark
            ? Color.white.opacity(isHovering ? 0.92 : 0.86)
            : Color.black.opacity(isHovering ? 0.88 : 0.82)
    }

    private var tileGlyph: Color {
        colorScheme == .dark ? Color.black.opacity(0.88) : Color.white
    }

    private var pillBackground: Color {
        if isHovering {
            return Color.primary.opacity(colorScheme == .dark ? 0.12 : 0.06)
        }
        return Color.primary.opacity(colorScheme == .dark ? 0.08 : 0.035)
    }

    private var pillStroke: Color {
        Color.primary.opacity(colorScheme == .dark ? 0.18 : 0.12)
    }

    private var helpText: String {
        if path == nil { return "Open project in editor" }
        if editors.isEmpty {
            return "Install Cursor, VS Code, Zed, or another IDE to open projects"
        }
        if let preferred {
            return "Open in \(preferred.displayName)"
        }
        return "Open in editor"
    }

    private func open(_ path: String, with editor: InstalledExternalEditor) {
        do {
            try ExternalEditorLauncher.open(path: path, with: editor)
        } catch {
            lastError = error.localizedDescription
        }
    }
}

/// Context-menu section shared by files, diffs, and workspace rows.
@ViewBuilder
func openInEditorContextMenuItems(path: String) -> some View {
    let editors = ExternalEditorLauncher.installedEditors()
    if editors.isEmpty {
        Button("Open in Editor") {}
            .disabled(true)
    } else if editors.count == 1, let only = editors.first {
        Button("Open in \(only.displayName)") {
            _ = try? ExternalEditorLauncher.open(path: path, with: only)
        }
    } else {
        Menu("Open in Editor") {
            ForEach(editors) { editor in
                Button(editor.displayName) {
                    _ = try? ExternalEditorLauncher.open(path: path, with: editor)
                }
            }
        }
    }
}
