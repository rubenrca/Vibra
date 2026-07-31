import AppKit
import GhosttyTerminal
import GhosttyTheme
import SwiftUI

enum SettingsKeys {
    static let projectSidebarVisible = "projectSidebarVisible"
    static let terminalSidebarWidth = "terminalSidebarWidth"
    static let gitSidebarWidth = "gitSidebarWidth"
    static let gitSidebarVisible = "gitSidebarVisible"
    static let cmuxShortcutsEnabled = "cmuxShortcutsEnabled"
    static let gitAutoRefreshEnabled = "gitAutoRefreshEnabled"
    static let gitRefreshDelay = "gitRefreshDelay"
    static let preferredExternalEditor = "preferredExternalEditor"
    static let tabFolderModelMigrated = "tabFolderModelMigrated"

    static let terminalThemeSource = "terminalThemeSource"
    static let terminalThemeDark = "terminalThemeDark"
    static let terminalThemeLight = "terminalThemeLight"
    static let terminalFontFamily = "terminalFontFamily"
    static let terminalFontSize = "terminalFontSize"
    static let terminalCursorStyle = "terminalCursorStyle"
    static let terminalCursorBlink = "terminalCursorBlink"
    static let terminalBackgroundOpacity = "terminalBackgroundOpacity"
    static let terminalLoadGhosttyConfig = "terminalLoadGhosttyConfig"
    static let terminalMatchAppMigrated = "terminalMatchAppMigrated"
    static let terminalMatchGhosttyMigrated = "terminalMatchGhosttyMigrated"

    @MainActor static let defaults: [String: Any] = [
        cmuxShortcutsEnabled: true,
        gitAutoRefreshEnabled: true,
        gitRefreshDelay: 420,
        terminalThemeSource: TerminalAppearance.ThemeSource.ghostty.rawValue,
        terminalThemeDark: TerminalAppearance.defaultDarkThemeName,
        terminalThemeLight: TerminalAppearance.defaultLightThemeName,
        terminalFontFamily: "",
        terminalFontSize: TerminalAppearance.defaultFontSize,
        terminalCursorStyle: TerminalAppearance.CursorStyleSetting.block.rawValue,
        terminalCursorBlink: true,
        terminalBackgroundOpacity: 1.0,
        terminalLoadGhosttyConfig: true,
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

                ExternalEditorSettingsSection()
            }
            .formStyle(.grouped)
            .tabItem { Label("General", systemImage: "gearshape") }

            AppearanceSettingsView()
                .tabItem { Label("Appearance", systemImage: "paintpalette") }

            ShortcutReferenceView()
                .tabItem { Label("Shortcuts", systemImage: "keyboard") }
        }
        .frame(width: 560, height: 560)
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

// MARK: - External editor

private struct ExternalEditorSettingsSection: View {
    @State private var preferredRaw: String = {
        UserDefaults.standard.string(forKey: SettingsKeys.preferredExternalEditor) ?? ""
    }()
    @State private var installed: [InstalledExternalEditor] = ExternalEditorLauncher.installedEditors()

    var body: some View {
        Section {
            if installed.isEmpty {
                Text("No supported editor found. Install Cursor, VS Code, Zed, Windsurf, or Xcode.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                Picker("Preferred editor", selection: $preferredRaw) {
                    Text("First available").tag("")
                    ForEach(installed) { editor in
                        Text(editor.displayName).tag(editor.kind.rawValue)
                    }
                }
                .onChange(of: preferredRaw) { _, newValue in
                    if newValue.isEmpty {
                        ExternalEditorLauncher.setPreferredKind(nil)
                    } else if let kind = KnownExternalEditor(rawValue: newValue) {
                        ExternalEditorLauncher.setPreferredKind(kind)
                    }
                }
            }
        } header: {
            Text("External Editor")
        } footer: {
            Text("Used by Open in Editor in the session header, File menu, and context menus.")
        }
        .onAppear {
            installed = ExternalEditorLauncher.installedEditors()
            preferredRaw = UserDefaults.standard.string(forKey: SettingsKeys.preferredExternalEditor) ?? ""
            // Drop a stale preference if that app was uninstalled.
            if !preferredRaw.isEmpty,
               !installed.contains(where: { $0.kind.rawValue == preferredRaw }) {
                preferredRaw = ""
                ExternalEditorLauncher.setPreferredKind(nil)
            }
        }
    }
}

// MARK: - Appearance

private struct AppearanceSettingsView: View {
    @ObservedObject private var chromeTheme = ChromeThemeController.shared
    @State private var appearance = TerminalAppearance.current
    @State private var themeQuery = ""
    @State private var themePickerTarget: ThemePickerTarget = .dark

    private enum ThemePickerTarget: String, CaseIterable, Identifiable {
        case dark
        case light

        var id: String { rawValue }

        var title: String {
            switch self {
            case .dark: "Dark"
            case .light: "Light"
            }
        }
    }

    private var ghosttyConfigPath: String? {
        GhosttyConfigLocator.path()
    }

    private var filteredThemes: [GhosttyThemeDefinition] {
        let query = themeQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        if query.isEmpty {
            return GhosttyThemeCatalog.allThemes
        }
        return GhosttyThemeCatalog.search(query)
    }

    private var featuredThemes: [GhosttyThemeDefinition] {
        TerminalAppearance.featuredThemeNames.compactMap { GhosttyThemeCatalog.theme(named: $0) }
    }

    var body: some View {
        Form {
            Section {
                Picker("Color source", selection: $appearance.themeSource) {
                    ForEach(TerminalAppearance.ThemeSource.allCases) { source in
                        Text(source.title).tag(source)
                    }
                }
                .pickerStyle(.segmented)

                Toggle("Load Ghostty config file as base", isOn: $appearance.loadGhosttyConfig)
                if let ghosttyConfigPath {
                    Text(ghosttyConfigPath)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                } else {
                    Text("No Ghostty config found. Vibra will use built-in defaults for the base layer.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } header: {
                Text("Theme source")
            } footer: {
                Text(appearance.themeSource.detail)
            }

            if appearance.themeSource == .ghostty {
                Section("Ghostty surface") {
                    HStack(spacing: 14) {
                        chromePreviewSwatch(
                            title: "Dark",
                            theme: AppChromeTheme.resolve(appearance: appearance, colorScheme: .dark)
                        )
                        chromePreviewSwatch(
                            title: "Light",
                            theme: AppChromeTheme.resolve(appearance: appearance, colorScheme: .light)
                        )
                    }
                    .padding(.vertical, 4)
                    Text("Sidebars, headers, and the terminal all use this surface.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            if appearance.themeSource == .vibra {
                Section("Vibra surface") {
                    HStack(spacing: 14) {
                        vibraPreviewSwatch(title: "Dark", configuration: TerminalAppearance.vibraDarkConfiguration)
                        vibraPreviewSwatch(title: "Light", configuration: TerminalAppearance.vibraLightConfiguration)
                    }
                    .padding(.vertical, 4)
                    Text("Built-in purple surface when you do not want Ghostty colors on the chrome.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            if appearance.themeSource == .catalog {
                Section("Active themes") {
                    themeSummaryRow(
                        title: "Dark appearance",
                        name: appearance.darkThemeName,
                        definition: appearance.darkDefinition()
                    ) {
                        themePickerTarget = .dark
                    }
                    themeSummaryRow(
                        title: "Light appearance",
                        name: appearance.lightThemeName,
                        definition: appearance.lightDefinition()
                    ) {
                        themePickerTarget = .light
                    }
                }

                Section {
                    Picker("Editing", selection: $themePickerTarget) {
                        ForEach(ThemePickerTarget.allCases) { target in
                            Text(target.title).tag(target)
                        }
                    }
                    .pickerStyle(.segmented)

                    TextField("Search \(GhosttyThemeCatalog.allThemes.count) themes", text: $themeQuery)
                        .textFieldStyle(.roundedBorder)

                    if themeQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        themeChipRow(title: "Popular", themes: featuredThemes)
                    }

                    themeList
                } header: {
                    Text("Theme catalog")
                }
            }

            Section("Typography") {
                HStack {
                    TextField("Font family", text: $appearance.fontFamily)
                        .textFieldStyle(.roundedBorder)
                    Menu("Presets") {
                        Button("System default") {
                            appearance.fontFamily = ""
                        }
                        Divider()
                        ForEach(TerminalAppearance.suggestedFontFamilies, id: \.self) { family in
                            Button(family) {
                                appearance.fontFamily = family
                            }
                        }
                    }
                }
                Text("Leave empty to keep the font from Ghostty config or the system default.")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                HStack {
                    Text("Font size")
                    Spacer()
                    Text("\(Int(appearance.fontSize.rounded())) pt")
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
                Slider(
                    value: $appearance.fontSize,
                    in: TerminalAppearance.minFontSize...TerminalAppearance.maxFontSize,
                    step: 1
                )
            }

            Section("Cursor") {
                Picker("Style", selection: $appearance.cursorStyle) {
                    ForEach(TerminalAppearance.CursorStyleSetting.allCases) { style in
                        Text(style.title).tag(style)
                    }
                }
                Toggle("Blink", isOn: $appearance.cursorBlink)
            }

            Section("Background") {
                HStack {
                    Text("Opacity")
                    Spacer()
                    Text("\(Int((appearance.backgroundOpacity * 100).rounded()))%")
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
                Slider(value: $appearance.backgroundOpacity, in: 0.2...1, step: 0.05)
                Text("Values below 100% enable a translucent terminal surface.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onChange(of: appearance) { _, newValue in
            newValue.write()
        }
    }

    @ViewBuilder
    private var themeList: some View {
        let themes = filteredThemes
        if themes.isEmpty {
            Text("No themes match “\(themeQuery)”.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 8)
        } else {
            ScrollView {
                LazyVStack(spacing: 4) {
                    ForEach(themes) { theme in
                        themeRow(theme)
                    }
                }
            }
            .frame(minHeight: 180, maxHeight: 220)
        }
    }

    private func themeChipRow(title: String, themes: [GhosttyThemeDefinition]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(themes) { theme in
                        Button {
                            selectTheme(theme)
                        } label: {
                            HStack(spacing: 6) {
                                ThemeSwatch(theme: theme, size: 14)
                                Text(theme.name)
                                    .font(.system(size: 11, weight: .medium))
                                    .lineLimit(1)
                            }
                            .padding(.horizontal, 8)
                            .padding(.vertical, 6)
                            .background(
                                Color.primary.opacity(isSelected(theme) ? 0.12 : 0.05),
                                in: Capsule()
                            )
                            .overlay(
                                Capsule()
                                    .strokeBorder(
                                        isSelected(theme)
                                            ? chromeTheme.theme.accent.opacity(0.8)
                                            : Color.clear,
                                        lineWidth: 1
                                    )
                            )
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .padding(.vertical, 4)
    }

    private func themeSummaryRow(
        title: String,
        name: String,
        definition: GhosttyThemeDefinition?,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                if let definition {
                    ThemeSwatch(theme: definition, size: 22)
                } else {
                    RoundedRectangle(cornerRadius: 5)
                        .fill(Color.secondary.opacity(0.2))
                        .frame(width: 22, height: 22)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Text(name)
                        .font(.system(size: 12, weight: .medium))
                }
                Spacer()
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func themeRow(_ theme: GhosttyThemeDefinition) -> some View {
        Button {
            selectTheme(theme)
        } label: {
            HStack(spacing: 10) {
                ThemeSwatch(theme: theme, size: 20)
                Text(theme.name)
                    .font(.system(size: 12))
                    .lineLimit(1)
                Spacer()
                if isSelected(theme) {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(chromeTheme.theme.accent)
                        .font(.system(size: 13))
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(
                Color.primary.opacity(isSelected(theme) ? 0.08 : 0),
                in: RoundedRectangle(cornerRadius: 6)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func isSelected(_ theme: GhosttyThemeDefinition) -> Bool {
        switch themePickerTarget {
        case .dark: appearance.darkThemeName == theme.name
        case .light: appearance.lightThemeName == theme.name
        }
    }

    private func selectTheme(_ theme: GhosttyThemeDefinition) {
        switch themePickerTarget {
        case .dark:
            appearance.darkThemeName = theme.name
        case .light:
            appearance.lightThemeName = theme.name
        }
        if appearance.themeSource != .catalog {
            appearance.themeSource = .catalog
        }
    }

    private func vibraPreviewSwatch(title: String, configuration: TerminalConfiguration) -> some View {
        let bg = colorFromConfiguration(configuration, keyPrefix: "background = ") ?? .black
        let fg = colorFromConfiguration(configuration, keyPrefix: "foreground = ") ?? .white
        let accent = colorFromConfiguration(configuration, keyPrefix: "cursor-color = ")
            ?? Color(vibraHex: VibraBrand.accentHex)
            ?? VibraBrand.accent
        return surfacePreviewSwatch(title: title, background: bg, foreground: fg, accent: accent)
    }

    private func chromePreviewSwatch(title: String, theme: AppChromeTheme) -> some View {
        surfacePreviewSwatch(
            title: title,
            background: theme.background,
            foreground: theme.foreground,
            accent: theme.accent
        )
    }

    private func surfacePreviewSwatch(
        title: String,
        background: Color,
        foreground: Color,
        accent: Color
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
            ZStack(alignment: .bottomLeading) {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(background)
                HStack(spacing: 5) {
                    Capsule().fill(accent).frame(width: 10, height: 4)
                    Capsule().fill(foreground.opacity(0.85)).frame(width: 28, height: 4)
                    Capsule().fill(foreground.opacity(0.35)).frame(width: 16, height: 4)
                }
                .padding(10)
            }
            .frame(height: 52)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(Color.primary.opacity(0.1), lineWidth: 0.5)
            )
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func colorFromConfiguration(
        _ configuration: TerminalConfiguration,
        keyPrefix: String
    ) -> Color? {
        guard let line = configuration.rendered
            .split(separator: "\n")
            .map(String.init)
            .first(where: { $0.hasPrefix(keyPrefix) })
        else { return nil }
        let value = String(line.dropFirst(keyPrefix.count))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return Color(vibraHex: value)
    }
}

private struct ThemeSwatch: View {
    let theme: GhosttyThemeDefinition
    var size: CGFloat = 20

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                .fill(Color(vibraHex: theme.background) ?? .black)
            HStack(spacing: size * 0.08) {
                ForEach(accentIndices, id: \.self) { index in
                    Circle()
                        .fill(Color(vibraHex: theme.palette[index] ?? theme.foreground) ?? .white)
                        .frame(width: size * 0.18, height: size * 0.18)
                }
            }
        }
        .frame(width: size * 1.55, height: size)
        .overlay(
            RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                .strokeBorder(Color.primary.opacity(0.12), lineWidth: 0.5)
        )
    }

    private var accentIndices: [Int] { [1, 2, 4, 5] }
}



private struct ShortcutReferenceView: View {
    private let groups: [(String, [(String, String)])] = [
        ("Terminals", [
            ("New workspace", "⌘N"),
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
        ("Workspaces", [
            ("Close selected workspace", "⇧⌘W"),
            ("Jump to workspace", "⌘1–9"),
            ("Previous / next workspace", "⌃⌘[  ]"),
        ]),
        ("Sidebars", [
            ("Toggle terminals", "⌘B"),
            ("Toggle right sidebar", "⌘R"),
        ]),
        ("Project", [
            ("Open in editor", "⇧⌘E"),
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
                case "1": store.selectWorkspace(at: 0)
                case "2": store.selectWorkspace(at: 1)
                case "3": store.selectWorkspace(at: 2)
                case "4": store.selectWorkspace(at: 3)
                case "5": store.selectWorkspace(at: 4)
                case "6": store.selectWorkspace(at: 5)
                case "7": store.selectWorkspace(at: 6)
                case "8": store.selectWorkspace(at: 7)
                case "9": store.selectWorkspace(at: store.tabCount - 1)
                default: return false
                }
                return true
            }

            if flags == [.command, .shift] {
                switch key {
                case "n": newTerminalWindow()
                case "d": store.splitSelected(.vertical)
                case "w": store.closeSelectedWorkspace()
                default: return false
                }
                return true
            }

            if flags == [.command, .control] {
                switch key {
                case "[": store.selectAdjacentWorkspace(-1)
                case "]": store.selectAdjacentWorkspace(1)
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
