import Foundation

/// Remembered layout for the full-area diff surface.
enum DiffLayoutStyle: String, CaseIterable, Sendable {
    case unified
    case split
}

/// Predicate for the Git changes list filter.
enum ChangeListFilter {
    nonisolated static func matches(_ change: GitFileChange, query: String) -> Bool {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return true }
        return change.path.localizedCaseInsensitiveContains(trimmed)
            || change.fileName.localizedCaseInsensitiveContains(trimmed)
    }

    nonisolated static func apply(_ changes: [GitFileChange], query: String) -> [GitFileChange] {
        changes.filter { matches($0, query: query) }
    }
}

/// A contiguous span inside a single diff line for word-level highlighting.
struct DiffTextSpan: Equatable, Sendable {
    enum Kind: Equatable, Sendable {
        case equal
        case insert
        case delete
    }

    let kind: Kind
    let text: String
}

/// Myers-style word/token diff for pairing a deletion line with an addition line.
enum WordDiff {
    /// Tokenizes on word boundaries and punctuation so common code edits light up.
    nonisolated static func spans(old: String, new: String) -> (old: [DiffTextSpan], new: [DiffTextSpan]) {
        let oldTokens = tokenize(old)
        let newTokens = tokenize(new)
        let ops = diffTokens(oldTokens, newTokens)
        var oldSpans: [DiffTextSpan] = []
        var newSpans: [DiffTextSpan] = []
        for op in ops {
            switch op {
            case .equal(let token):
                oldSpans.append(DiffTextSpan(kind: .equal, text: token))
                newSpans.append(DiffTextSpan(kind: .equal, text: token))
            case .delete(let token):
                oldSpans.append(DiffTextSpan(kind: .delete, text: token))
            case .insert(let token):
                newSpans.append(DiffTextSpan(kind: .insert, text: token))
            }
        }
        return (mergeAdjacent(oldSpans), mergeAdjacent(newSpans))
    }

    /// Pairs consecutive deletion blocks with following addition blocks in a
    /// unified hunk so the UI can paint word-level highlights.
    nonisolated static func pairings(for lines: [DiffLine]) -> [Int: WordPair] {
        var result: [Int: WordPair] = [:]
        var index = 0
        while index < lines.count {
            let kind = lines[index].kind
            guard kind == .deletion || kind == .addition else {
                index += 1
                continue
            }
            var deletions: [Int] = []
            var additions: [Int] = []
            var cursor = index
            while cursor < lines.count, lines[cursor].kind == .deletion {
                deletions.append(cursor)
                cursor += 1
            }
            while cursor < lines.count, lines[cursor].kind == .addition {
                additions.append(cursor)
                cursor += 1
            }
            let pairCount = min(deletions.count, additions.count)
            for offset in 0..<pairCount {
                let oldIndex = deletions[offset]
                let newIndex = additions[offset]
                let oldText = displayCode(lines[oldIndex])
                let newText = displayCode(lines[newIndex])
                let (oldSpans, newSpans) = spans(old: oldText, new: newText)
                // Only store when there is real intra-line change (not whole-line replace).
                let hasPartial = oldSpans.contains { $0.kind == .equal }
                    || newSpans.contains { $0.kind == .equal }
                if hasPartial {
                    result[oldIndex] = WordPair(oldSpans: oldSpans, newSpans: newSpans, partnerID: lines[newIndex].id)
                    result[newIndex] = WordPair(oldSpans: oldSpans, newSpans: newSpans, partnerID: lines[oldIndex].id)
                }
            }
            index = cursor
        }
        return result
    }

    struct WordPair: Equatable, Sendable {
        let oldSpans: [DiffTextSpan]
        let newSpans: [DiffTextSpan]
        let partnerID: Int
    }

    private enum Op {
        case equal(String)
        case delete(String)
        case insert(String)
    }

    private nonisolated static func tokenize(_ text: String) -> [String] {
        var tokens: [String] = []
        var current = ""
        for character in text {
            if character.isLetter || character.isNumber || character == "_" {
                current.append(character)
            } else {
                if !current.isEmpty {
                    tokens.append(current)
                    current = ""
                }
                tokens.append(String(character))
            }
        }
        if !current.isEmpty { tokens.append(current) }
        return tokens
    }

    private nonisolated static func diffTokens(_ old: [String], _ new: [String]) -> [Op] {
        // Classic LCS DP — fine for single-line token counts.
        let n = old.count
        let m = new.count
        if n == 0 {
            return new.map { .insert($0) }
        }
        if m == 0 {
            return old.map { .delete($0) }
        }
        var table = Array(repeating: Array(repeating: 0, count: m + 1), count: n + 1)
        for i in 1...n {
            for j in 1...m {
                if old[i - 1] == new[j - 1] {
                    table[i][j] = table[i - 1][j - 1] + 1
                } else {
                    table[i][j] = max(table[i - 1][j], table[i][j - 1])
                }
            }
        }
        var ops: [Op] = []
        var i = n
        var j = m
        while i > 0 || j > 0 {
            if i > 0, j > 0, old[i - 1] == new[j - 1] {
                ops.append(.equal(old[i - 1]))
                i -= 1
                j -= 1
            } else if j > 0, i == 0 || table[i][j - 1] >= table[i - 1][j] {
                ops.append(.insert(new[j - 1]))
                j -= 1
            } else if i > 0 {
                ops.append(.delete(old[i - 1]))
                i -= 1
            }
        }
        return ops.reversed()
    }

    private nonisolated static func mergeAdjacent(_ spans: [DiffTextSpan]) -> [DiffTextSpan] {
        guard var last = spans.first else { return [] }
        var result: [DiffTextSpan] = []
        for span in spans.dropFirst() {
            if span.kind == last.kind {
                last = DiffTextSpan(kind: last.kind, text: last.text + span.text)
            } else {
                result.append(last)
                last = span
            }
        }
        result.append(last)
        return result
    }

    private nonisolated static func displayCode(_ line: DiffLine) -> String {
        switch line.kind {
        case .addition, .deletion:
            return String(line.text.dropFirst())
        case .context where line.text.hasPrefix(" "):
            return String(line.text.dropFirst())
        default:
            return line.text
        }
    }
}

/// One row in a side-by-side split presentation of unified diff lines.
struct SplitDiffRow: Identifiable, Equatable, Sendable {
    let id: Int
    let left: DiffLine?
    let right: DiffLine?
}

enum DiffPresentation {
    /// Maps unified `DiffLine` data into split rows: context on both sides,
    /// deletions on the left, additions on the right, paired when adjacent.
    nonisolated static func splitRows(from lines: [DiffLine]) -> [SplitDiffRow] {
        var rows: [SplitDiffRow] = []
        var index = 0
        var rowID = 0
        while index < lines.count {
            let line = lines[index]
            switch line.kind {
            case .metadata:
                index += 1
                continue
            case .hunk:
                rows.append(SplitDiffRow(id: rowID, left: line, right: line))
                rowID += 1
                index += 1
            case .context:
                rows.append(SplitDiffRow(id: rowID, left: line, right: line))
                rowID += 1
                index += 1
            case .deletion, .addition:
                var deletions: [DiffLine] = []
                var additions: [DiffLine] = []
                while index < lines.count, lines[index].kind == .deletion {
                    deletions.append(lines[index])
                    index += 1
                }
                while index < lines.count, lines[index].kind == .addition {
                    additions.append(lines[index])
                    index += 1
                }
                let count = max(deletions.count, additions.count)
                for offset in 0..<count {
                    let left = offset < deletions.count ? deletions[offset] : nil
                    let right = offset < additions.count ? additions[offset] : nil
                    rows.append(SplitDiffRow(id: rowID, left: left, right: right))
                    rowID += 1
                }
            }
        }
        return rows
    }

    /// Filters metadata and builds the unified display sequence used by the UI.
    nonisolated static func unifiedRows(from lines: [DiffLine]) -> [DiffLine] {
        lines.filter { $0.kind != .metadata }
    }
}
