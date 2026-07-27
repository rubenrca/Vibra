import AppKit
import Foundation
import GhosttyTerminal
import GhosttyTheme
import SwiftUI

/// Shared surface colors for app chrome + terminal, resolved from the active appearance source.
struct AppChromeTheme: Equatable, Sendable, Identifiable {
    var id: String { "\(backgroundHex)-\(foregroundHex)-\(accentHex)-\(isDark)" }

    var backgroundHex: String
    var foregroundHex: String
    var accentHex: String
    var selectionHex: String
    var isDark: Bool

    var background: Color { Color(vibraHex: backgroundHex) ?? .black }
    var foreground: Color { Color(vibraHex: foregroundHex) ?? .white }
    var accent: Color { Color(vibraHex: accentHex) ?? VibraBrand.accent }
    var selection: Color { Color(vibraHex: selectionHex) ?? accent.opacity(0.35) }

    /// Slightly lifted surface for cards/headers on top of the base background.
    var elevated: Color {
        isDark
            ? background.mix(with: .white, amount: 0.05)
            : background.mix(with: .black, amount: 0.04)
    }

    var secondaryForeground: Color {
        foreground.opacity(isDark ? 0.62 : 0.58)
    }

    /// Resolve chrome colors for the current appearance and system color scheme.
    static func resolve(
        appearance: TerminalAppearance = .current,
        colorScheme: ColorScheme,
        ghosttyConfigPath: String? = GhosttyConfigLocator.path(),
        fileManager: FileManager = .default
    ) -> AppChromeTheme {
        let isDark = colorScheme == .dark
        switch appearance.themeSource {
        case .vibra:
            return from(
                configuration: isDark
                    ? TerminalAppearance.vibraDarkConfiguration
                    : TerminalAppearance.vibraLightConfiguration,
                isDark: isDark,
                fallbackAccent: VibraBrand.accentHex
            )
        case .catalog:
            let definition = isDark
                ? appearance.darkDefinition()
                : appearance.lightDefinition()
            if let definition {
                return from(definition: definition, isDark: isDark)
            }
            return ghosttyBuiltinFallback(isDark: isDark)
        case .ghostty:
            return resolveGhostty(
                isDark: isDark,
                configPath: appearance.loadGhosttyConfig ? ghosttyConfigPath : nil,
                fileManager: fileManager
            )
        }
    }

    private static func resolveGhostty(
        isDark: Bool,
        configPath: String?,
        fileManager: FileManager
    ) -> AppChromeTheme {
        var values = GhosttyConfigValues()
        if let configPath, fileManager.fileExists(atPath: configPath),
           let contents = try? String(contentsOfFile: configPath, encoding: .utf8) {
            values = GhosttyConfigParser.parse(contents)
        }

        // Prefer an explicit `theme =` (or dark/light pair), then overlay explicit color keys.
        var base: AppChromeTheme?
        if let themeName = values.themeName(forDark: isDark),
           let definition = GhosttyThemeCatalog.theme(named: themeName)
            ?? GhosttyThemeCatalog.search(themeName).first {
            base = from(definition: definition, isDark: isDark)
        }

        var theme = base ?? ghosttyBuiltinFallback(isDark: isDark)

        if let background = values.background {
            theme.backgroundHex = normalizeHex(background)
        }
        if let foreground = values.foreground {
            theme.foregroundHex = normalizeHex(foreground)
        }
        if let cursor = values.cursorColor {
            theme.accentHex = normalizeHex(cursor)
        } else if let palette5 = values.palette[5] {
            theme.accentHex = normalizeHex(palette5)
        }
        if let selection = values.selectionBackground {
            theme.selectionHex = normalizeHex(selection)
        } else {
            theme.selectionHex = theme.accentHex
        }
        theme.isDark = isDark
        return theme
    }

    static func from(definition: GhosttyThemeDefinition, isDark: Bool) -> AppChromeTheme {
        let accent = definition.cursorColor
            ?? definition.palette[5]
            ?? (isDark ? VibraBrand.accentHex : VibraBrand.accentHex)
        let selection = definition.selectionBackground ?? accent
        return AppChromeTheme(
            backgroundHex: normalizeHex(definition.background),
            foregroundHex: normalizeHex(definition.foreground),
            accentHex: normalizeHex(accent),
            selectionHex: normalizeHex(selection),
            isDark: isDark
        )
    }

    static func from(
        configuration: TerminalConfiguration,
        isDark: Bool,
        fallbackAccent: String
    ) -> AppChromeTheme {
        let map = configurationValueMap(configuration.rendered)
        return AppChromeTheme(
            backgroundHex: normalizeHex(map["background"] ?? (isDark ? "141416" : "F2F2F7")),
            foregroundHex: normalizeHex(map["foreground"] ?? (isDark ? "E8E8ED" : "1C1C1E")),
            accentHex: normalizeHex(map["cursor-color"] ?? fallbackAccent),
            selectionHex: normalizeHex(map["selection-background"] ?? map["cursor-color"] ?? fallbackAccent),
            isDark: isDark
        )
    }

    /// Ghostty / libghostty default when config has no theme or colors (Afterglow / Alabaster).
    static func ghosttyBuiltinFallback(isDark: Bool) -> AppChromeTheme {
        from(
            configuration: isDark ? .afterglow : .alabaster,
            isDark: isDark,
            fallbackAccent: isDark ? "D0D0D0" : "007ACC"
        )
    }

    private static func configurationValueMap(_ rendered: String) -> [String: String] {
        var map: [String: String] = [:]
        for line in rendered.split(separator: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard let eq = trimmed.firstIndex(of: "=") else { continue }
            let key = trimmed[..<eq].trimmingCharacters(in: .whitespaces)
            let value = trimmed[trimmed.index(after: eq)...].trimmingCharacters(in: .whitespaces)
            map[key] = value
        }
        return map
    }

    static func normalizeHex(_ raw: String) -> String {
        var value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if value.hasPrefix("#") { value.removeFirst() }
        return value.uppercased()
    }
}

// MARK: - Ghostty config parsing

struct GhosttyConfigValues: Equatable, Sendable {
    var background: String?
    var foreground: String?
    var cursorColor: String?
    var selectionBackground: String?
    var theme: String?
    var palette: [Int: String] = [:]

    func themeName(forDark isDark: Bool) -> String? {
        guard let theme, !theme.isEmpty else { return nil }
        // `dark:Name,light:Name` or `light:Name,dark:Name`
        if theme.contains("dark:") || theme.contains("light:") {
            let parts = theme.split(separator: ",").map {
                $0.trimmingCharacters(in: .whitespacesAndNewlines)
            }
            let prefix = isDark ? "dark:" : "light:"
            if let match = parts.first(where: { $0.lowercased().hasPrefix(prefix) }) {
                return String(match.dropFirst(prefix.count)).trimmingCharacters(in: .whitespaces)
            }
            // Fall back to the other half if only one is specified.
            let other = isDark ? "light:" : "dark:"
            if let match = parts.first(where: { $0.lowercased().hasPrefix(other) }) {
                return String(match.dropFirst(other.count)).trimmingCharacters(in: .whitespaces)
            }
            return nil
        }
        return theme
    }
}

enum GhosttyConfigParser {
    static func parse(_ contents: String) -> GhosttyConfigValues {
        var values = GhosttyConfigValues()
        for rawLine in contents.split(separator: "\n", omittingEmptySubsequences: false) {
            var line = String(rawLine)
            // Strip UTF-8 BOM if present on the first line.
            if line.first == "\u{FEFF}" { line.removeFirst() }
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty, !trimmed.hasPrefix("#") else { continue }
            guard let eq = trimmed.firstIndex(of: "=") else { continue }
            let key = trimmed[..<eq]
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .lowercased()
            var value = trimmed[trimmed.index(after: eq)...]
                .trimmingCharacters(in: .whitespacesAndNewlines)
            // Unquote simple "..." or '...' values.
            if value.count >= 2,
               (value.hasPrefix("\"") && value.hasSuffix("\""))
                || (value.hasPrefix("'") && value.hasSuffix("'")) {
                value = String(value.dropFirst().dropLast())
            }
            guard !value.isEmpty else { continue }

            switch key {
            case "background":
                values.background = value
            case "foreground":
                values.foreground = value
            case "cursor-color":
                values.cursorColor = value
            case "selection-background":
                values.selectionBackground = value
            case "theme":
                values.theme = value
            case "palette":
                // palette = 1=#ff0000  or  palette = 1=ff0000
                if let eqIdx = value.firstIndex(of: "=") {
                    let indexPart = value[..<eqIdx].trimmingCharacters(in: .whitespaces)
                    let colorPart = value[value.index(after: eqIdx)...]
                        .trimmingCharacters(in: .whitespaces)
                    if let index = Int(indexPart) {
                        values.palette[index] = String(colorPart)
                    }
                }
            default:
                break
            }
        }
        return values
    }
}

// MARK: - Controller

@MainActor
final class ChromeThemeController: ObservableObject {
    static let shared = ChromeThemeController()

    @Published private(set) var theme: AppChromeTheme

    nonisolated(unsafe) private var appearanceObserver: NSObjectProtocol?

    init() {
        // Do not touch NSApp here — `shared` can be created from App.init before
        // the application object exists, and NSApp is an IUO that traps when nil.
        let scheme = Self.preferredColorScheme()
        theme = AppChromeTheme.resolve(colorScheme: scheme)
        appearanceObserver = NotificationCenter.default.addObserver(
            forName: .terminalAppearanceDidChange,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.refresh()
            }
        }
    }

    deinit {
        if let appearanceObserver {
            NotificationCenter.default.removeObserver(appearanceObserver)
        }
    }

    func refresh(colorScheme: ColorScheme? = nil) {
        let scheme = colorScheme ?? Self.preferredColorScheme()
        let resolved = AppChromeTheme.resolve(colorScheme: scheme)
        if resolved != theme {
            theme = resolved
        }
    }

    /// Safe during static/App init: never force-unwrap `NSApp`.
    private static func preferredColorScheme() -> ColorScheme {
        if let app = NSApplication.shared as NSApplication? {
            // `shared` is always a real instance once AppKit is linked; still
            // prefer effectiveAppearance only after the app has finished launching
            // so we do not depend on partially-initialized UI state.
            if app.isRunning {
                let matched = app.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua])
                return matched == .darkAqua ? .dark : .light
            }
        }
        // AppleInterfaceStyle is set by the system for Dark Mode; absent means Light.
        if UserDefaults.standard.string(forKey: "AppleInterfaceStyle") == "Dark" {
            return .dark
        }
        return .light
    }
}

// MARK: - Brand + color helpers

enum VibraBrand {
    static let accentHex = "7D5CF5"
    static let accent = Color(red: 0.49, green: 0.36, blue: 0.96)
}

extension Color {
    init?(vibraHex: String) {
        var cleaned = vibraHex.trimmingCharacters(in: .whitespacesAndNewlines)
        if cleaned.hasPrefix("#") { cleaned.removeFirst() }
        guard cleaned.count == 6 || cleaned.count == 8 else { return nil }
        var value: UInt64 = 0
        guard Scanner(string: cleaned).scanHexInt64(&value) else { return nil }

        let hasAlpha = cleaned.count == 8
        let a = hasAlpha ? Double((value & 0xFF00_0000) >> 24) / 255 : 1
        let r = Double((value & 0x00FF_0000) >> 16) / 255
        let g = Double((value & 0x0000_FF00) >> 8) / 255
        let b = Double(value & 0x0000_00FF) / 255
        self.init(.sRGB, red: r, green: g, blue: b, opacity: a)
    }

    /// Linear mix toward another color (UI-only approximation).
    func mix(with other: Color, amount: Double) -> Color {
        let t = min(max(amount, 0), 1)
        #if canImport(AppKit)
        let nsSelf = NSColor(self).usingColorSpace(.sRGB) ?? NSColor(self)
        let nsOther = NSColor(other).usingColorSpace(.sRGB) ?? NSColor(other)
        var r1: CGFloat = 0, g1: CGFloat = 0, b1: CGFloat = 0, a1: CGFloat = 0
        var r2: CGFloat = 0, g2: CGFloat = 0, b2: CGFloat = 0, a2: CGFloat = 0
        nsSelf.getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
        nsOther.getRed(&r2, green: &g2, blue: &b2, alpha: &a2)
        return Color(
            red: Double(r1 + (r2 - r1) * t),
            green: Double(g1 + (g2 - g1) * t),
            blue: Double(b1 + (b2 - b1) * t),
            opacity: Double(a1 + (a2 - a1) * t)
        )
        #else
        return self
        #endif
    }
}

private struct AppChromeThemeKey: EnvironmentKey {
    static let defaultValue = AppChromeTheme.ghosttyBuiltinFallback(isDark: true)
}

extension EnvironmentValues {
    var appChrome: AppChromeTheme {
        get { self[AppChromeThemeKey.self] }
        set { self[AppChromeThemeKey.self] = newValue }
    }
}
