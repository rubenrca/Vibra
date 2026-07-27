import AppKit
import Foundation
import GhosttyTerminal
import GhosttyTheme

extension Notification.Name {
    /// Posted after terminal appearance settings change and live sessions should reconfigure.
    static let terminalAppearanceDidChange = Notification.Name("VibraTerminalAppearanceDidChange")
}

/// Persisted terminal look-and-feel: Vibra chrome, Ghostty config, or catalog themes.
struct TerminalAppearance: Equatable, Sendable {
    enum ThemeSource: String, CaseIterable, Identifiable, Sendable {
        /// Colors come from the user's Ghostty config file (if present); chrome matches.
        case ghostty
        /// Colors come from the bundled GhosttyTheme catalog (iTerm2 schemes).
        case catalog
        /// Colors match Vibra's built-in purple surface.
        case vibra

        var id: String { rawValue }

        var title: String {
            switch self {
            case .ghostty: "Ghostty"
            case .catalog: "Catalog"
            case .vibra: "Vibra"
            }
        }

        var detail: String {
            switch self {
            case .ghostty:
                "App chrome and terminal share your Ghostty config (or Ghostty defaults if unset)."
            case .catalog:
                "App chrome and terminal share the selected catalog themes. Ghostty keybinds still load when enabled."
            case .vibra:
                "App chrome and terminal use Vibra’s built-in purple surface instead of Ghostty."
            }
        }
    }

    enum CursorStyleSetting: String, CaseIterable, Identifiable, Sendable {
        case block
        case bar
        case underline

        var id: String { rawValue }

        var title: String {
            switch self {
            case .block: "Block"
            case .bar: "Bar"
            case .underline: "Underline"
            }
        }

        var terminalStyle: TerminalCursorStyle {
            switch self {
            case .block: .block
            case .bar: .bar
            case .underline: .underline
            }
        }

        init(terminalStyle: TerminalCursorStyle) {
            switch terminalStyle {
            case .block: self = .block
            case .bar: self = .bar
            case .underline: self = .underline
            }
        }
    }

    var themeSource: ThemeSource
    /// Catalog theme used when the system appearance is dark.
    var darkThemeName: String
    /// Catalog theme used when the system appearance is light.
    var lightThemeName: String
    /// Empty means do not override font-family (Ghostty config / system default).
    var fontFamily: String
    var fontSize: Double
    var cursorStyle: CursorStyleSetting
    var cursorBlink: Bool
    /// 0…1. Values below 1 enable translucent terminal backgrounds when the compositor allows it.
    var backgroundOpacity: Double
    /// When true, still load `~/.config/ghostty/config` as the base config (keybinds, shell, etc.).
    var loadGhosttyConfig: Bool

    static let defaultDarkThemeName = "Catppuccin Mocha"
    static let defaultLightThemeName = "Catppuccin Latte"
    static let defaultFontSize: Double = 13
    static let minFontSize: Double = 9
    static let maxFontSize: Double = 28

    static let featuredThemeNames: [String] = [
        "Catppuccin Mocha",
        "Catppuccin Latte",
        "Catppuccin Macchiato",
        "Catppuccin Frappe",
        "Dracula",
        "Nord",
        "Nord Light",
        "TokyoNight",
        "TokyoNight Day",
        "TokyoNight Storm",
        "Gruvbox Dark",
        "Gruvbox Light",
        "Atom One Dark",
        "GitHub Dark",
        "Afterglow",
        "Alabaster",
        "iTerm2 Solarized Dark",
        "iTerm2 Solarized Light",
    ]

    static let suggestedFontFamilies: [String] = [
        "SF Mono",
        "Menlo",
        "Monaco",
        "JetBrains Mono",
        "Cascadia Code",
        "Fira Code",
        "Hack",
        "Source Code Pro",
        "IBM Plex Mono",
        "Geist Mono",
    ]

    static var `default`: TerminalAppearance {
        TerminalAppearance(
            themeSource: .ghostty,
            darkThemeName: defaultDarkThemeName,
            lightThemeName: defaultLightThemeName,
            fontFamily: "",
            fontSize: defaultFontSize,
            cursorStyle: .block,
            cursorBlink: true,
            backgroundOpacity: 1,
            loadGhosttyConfig: true
        )
    }

    /// Theme tuned to Vibra’s dark/light chrome (sidebars + canvas), not a stock terminal scheme.
    static var vibraChromeTheme: TerminalTheme {
        TerminalTheme(light: vibraLightConfiguration, dark: vibraDarkConfiguration)
    }

    /// Near-black surface matching the app sidebars in dark mode, with purple accent cursor.
    static var vibraDarkConfiguration: TerminalConfiguration {
        TerminalConfiguration { builder in
            // Matches the dark chrome around the terminal (sidebars / title area).
            builder.withBackground("141416")
            builder.withForeground("E8E8ED")
            builder.withCursorColor("7D5CF5") // Vibra accent
            builder.withCursorText("141416")
            builder.withSelectionBackground("3A3260")
            builder.withSelectionForeground("F2F0FF")
            builder.withPalette(0, color: "#1C1C1E")
            builder.withPalette(1, color: "#FF6B6B")
            builder.withPalette(2, color: "#6BCB77")
            builder.withPalette(3, color: "#E8C468")
            builder.withPalette(4, color: "#7D9CFF")
            builder.withPalette(5, color: "#B794F6")
            builder.withPalette(6, color: "#6BCBDB")
            builder.withPalette(7, color: "#D1D1D6")
            builder.withPalette(8, color: "#636366")
            builder.withPalette(9, color: "#FF8A8A")
            builder.withPalette(10, color: "#85D992")
            builder.withPalette(11, color: "#F0D58A")
            builder.withPalette(12, color: "#9BB3FF")
            builder.withPalette(13, color: "#C9A8FF")
            builder.withPalette(14, color: "#8ADAE8")
            builder.withPalette(15, color: "#F2F2F7")
        }
    }

    /// Light chrome surface matching macOS window background with the same accent.
    static var vibraLightConfiguration: TerminalConfiguration {
        TerminalConfiguration { builder in
            builder.withBackground("F2F2F7")
            builder.withForeground("1C1C1E")
            builder.withCursorColor("7D5CF5")
            builder.withCursorText("F2F2F7")
            builder.withSelectionBackground("DDD6FF")
            builder.withSelectionForeground("1C1C1E")
            builder.withPalette(0, color: "#1C1C1E")
            builder.withPalette(1, color: "#D70015")
            builder.withPalette(2, color: "#248A3D")
            builder.withPalette(3, color: "#B25000")
            builder.withPalette(4, color: "#0040DD")
            builder.withPalette(5, color: "#8944AB")
            builder.withPalette(6, color: "#0071A4")
            builder.withPalette(7, color: "#8E8E93")
            builder.withPalette(8, color: "#636366")
            builder.withPalette(9, color: "#FF453A")
            builder.withPalette(10, color: "#30D158")
            builder.withPalette(11, color: "#FFD60A")
            builder.withPalette(12, color: "#0A84FF")
            builder.withPalette(13, color: "#BF5AF2")
            builder.withPalette(14, color: "#64D2FF")
            builder.withPalette(15, color: "#F2F2F7")
        }
    }

    static var current: TerminalAppearance {
        read(from: .standard)
    }

    /// One-shot: default the whole app to Match Ghostty (chrome + terminal).
    /// Skips users who already chose catalog or Vibra explicitly after this migration.
    static func migrateToMatchGhosttyIfNeeded(defaults: UserDefaults = .standard) {
        let flag = SettingsKeys.terminalMatchGhosttyMigrated
        guard !defaults.bool(forKey: flag) else { return }
        defaults.set(true, forKey: flag)

        let sourceRaw = defaults.string(forKey: SettingsKeys.terminalThemeSource)
        // Only rewrite the previous vibra-forced default; leave catalog alone.
        if sourceRaw == ThemeSource.catalog.rawValue {
            return
        }

        var appearance = read(from: defaults)
        appearance.themeSource = .ghostty
        appearance.loadGhosttyConfig = true
        appearance.write(to: defaults, notify: false)
    }

    /// Legacy migration kept for older builds that forced Match app.
    static func migrateToMatchAppIfNeeded(defaults: UserDefaults = .standard) {
        migrateToMatchGhosttyIfNeeded(defaults: defaults)
    }

    static func read(from defaults: UserDefaults) -> TerminalAppearance {
        let fallback = TerminalAppearance.default
        let sourceRaw = defaults.string(forKey: SettingsKeys.terminalThemeSource)
        let themeSource = ThemeSource(rawValue: sourceRaw ?? "") ?? fallback.themeSource

        let dark = defaults.string(forKey: SettingsKeys.terminalThemeDark)
            ?? fallback.darkThemeName
        let light = defaults.string(forKey: SettingsKeys.terminalThemeLight)
            ?? fallback.lightThemeName

        let fontSizeValue = defaults.object(forKey: SettingsKeys.terminalFontSize) as? Double
            ?? fallback.fontSize
        let clampedFontSize = min(max(fontSizeValue, minFontSize), maxFontSize)

        let cursorRaw = defaults.string(forKey: SettingsKeys.terminalCursorStyle)
        let cursorStyle = CursorStyleSetting(rawValue: cursorRaw ?? "") ?? fallback.cursorStyle

        let opacityValue = defaults.object(forKey: SettingsKeys.terminalBackgroundOpacity) as? Double
            ?? fallback.backgroundOpacity
        let clampedOpacity = min(max(opacityValue, 0.2), 1)

        let loadGhostty: Bool
        if defaults.object(forKey: SettingsKeys.terminalLoadGhosttyConfig) == nil {
            loadGhostty = fallback.loadGhosttyConfig
        } else {
            loadGhostty = defaults.bool(forKey: SettingsKeys.terminalLoadGhosttyConfig)
        }

        let blink: Bool
        if defaults.object(forKey: SettingsKeys.terminalCursorBlink) == nil {
            blink = fallback.cursorBlink
        } else {
            blink = defaults.bool(forKey: SettingsKeys.terminalCursorBlink)
        }

        return TerminalAppearance(
            themeSource: themeSource,
            darkThemeName: dark,
            lightThemeName: light,
            fontFamily: defaults.string(forKey: SettingsKeys.terminalFontFamily) ?? "",
            fontSize: clampedFontSize,
            cursorStyle: cursorStyle,
            cursorBlink: blink,
            backgroundOpacity: clampedOpacity,
            loadGhosttyConfig: loadGhostty
        )
    }

    func write(to defaults: UserDefaults = .standard, notify: Bool = true) {
        defaults.set(themeSource.rawValue, forKey: SettingsKeys.terminalThemeSource)
        defaults.set(darkThemeName, forKey: SettingsKeys.terminalThemeDark)
        defaults.set(lightThemeName, forKey: SettingsKeys.terminalThemeLight)
        defaults.set(fontFamily, forKey: SettingsKeys.terminalFontFamily)
        defaults.set(fontSize, forKey: SettingsKeys.terminalFontSize)
        defaults.set(cursorStyle.rawValue, forKey: SettingsKeys.terminalCursorStyle)
        defaults.set(cursorBlink, forKey: SettingsKeys.terminalCursorBlink)
        defaults.set(backgroundOpacity, forKey: SettingsKeys.terminalBackgroundOpacity)
        defaults.set(loadGhosttyConfig, forKey: SettingsKeys.terminalLoadGhosttyConfig)
        if notify {
            NotificationCenter.default.post(name: .terminalAppearanceDidChange, object: nil)
        }
    }

    // MARK: - Resolution for GhosttyTerminal

    var configSource: TerminalController.ConfigSource {
        guard loadGhosttyConfig, let path = GhosttyConfigLocator.path() else {
            return .none
        }
        return .file(path)
    }

    var terminalTheme: TerminalTheme {
        switch themeSource {
        case .vibra:
            return Self.vibraChromeTheme
        case .ghostty:
            // Same resolved surfaces as the app chrome so both stay in lockstep.
            return TerminalTheme(
                light: ghosttySurfaceConfiguration(isDark: false),
                dark: ghosttySurfaceConfiguration(isDark: true)
            )
        case .catalog:
            let dark = resolvedTheme(named: darkThemeName, fallbackName: Self.defaultDarkThemeName)
            let light = resolvedTheme(named: lightThemeName, fallbackName: Self.defaultLightThemeName)
            return TerminalTheme(
                light: light.toTerminalConfiguration(),
                dark: dark.toTerminalConfiguration()
            )
        }
    }

    /// Colors for the terminal surface under Ghostty source (config theme + key overrides).
    private func ghosttySurfaceConfiguration(isDark: Bool) -> TerminalConfiguration {
        let values: GhosttyConfigValues
        if loadGhosttyConfig, let path = GhosttyConfigLocator.path(),
           let contents = try? String(contentsOfFile: path, encoding: .utf8) {
            values = GhosttyConfigParser.parse(contents)
        } else {
            values = GhosttyConfigValues()
        }

        var base: TerminalConfiguration
        if let themeName = values.themeName(forDark: isDark),
           let definition = GhosttyThemeCatalog.theme(named: themeName)
            ?? GhosttyThemeCatalog.search(themeName).first {
            base = definition.toTerminalConfiguration()
        } else {
            base = isDark ? .afterglow : .alabaster
        }

        return TerminalConfiguration(startingFrom: base) { builder in
            if let background = values.background {
                builder.withBackground(AppChromeTheme.normalizeHex(background))
            }
            if let foreground = values.foreground {
                builder.withForeground(AppChromeTheme.normalizeHex(foreground))
            }
            if let cursor = values.cursorColor {
                builder.withCursorColor(AppChromeTheme.normalizeHex(cursor))
            }
            if let selection = values.selectionBackground {
                builder.withSelectionBackground(AppChromeTheme.normalizeHex(selection))
            }
            for (index, color) in values.palette.sorted(by: { $0.key < $1.key }) {
                builder.withPalette(index, color: "#\(AppChromeTheme.normalizeHex(color))")
            }
        }
    }

    var terminalConfiguration: TerminalConfiguration {
        TerminalConfiguration { builder in
            builder.withCustom("scrollback-limit", "1048576")
            builder.withCustom("macos-option-as-alt", "true")

            let trimmedFamily = fontFamily.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmedFamily.isEmpty {
                builder.withFontFamily(trimmedFamily)
            }
            // Force POSIX decimals so locales that use "," do not break Ghostty's config parser.
            builder.withCustom("font-size", posixNumber(fontSize, fractionDigits: 0...2))
            builder.withCursorStyle(cursorStyle.terminalStyle)
            builder.withCursorStyleBlink(cursorBlink)

            if backgroundOpacity < 0.999 {
                builder.withCustom(
                    "background-opacity",
                    posixNumber(backgroundOpacity, fractionDigits: 0...3)
                )
                // Ghostty uses integer blur radius; a light blur reads better on translucent panes.
                builder.withBackgroundBlur(20)
            }
        }
    }

    private func posixNumber(_ value: Double, fractionDigits: ClosedRange<Int>) -> String {
        let formatter = NumberFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.minimumFractionDigits = fractionDigits.lowerBound
        formatter.maximumFractionDigits = fractionDigits.upperBound
        formatter.numberStyle = .decimal
        return formatter.string(from: NSNumber(value: value))
            ?? String(format: "%.\(fractionDigits.upperBound)f", value)
    }

    func darkDefinition() -> GhosttyThemeDefinition? {
        GhosttyThemeCatalog.theme(named: darkThemeName)
            ?? GhosttyThemeCatalog.theme(named: Self.defaultDarkThemeName)
    }

    func lightDefinition() -> GhosttyThemeDefinition? {
        GhosttyThemeCatalog.theme(named: lightThemeName)
            ?? GhosttyThemeCatalog.theme(named: Self.defaultLightThemeName)
    }

    private func resolvedTheme(named name: String, fallbackName: String) -> GhosttyThemeDefinition {
        if let theme = GhosttyThemeCatalog.theme(named: name) {
            return theme
        }
        if let fallback = GhosttyThemeCatalog.theme(named: fallbackName) {
            return fallback
        }
        // Catalog is always non-empty in production; keep a hard fallback for safety.
        return GhosttyThemeCatalog.allThemes.first ?? GhosttyThemeDefinition(
            name: "Fallback",
            background: "1e1e2e",
            foreground: "cdd6f4"
        )
    }
}

@MainActor
enum TerminalAppearanceApplier {
    static func apply(_ appearance: TerminalAppearance, to session: TerminalSession) {
        session.applyAppearance(appearance)
    }

    static func apply(_ appearance: TerminalAppearance, to sessions: [TerminalSession]) {
        for session in sessions {
            apply(appearance, to: session)
        }
    }
}
