import AppKit
import SwiftUI

/// Shared editor-like diff canvas for the right-sidebar expanded viewer.
struct DiffLinesCanvas: View {
    let lines: [DiffLine]
    let layout: DiffLayoutStyle
    let filePath: String
    var minimumCodeWidth: CGFloat = 480
    var minimumHeight: CGFloat = 120
    @Environment(\.appChrome) private var chrome

    private var language: SyntaxLanguage {
        SyntaxHighlighter.language(forPath: filePath)
    }

    private var wordPairs: [Int: WordDiff.WordPair] {
        WordDiff.pairings(for: lines)
    }

    var body: some View {
        Group {
            if layout == .split {
                splitBody
            } else {
                unifiedBody
            }
        }
    }

    private var unifiedBody: some View {
        let display = DiffPresentation.unifiedRows(from: lines)
        return ScrollView([.horizontal, .vertical]) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(Array(display.enumerated()), id: \.element.id) { index, line in
                    if line.kind == .hunk, let omitted = omittedCount(before: line, in: display, at: index) {
                        omittedRow(omitted)
                    }
                    unifiedRow(line, lineIndex: indexInOriginal(line))
                }
            }
            .fixedSize(horizontal: true, vertical: false)
        }
        .defaultScrollAnchor(.topLeading)
        .frame(minHeight: minimumHeight)
    }

    private var splitBody: some View {
        let rows = DiffPresentation.splitRows(from: lines)
        return ScrollView([.horizontal, .vertical]) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(rows) { row in
                    HStack(spacing: 0) {
                        splitSide(row.left, side: .left)
                        Rectangle()
                            .fill(chrome.quietBorder)
                            .frame(width: 1)
                        splitSide(row.right, side: .right)
                    }
                }
            }
            .fixedSize(horizontal: true, vertical: false)
        }
        .defaultScrollAnchor(.topLeading)
        .frame(minHeight: minimumHeight)
    }

    private enum SplitSide { case left, right }

    private func splitSide(_ line: DiffLine?, side: SplitSide) -> some View {
        Group {
            if let line {
                HStack(spacing: 0) {
                    gutterAccent(line.kind)
                    lineNumber(side == .left ? line.oldLine : line.newLine)
                    signColumn(line.kind)
                    codeText(line, lineIndex: indexInOriginal(line))
                        .frame(minWidth: minimumCodeWidth / 2, maxWidth: .infinity, alignment: .leading)
                        .padding(.trailing, 12)
                }
                .font(.system(size: 11.5, design: .monospaced))
                .frame(minHeight: 22)
                .background(lineBackground(line.kind))
            } else {
                Color.clear
                    .frame(minWidth: minimumCodeWidth / 2 + 100, minHeight: 22)
                    .background(chrome.foreground.opacity(0.02))
            }
        }
        .frame(maxWidth: .infinity)
    }

    private func unifiedRow(_ line: DiffLine, lineIndex: Int) -> some View {
        HStack(spacing: 0) {
            gutterAccent(line.kind)
            lineNumber(line.oldLine)
            lineNumber(line.newLine)
            signColumn(line.kind)
            codeText(line, lineIndex: lineIndex)
                .frame(minWidth: minimumCodeWidth, maxWidth: .infinity, alignment: .leading)
                .padding(.trailing, 16)
        }
        .font(.system(size: 11.5, design: .monospaced))
        .frame(minHeight: 22)
        .background(lineBackground(line.kind))
    }

    @ViewBuilder
    private func codeText(_ line: DiffLine, lineIndex: Int) -> some View {
        let code = displayText(line)
        if line.kind == .hunk || line.kind == .metadata {
            Text(verbatim: code.isEmpty ? " " : code)
                .foregroundStyle(lineColor(line.kind))
                .textSelection(.enabled)
        } else if let pair = wordPairs[lineIndex],
                  line.kind == .deletion || line.kind == .addition
        {
            let spans = line.kind == .deletion ? pair.oldSpans : pair.newSpans
            wordHighlighted(spans, base: line.kind)
                .textSelection(.enabled)
        } else {
            syntaxColored(code, kind: line.kind)
                .textSelection(.enabled)
        }
    }

    private func wordHighlighted(_ spans: [DiffTextSpan], base: DiffLineKind) -> Text {
        var attributed = AttributedString()
        for span in spans {
            var part = AttributedString(span.text)
            part.font = .system(size: 11.5, design: .monospaced)
            switch span.kind {
            case .equal:
                part.foregroundColor = Color(nsColor: .labelColor)
                if let tokens = optionalSyntax(for: span.text), !tokens.isEmpty {
                    // Keep equal regions lightly syntax-tinted when whole-token.
                    part.foregroundColor = Color(nsColor: .labelColor)
                }
            case .insert:
                part.foregroundColor = .green
                part.backgroundColor = Color.green.opacity(0.28)
            case .delete:
                part.foregroundColor = .red
                part.backgroundColor = Color.red.opacity(0.26)
            }
            attributed.append(part)
        }
        if attributed.characters.isEmpty {
            return Text(verbatim: " ").foregroundStyle(lineColor(base))
        }
        return Text(attributed)
    }

    private func syntaxColored(_ code: String, kind: DiffLineKind) -> Text {
        guard kind == .context || kind == .addition || kind == .deletion else {
            return Text(verbatim: code.isEmpty ? " " : code)
                .foregroundStyle(lineColor(kind))
        }
        let tokens = SyntaxHighlighter.tokens(in: code, language: language)
        guard !tokens.isEmpty, language != .plain else {
            return Text(verbatim: code.isEmpty ? " " : code)
                .foregroundStyle(lineColor(kind))
        }
        var attributed = AttributedString(code)
        let ns = code as NSString
        for token in tokens {
            guard token.length > 0,
                  token.location >= 0,
                  token.location + token.length <= ns.length,
                  let range = Range(NSRange(location: token.location, length: token.length), in: code)
            else { continue }
            let attrRange = Range(range, in: attributed)
            guard let attrRange else { continue }
            attributed[attrRange].foregroundColor = syntaxColor(token.kind, fallback: lineColor(kind))
            attributed[attrRange].font = .system(size: 11.5, design: .monospaced)
        }
        return Text(attributed)
    }

    private func optionalSyntax(for text: String) -> [SyntaxToken]? {
        SyntaxHighlighter.tokens(in: text, language: language)
    }

    private func syntaxColor(_ kind: SyntaxTokenKind, fallback: Color) -> Color {
        switch kind {
        case .keyword: Color(red: 0.72, green: 0.42, blue: 0.86)
        case .string: Color(red: 0.35, green: 0.68, blue: 0.42)
        case .comment: Color(nsColor: .secondaryLabelColor)
        case .number: Color(red: 0.82, green: 0.55, blue: 0.28)
        case .type: Color(red: 0.35, green: 0.62, blue: 0.88)
        case .plain: fallback
        }
    }

    private func gutterAccent(_ kind: DiffLineKind) -> some View {
        Rectangle()
            .fill(accentColor(kind))
            .frame(width: 3)
    }

    private func lineNumber(_ value: Int?) -> some View {
        Text(value.map(String.init) ?? "")
            .font(.system(size: 9.5, design: .monospaced))
            .foregroundStyle(chrome.foreground.opacity(0.38))
            .frame(width: 46, alignment: .trailing)
            .padding(.trailing, 8)
            .background(chrome.foreground.opacity(0.045))
    }

    private func signColumn(_ kind: DiffLineKind) -> some View {
        Text(sign(for: kind))
            .foregroundStyle(lineColor(kind))
            .frame(width: 18)
    }

    private func omittedRow(_ count: Int) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "ellipsis")
                .font(.system(size: 10, weight: .semibold))
            Text("\(count) unmodified lines")
        }
        .font(.system(size: 10.5, design: .monospaced))
        .foregroundStyle(chrome.foreground.opacity(0.62))
        .padding(.leading, 88)
        .frame(
            minWidth: minimumCodeWidth + 100,
            maxWidth: .infinity,
            minHeight: 28,
            alignment: .leading
        )
        .background(chrome.foreground.opacity(0.035))
    }

    private func displayText(_ line: DiffLine) -> String {
        switch line.kind {
        case .addition, .deletion:
            return String(line.text.dropFirst())
        case .context where line.oldLine != nil:
            return line.text.hasPrefix(" ") ? String(line.text.dropFirst()) : line.text
        default:
            return line.text.isEmpty ? " " : line.text
        }
    }

    private func sign(for kind: DiffLineKind) -> String {
        switch kind {
        case .addition: "+"
        case .deletion: "−"
        default: ""
        }
    }

    private func lineColor(_ kind: DiffLineKind) -> Color {
        switch kind {
        case .metadata: chrome.foreground.opacity(0.62)
        case .hunk: chrome.accent
        case .addition: Color(red: 0.28, green: 0.62, blue: 0.36)
        case .deletion: Color(red: 0.78, green: 0.32, blue: 0.28)
        case .context: chrome.foreground
        }
    }

    private func lineBackground(_ kind: DiffLineKind) -> Color {
        switch kind {
        case .addition: Color.green.opacity(0.12)
        case .deletion: Color.red.opacity(0.10)
        case .hunk: chrome.accent.opacity(0.075)
        default: .clear
        }
    }

    private func accentColor(_ kind: DiffLineKind) -> Color {
        switch kind {
        case .addition: .green
        case .deletion: .red
        default: .clear
        }
    }

    private func indexInOriginal(_ line: DiffLine) -> Int {
        lines.firstIndex(where: { $0.id == line.id }) ?? 0
    }

    private func omittedCount(before line: DiffLine, in display: [DiffLine], at index: Int) -> Int? {
        guard line.kind == .hunk, let range = newRange(from: line.text) else { return nil }
        var previousNewEnd = 0
        for prior in display.prefix(index) where prior.kind == .hunk {
            if let priorRange = newRange(from: prior.text) {
                previousNewEnd = priorRange.start + max(priorRange.count, 1) - 1
            }
        }
        let omitted = max(0, range.start - previousNewEnd - 1)
        return omitted > 0 ? omitted : nil
    }

    private func newRange(from hunk: String) -> (start: Int, count: Int)? {
        guard let field = hunk.split(separator: " ").first(where: { $0.hasPrefix("+") })
        else { return nil }
        let values = field.dropFirst().split(separator: ",", maxSplits: 1)
        guard let start = Int(values[0]) else { return nil }
        return (start, values.count > 1 ? Int(values[1]) ?? 1 : 1)
    }
}
