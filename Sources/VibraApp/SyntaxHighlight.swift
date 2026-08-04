import Foundation

enum SyntaxLanguage: String, Sendable {
    case swift
    case javascript
    case typescript
    case json
    case markdown
    case shell
    case plain
}

enum SyntaxTokenKind: Equatable, Sendable {
    case keyword
    case string
    case comment
    case number
    case type
    case plain
}

struct SyntaxToken: Equatable, Sendable {
    let kind: SyntaxTokenKind
    /// UTF-16 offset into the line (SwiftUI AttributedString friendly).
    let location: Int
    let length: Int
}

/// Lightweight, dependency-free syntax coloring for common source languages.
enum SyntaxHighlighter {
    nonisolated static func language(forPath path: String) -> SyntaxLanguage {
        let name = (path as NSString).lastPathComponent.lowercased()
        let ext = (name as NSString).pathExtension
        switch ext {
        case "swift": return .swift
        case "ts", "tsx": return .typescript
        case "js", "jsx", "mjs", "cjs": return .javascript
        case "json": return .json
        case "md", "markdown": return .markdown
        case "sh", "bash", "zsh", "fish": return .shell
        default:
            if name == "dockerfile" || name.hasPrefix("makefile") { return .shell }
            return .plain
        }
    }

    nonisolated static func tokens(in line: String, language: SyntaxLanguage) -> [SyntaxToken] {
        guard language != .plain, !line.isEmpty else {
            return [SyntaxToken(kind: .plain, location: 0, length: line.utf16.count)]
        }
        // Prefer comments and strings so keywords inside them stay dimmed.
        var claimed = Array(repeating: false, count: line.utf16.count)
        var result: [SyntaxToken] = []

        markPatterns(
            in: line,
            claimed: &claimed,
            result: &result,
            patterns: commentPatterns(for: language),
            kind: .comment
        )
        markPatterns(
            in: line,
            claimed: &claimed,
            result: &result,
            patterns: stringPatterns(for: language),
            kind: .string
        )
        markPatterns(
            in: line,
            claimed: &claimed,
            result: &result,
            patterns: [#"\b\d+(?:\.\d+)?\b"#],
            kind: .number
        )
        if language == .swift || language == .typescript || language == .javascript {
            markPatterns(
                in: line,
                claimed: &claimed,
                result: &result,
                patterns: [#"\b[A-Z][A-Za-z0-9_]*\b"#],
                kind: .type
            )
        }
        markKeywords(in: line, claimed: &claimed, result: &result, language: language)

        // Fill remaining gaps as plain.
        var location = 0
        while location < claimed.count {
            if claimed[location] {
                location += 1
                continue
            }
            let start = location
            while location < claimed.count, !claimed[location] {
                location += 1
            }
            result.append(SyntaxToken(kind: .plain, location: start, length: location - start))
        }

        return result.sorted { $0.location < $1.location }
    }

    private nonisolated static func keywords(for language: SyntaxLanguage) -> Set<String> {
        switch language {
        case .swift:
            return [
                "import", "struct", "class", "enum", "func", "var", "let", "return",
                "if", "else", "guard", "switch", "case", "for", "while", "in", "where",
                "protocol", "extension", "private", "public", "internal", "static",
                "async", "await", "throws", "try", "catch", "true", "false", "nil",
                "self", "Self", "some", "any", "init", "deinit", "override", "final",
            ]
        case .javascript, .typescript:
            return [
                "import", "export", "from", "const", "let", "var", "function", "return",
                "if", "else", "switch", "case", "for", "while", "of", "in", "class",
                "extends", "new", "async", "await", "try", "catch", "throw", "true",
                "false", "null", "undefined", "typeof", "instanceof", "interface",
                "type", "enum", "as", "default", "break", "continue",
            ]
        case .shell:
            return [
                "if", "then", "else", "fi", "for", "while", "do", "done", "case",
                "esac", "function", "return", "export", "local", "echo", "cd",
            ]
        case .json, .markdown, .plain:
            return []
        }
    }

    private nonisolated static func commentPatterns(for language: SyntaxLanguage) -> [String] {
        switch language {
        case .swift, .javascript, .typescript:
            return [#"//.*$"#, #"/\*.*?\*/"#]
        case .shell:
            return [#"#.*$"#]
        case .markdown:
            return []
        case .json, .plain:
            return []
        }
    }

    private nonisolated static func stringPatterns(for language: SyntaxLanguage) -> [String] {
        switch language {
        case .swift:
            return [#"#?"(?:\\.|[^"\\])*""#, #"#?'(?:\\.|[^'\\])*'"#]
        case .javascript, .typescript, .json:
            return [#""(?:\\.|[^"\\])*""#, #"'(?:\\.|[^'\\])*'"#, #"`(?:\\.|[^`\\])*`"#]
        case .shell:
            return [#""(?:\\.|[^"\\])*""#, #"'[^']*'"#]
        case .markdown:
            return [#"`[^`]+`"#]
        case .plain:
            return []
        }
    }

    private nonisolated static func markKeywords(
        in line: String,
        claimed: inout [Bool],
        result: inout [SyntaxToken],
        language: SyntaxLanguage
    ) {
        let words = keywords(for: language)
        guard !words.isEmpty else { return }
        let ns = line as NSString
        let full = NSRange(location: 0, length: ns.length)
        guard let regex = try? NSRegularExpression(pattern: #"\b[A-Za-z_][A-Za-z0-9_]*\b"#) else {
            return
        }
        for match in regex.matches(in: line, range: full) {
            let word = ns.substring(with: match.range)
            guard words.contains(word) else { continue }
            guard !isClaimed(match.range, in: claimed) else { continue }
            claim(match.range, in: &claimed)
            result.append(
                SyntaxToken(kind: .keyword, location: match.range.location, length: match.range.length)
            )
        }
    }

    private nonisolated static func markPatterns(
        in line: String,
        claimed: inout [Bool],
        result: inout [SyntaxToken],
        patterns: [String],
        kind: SyntaxTokenKind
    ) {
        let ns = line as NSString
        let full = NSRange(location: 0, length: ns.length)
        for pattern in patterns {
            guard let regex = try? NSRegularExpression(pattern: pattern, options: [.anchorsMatchLines])
            else { continue }
            for match in regex.matches(in: line, range: full) {
                guard match.range.length > 0, !isClaimed(match.range, in: claimed) else { continue }
                claim(match.range, in: &claimed)
                result.append(
                    SyntaxToken(kind: kind, location: match.range.location, length: match.range.length)
                )
            }
        }
    }

    private nonisolated static func isClaimed(_ range: NSRange, in claimed: [Bool]) -> Bool {
        guard range.location >= 0, range.length > 0 else { return true }
        let end = min(range.location + range.length, claimed.count)
        guard range.location < claimed.count else { return true }
        for index in range.location..<end where claimed[index] {
            return true
        }
        return false
    }

    private nonisolated static func claim(_ range: NSRange, in claimed: inout [Bool]) {
        let end = min(range.location + range.length, claimed.count)
        guard range.location < claimed.count else { return }
        for index in range.location..<end {
            claimed[index] = true
        }
    }
}
