import Foundation
import GhosttyTheme
import SwiftUI
import Testing
@testable import VibraApp

@Test func terminalAppearanceDefaultsMatchGhostty() {
    let appearance = TerminalAppearance.default
    #expect(appearance.themeSource == .ghostty)
    #expect(appearance.terminalConfiguration.rendered.contains("scrollback-limit = 1048576"))
    #expect(appearance.terminalConfiguration.rendered.contains("font-size = 13"))
    #expect(appearance.terminalConfiguration.rendered.contains("cursor-style = block"))

    // Empty Ghostty config → Afterglow / Alabaster surfaces shared with chrome.
    let dark = appearance.terminalTheme.dark.rendered
    #expect(dark.contains("background = 212121") || dark.contains("background = #212121"))
    let light = appearance.terminalTheme.light.rendered
    #expect(light.contains("background = F7F7F7") || light.contains("background = #F7F7F7"))
}

@Test func terminalAppearanceCatalogThemeIncludesPalette() {
    var appearance = TerminalAppearance.default
    appearance.themeSource = .catalog
    appearance.darkThemeName = "Catppuccin Mocha"
    appearance.lightThemeName = "Catppuccin Latte"

    let theme = appearance.terminalTheme
    #expect(theme.dark.rendered.contains("background = 1e1e2e")
        || theme.dark.rendered.contains("background = #1e1e2e"))
    #expect(theme.light.rendered.contains("background = eff1f5")
        || theme.light.rendered.contains("background = #eff1f5"))
    #expect(theme.dark.rendered.contains("palette = 1="))
}

@Test func terminalAppearanceGhosttyConfigOverridesSurface() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("vibra-ghostty-theme-\(UUID().uuidString)", isDirectory: true)
    let config = root.appendingPathComponent("config")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }

    try """
    background = #101018
    foreground = #e0e0ff
    cursor-color = #ff88cc
    selection-background = #333355
    palette = 5=#aabbff
    """.write(to: config, atomically: true, encoding: .utf8)

    // Resolve chrome against the temp config path.
    var appearance = TerminalAppearance.default
    appearance.themeSource = .ghostty
    appearance.loadGhosttyConfig = true

    let chrome = AppChromeTheme.resolve(
        appearance: appearance,
        colorScheme: .dark,
        ghosttyConfigPath: config.path
    )
    #expect(chrome.backgroundHex == "101018")
    #expect(chrome.foregroundHex == "E0E0FF")
    #expect(chrome.accentHex == "FF88CC")
    #expect(chrome.selectionHex == "333355")
}

@Test func ghosttyConfigParserReadsThemePairAndPalette() {
    let values = GhosttyConfigParser.parse(
        """
        # comment
        theme = dark:Catppuccin Mocha,light:Catppuccin Latte
        background = 112233
        palette = 1=#ff0000
        palette = 4=00ff00
        """
    )
    #expect(values.themeName(forDark: true) == "Catppuccin Mocha")
    #expect(values.themeName(forDark: false) == "Catppuccin Latte")
    #expect(values.background == "112233")
    #expect(values.palette[1] == "#ff0000")
    #expect(values.palette[4] == "00ff00")
}

@Test func appChromeThemeMatchesCatalogDefinition() throws {
    let definition = try #require(GhosttyThemeCatalog.theme(named: "Dracula"))
    let chrome = AppChromeTheme.from(definition: definition, isDark: true)
    #expect(chrome.backgroundHex == AppChromeTheme.normalizeHex(definition.background))
    #expect(chrome.foregroundHex == AppChromeTheme.normalizeHex(definition.foreground))
}

@Test func terminalAppearancePersistsAndRestoresFromDefaults() {
    let suiteName = "vibra.appearance.tests.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suiteName)!
    defer { defaults.removePersistentDomain(forName: suiteName) }

    var appearance = TerminalAppearance.default
    appearance.themeSource = .catalog
    appearance.darkThemeName = "Dracula"
    appearance.lightThemeName = "Alabaster"
    appearance.fontFamily = "JetBrains Mono"
    appearance.fontSize = 15
    appearance.cursorStyle = .bar
    appearance.cursorBlink = false
    appearance.backgroundOpacity = 0.85
    appearance.loadGhosttyConfig = false
    appearance.write(to: defaults, notify: false)

    let restored = TerminalAppearance.read(from: defaults)
    #expect(restored == appearance)
    #expect(restored.terminalConfiguration.rendered.contains("font-family = JetBrains Mono"))
    #expect(restored.terminalConfiguration.rendered.contains("font-size = 15"))
    #expect(restored.terminalConfiguration.rendered.contains("cursor-style = bar"))
    #expect(restored.terminalConfiguration.rendered.contains("cursor-style-blink = false"))
    #expect(restored.terminalConfiguration.rendered.contains("background-opacity = 0.85"))
    #expect(restored.terminalConfiguration.rendered.contains("background-blur = 20"))
    #expect(restored.configSource == .none)
}

@Test func terminalAppearanceMigratesToMatchGhostty() {
    let suiteName = "vibra.appearance.migrate.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suiteName)!
    defer { defaults.removePersistentDomain(forName: suiteName) }

    defaults.set(TerminalAppearance.ThemeSource.vibra.rawValue, forKey: SettingsKeys.terminalThemeSource)
    TerminalAppearance.migrateToMatchGhosttyIfNeeded(defaults: defaults)

    #expect(defaults.bool(forKey: SettingsKeys.terminalMatchGhosttyMigrated))
    #expect(
        defaults.string(forKey: SettingsKeys.terminalThemeSource)
            == TerminalAppearance.ThemeSource.ghostty.rawValue
    )

    // Second run is a no-op even if the user later picks Vibra again.
    defaults.set(TerminalAppearance.ThemeSource.vibra.rawValue, forKey: SettingsKeys.terminalThemeSource)
    TerminalAppearance.migrateToMatchGhosttyIfNeeded(defaults: defaults)
    #expect(
        defaults.string(forKey: SettingsKeys.terminalThemeSource)
            == TerminalAppearance.ThemeSource.vibra.rawValue
    )
}

@Test func terminalAppearanceMigrationSkipsCatalogChoice() {
    let suiteName = "vibra.appearance.migrate.catalog.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suiteName)!
    defer { defaults.removePersistentDomain(forName: suiteName) }

    defaults.set(TerminalAppearance.ThemeSource.catalog.rawValue, forKey: SettingsKeys.terminalThemeSource)
    TerminalAppearance.migrateToMatchGhosttyIfNeeded(defaults: defaults)

    #expect(
        defaults.string(forKey: SettingsKeys.terminalThemeSource)
            == TerminalAppearance.ThemeSource.catalog.rawValue
    )
}

@Test func terminalAppearanceFeaturedThemesExistInCatalog() {
    for name in TerminalAppearance.featuredThemeNames {
        #expect(
            GhosttyThemeCatalog.theme(named: name) != nil,
            "Missing featured theme: \(name)"
        )
    }
}
