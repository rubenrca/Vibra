//! Lightweight, line-oriented syntax highlighting for the Diff panel.
//!
//! Enough to make code read like an IDE (keywords, strings, comments, numbers,
//! types, functions) for the languages Vibra users hit most.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::LazyLock;

use gpui::{FontStyle, FontWeight, HighlightStyle, Rgba, rgb};

use crate::ports::git::{GitDiffRow, GitDiffRowKind};
use crate::ui::theme::colors;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Swift,
    Go,
    Json,
    Toml,
    Markdown,
    Shell,
    Css,
    Html,
    Yaml,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxKind {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Attribute,
    Operator,
    Constant,
    Punctuation,
    Macro,
    Default,
}

#[derive(Debug, Clone)]
pub struct SyntaxSpan {
    pub range: Range<usize>,
    pub kind: SyntaxKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Code,
    BlockComment,
    String {
        quote: u8,
    },
    /// Rust raw string `r##"..."##` — number of `#` delimiters.
    RawString {
        hashes: u8,
    },
}

/// Multi-line state so block comments / strings survive across hunk lines.
#[derive(Debug, Clone)]
pub struct Highlighter {
    language: Language,
    mode: Mode,
}

impl Highlighter {
    pub fn new(language: Language) -> Self {
        Self {
            language,
            mode: Mode::Code,
        }
    }

    pub fn for_path(path: &str) -> Self {
        Self::new(language_from_path(path))
    }

    pub fn highlight_line(&mut self, line: &str) -> Vec<SyntaxSpan> {
        if matches!(self.language, Language::Plain | Language::Markdown) {
            return Vec::new();
        }
        let bytes = line.as_bytes();
        let mut spans = Vec::new();
        let mut i = 0;
        let mut code_start = 0usize;

        while i < bytes.len() {
            match self.mode {
                Mode::BlockComment => {
                    let start = i;
                    let mut closed = false;
                    while i + 1 < bytes.len() {
                        if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                            i += 2;
                            closed = true;
                            break;
                        }
                        i += 1;
                    }
                    if !closed {
                        i = bytes.len();
                    } else {
                        self.mode = Mode::Code;
                    }
                    spans.push(SyntaxSpan {
                        range: start..i,
                        kind: SyntaxKind::Comment,
                    });
                    code_start = i;
                }
                Mode::String { quote } => {
                    let start = i;
                    let (end, closed) = scan_string_end(bytes, i, quote);
                    i = end;
                    spans.push(SyntaxSpan {
                        range: start..i,
                        kind: SyntaxKind::String,
                    });
                    if closed {
                        self.mode = Mode::Code;
                        code_start = i;
                    }
                }
                Mode::RawString { hashes } => {
                    let start = i;
                    let (end, closed) = scan_raw_string_end(bytes, i, hashes);
                    i = end;
                    spans.push(SyntaxSpan {
                        range: start..i,
                        kind: SyntaxKind::String,
                    });
                    if closed {
                        self.mode = Mode::Code;
                        code_start = i;
                    }
                }
                Mode::Code => {
                    // Line comment //
                    if self.line_comment_slash()
                        && i + 1 < bytes.len()
                        && bytes[i] == b'/'
                        && bytes[i + 1] == b'/'
                    {
                        flush_code(line, code_start, i, self.language, &mut spans);
                        spans.push(SyntaxSpan {
                            range: i..bytes.len(),
                            kind: SyntaxKind::Comment,
                        });
                        return spans;
                    }
                    // Hash line comment (#) for Python / Shell / Toml / Yaml
                    if self.line_comment_hash() && bytes[i] == b'#' {
                        flush_code(line, code_start, i, self.language, &mut spans);
                        spans.push(SyntaxSpan {
                            range: i..bytes.len(),
                            kind: SyntaxKind::Comment,
                        });
                        return spans;
                    }
                    // Block comment /*
                    if self.block_comment()
                        && i + 1 < bytes.len()
                        && bytes[i] == b'/'
                        && bytes[i + 1] == b'*'
                    {
                        flush_code(line, code_start, i, self.language, &mut spans);
                        self.mode = Mode::BlockComment;
                        continue;
                    }
                    // Rust raw string r" / r#"
                    if self.language == Language::Rust
                        && bytes[i] == b'r'
                        && let Some((hashes, open_end)) = rust_raw_opener(bytes, i)
                    {
                        flush_code(line, code_start, i, self.language, &mut spans);
                        let start = i;
                        let (end, closed) = scan_raw_string_end(bytes, open_end, hashes);
                        spans.push(SyntaxSpan {
                            range: start..end,
                            kind: SyntaxKind::String,
                        });
                        i = end;
                        if closed {
                            self.mode = Mode::Code;
                            code_start = i;
                        } else {
                            self.mode = Mode::RawString { hashes };
                        }
                        continue;
                    }
                    // Normal string
                    if is_string_quote(bytes[i], self.language, bytes, i) {
                        flush_code(line, code_start, i, self.language, &mut spans);
                        let quote = bytes[i];
                        let start = i;
                        let (end, closed) = scan_string_end(bytes, i + 1, quote);
                        spans.push(SyntaxSpan {
                            range: start..end,
                            kind: SyntaxKind::String,
                        });
                        i = end;
                        if closed {
                            self.mode = Mode::Code;
                            code_start = i;
                        } else {
                            self.mode = Mode::String { quote };
                        }
                        continue;
                    }
                    i += 1;
                }
            }
        }

        if matches!(self.mode, Mode::Code) {
            flush_code(line, code_start, bytes.len(), self.language, &mut spans);
        }
        spans
    }

    fn line_comment_slash(&self) -> bool {
        matches!(
            self.language,
            Language::Rust
                | Language::JavaScript
                | Language::TypeScript
                | Language::Swift
                | Language::Go
                | Language::Css
        )
    }

    fn line_comment_hash(&self) -> bool {
        matches!(
            self.language,
            Language::Python | Language::Shell | Language::Toml | Language::Yaml
        )
    }

    fn block_comment(&self) -> bool {
        matches!(
            self.language,
            Language::Rust
                | Language::JavaScript
                | Language::TypeScript
                | Language::Swift
                | Language::Go
                | Language::Css
        )
    }
}

fn is_string_quote(b: u8, language: Language, bytes: &[u8], i: usize) -> bool {
    if b == b'"' || b == b'`' {
        return true;
    }
    if b != b'\'' {
        return false;
    }
    // Rust lifetime `'a` / `'static` — not a char/string.
    if language == Language::Rust
        && i + 1 < bytes.len()
        && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_')
    {
        return false;
    }
    true
}

fn rust_raw_opener(bytes: &[u8], i: usize) -> Option<(u8, usize)> {
    // r"…", r#"…"#, r##"…"##
    if bytes.get(i) != Some(&b'r') {
        return None;
    }
    let mut j = i + 1;
    let mut hashes = 0u8;
    while j < bytes.len() && bytes[j] == b'#' && hashes < 16 {
        hashes += 1;
        j += 1;
    }
    if j < bytes.len() && bytes[j] == b'"' {
        Some((hashes, j + 1))
    } else {
        None
    }
}

fn scan_string_end(bytes: &[u8], mut i: usize, quote: u8) -> (usize, bool) {
    let mut escape = false;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
            i += 1;
            continue;
        }
        if b == b'\\' {
            escape = true;
            i += 1;
            continue;
        }
        if b == quote {
            return (i + 1, true);
        }
        i += 1;
    }
    (bytes.len(), false)
}

fn scan_raw_string_end(bytes: &[u8], mut i: usize, hashes: u8) -> (usize, bool) {
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let mut k = i + 1;
            let mut seen = 0u8;
            while k < bytes.len() && bytes[k] == b'#' && seen < hashes {
                seen += 1;
                k += 1;
            }
            if seen == hashes {
                return (k, true);
            }
        }
        i += 1;
    }
    (bytes.len(), false)
}

fn flush_code(
    line: &str,
    start: usize,
    end: usize,
    language: Language,
    spans: &mut Vec<SyntaxSpan>,
) {
    if start >= end {
        return;
    }
    let bytes = line.as_bytes();
    let mut i = start;
    while i < end {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Number
        if b.is_ascii_digit() || (b == b'.' && i + 1 < end && bytes[i + 1].is_ascii_digit()) {
            let s = i;
            i += 1;
            while i < end
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            spans.push(SyntaxSpan {
                range: s..i,
                kind: SyntaxKind::Number,
            });
            continue;
        }
        // Identifier
        if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
            let s = i;
            i += 1;
            while i < end
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
            {
                i += 1;
            }
            let is_macro = language == Language::Rust && i < end && bytes[i] == b'!';
            let word = &line[s..i];
            let after = peek_after_ident(bytes, i, end);
            let kind = classify_ident(word, language, is_macro, after);
            if is_macro {
                i += 1;
            }
            spans.push(SyntaxSpan { range: s..i, kind });
            continue;
        }
        // Two-char operators
        if i + 1 < end {
            let two = &line[i..i + 2];
            if matches!(
                two,
                "=>" | "->"
                    | "::"
                    | "=="
                    | "!="
                    | "<="
                    | ">="
                    | "&&"
                    | "||"
                    | "+="
                    | "-="
                    | "*="
                    | "/="
                    | "<<"
                    | ">>"
                    | ".."
                    | "??"
                    | "?."
            ) {
                spans.push(SyntaxSpan {
                    range: i..i + 2,
                    kind: SyntaxKind::Operator,
                });
                i += 2;
                continue;
            }
        }
        if matches!(
            b,
            b'+' | b'-'
                | b'*'
                | b'/'
                | b'%'
                | b'='
                | b'!'
                | b'<'
                | b'>'
                | b'&'
                | b'|'
                | b'^'
                | b'~'
                | b'?'
                | b':'
        ) {
            spans.push(SyntaxSpan {
                range: i..i + 1,
                kind: SyntaxKind::Operator,
            });
            i += 1;
            continue;
        }
        if matches!(
            b,
            b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b';' | b'.' | b'@'
        ) {
            spans.push(SyntaxSpan {
                range: i..i + 1,
                kind: if b == b'@' {
                    SyntaxKind::Attribute
                } else {
                    SyntaxKind::Punctuation
                },
            });
            i += 1;
            continue;
        }
        i += 1;
    }
}

#[derive(Clone, Copy)]
enum AfterIdent {
    Call,
    Nothing,
}

fn peek_after_ident(bytes: &[u8], mut i: usize, end: usize) -> AfterIdent {
    while i < end && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i < end && bytes[i] == b'(' {
        AfterIdent::Call
    } else {
        AfterIdent::Nothing
    }
}

fn classify_ident(word: &str, language: Language, is_macro: bool, after: AfterIdent) -> SyntaxKind {
    if is_macro {
        return SyntaxKind::Macro;
    }
    if is_keyword(word, language) {
        return SyntaxKind::Keyword;
    }
    if is_literal_constant(word, language) {
        return SyntaxKind::Constant;
    }
    if word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return SyntaxKind::Type;
    }
    if matches!(after, AfterIdent::Call) {
        return SyntaxKind::Function;
    }
    if word.len() > 1
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && word.chars().any(|c| c.is_ascii_uppercase())
    {
        return SyntaxKind::Constant;
    }
    SyntaxKind::Default
}

fn is_literal_constant(word: &str, language: Language) -> bool {
    matches!(
        word,
        "true" | "false" | "null" | "undefined" | "None" | "nil"
    ) || (language == Language::Python && matches!(word, "True" | "False"))
        || (language == Language::Rust && matches!(word, "Some" | "Ok" | "Err"))
}

fn is_keyword(word: &str, language: Language) -> bool {
    match language {
        Language::Rust => RUST_KEYWORDS.contains(word),
        Language::JavaScript | Language::TypeScript => JS_KEYWORDS.contains(word),
        Language::Python => PYTHON_KEYWORDS.contains(word),
        Language::Swift => SWIFT_KEYWORDS.contains(word),
        Language::Go => GO_KEYWORDS.contains(word),
        Language::Shell => SHELL_KEYWORDS.contains(word),
        Language::Css => CSS_KEYWORDS.contains(word),
        Language::Json | Language::Toml | Language::Yaml => {
            matches!(word, "true" | "false" | "null")
        }
        Language::Html | Language::Markdown | Language::Plain => false,
    }
}

pub fn language_from_path(path: &str) -> Language {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Language::Rust,
        "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
        "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
        "py" | "pyi" => Language::Python,
        "swift" => Language::Swift,
        "go" => Language::Go,
        "json" | "jsonc" => Language::Json,
        "toml" => Language::Toml,
        "md" | "mdx" | "markdown" => Language::Markdown,
        "sh" | "bash" | "zsh" | "fish" => Language::Shell,
        "css" | "scss" | "less" => Language::Css,
        "html" | "htm" | "xml" | "svg" => Language::Html,
        "yml" | "yaml" => Language::Yaml,
        _ => Language::Plain,
    }
}

impl SyntaxKind {
    pub fn color(self) -> Rgba {
        match self {
            Self::Keyword => SYNTAX.keyword,
            Self::String => SYNTAX.string,
            Self::Comment => SYNTAX.comment,
            Self::Number => SYNTAX.number,
            Self::Type => SYNTAX.type_name,
            Self::Function => SYNTAX.function,
            Self::Attribute => SYNTAX.attribute,
            Self::Operator => SYNTAX.operator,
            Self::Constant => SYNTAX.constant,
            Self::Punctuation => SYNTAX.punctuation,
            Self::Macro => SYNTAX.macro_name,
            Self::Default => colors().foreground,
        }
    }

    pub fn highlight_style(self) -> HighlightStyle {
        let mut style = HighlightStyle {
            color: Some(self.color().into()),
            ..Default::default()
        };
        match self {
            Self::Comment => style.font_style = Some(FontStyle::Italic),
            Self::Keyword | Self::Macro => style.font_weight = Some(FontWeight::MEDIUM),
            _ => {}
        }
        style
    }
}

/// One Dark–inspired palette that reads well on Vibra’s dark surfaces.
struct SyntaxPalette {
    keyword: Rgba,
    string: Rgba,
    comment: Rgba,
    number: Rgba,
    type_name: Rgba,
    function: Rgba,
    attribute: Rgba,
    operator: Rgba,
    constant: Rgba,
    punctuation: Rgba,
    macro_name: Rgba,
}

static SYNTAX: LazyLock<SyntaxPalette> = LazyLock::new(|| SyntaxPalette {
    keyword: rgb(0xc792ea),
    string: rgb(0xc3e88d),
    comment: rgb(0x676e95),
    number: rgb(0xf78c6c),
    type_name: rgb(0xffcb6b),
    function: rgb(0x82aaff),
    attribute: rgb(0xffcb6b),
    operator: rgb(0x89ddff),
    constant: rgb(0xf78c6c),
    punctuation: rgb(0x89ddff),
    macro_name: rgb(0xc792ea),
});

fn set(words: &[&'static str]) -> HashSet<&'static str> {
    words.iter().copied().collect()
}

static RUST_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while", "try", "union", "box",
    ])
});

static JS_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "get",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "new",
        "null",
        "of",
        "private",
        "protected",
        "public",
        "return",
        "set",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "with",
        "yield",
        "type",
        "namespace",
        "declare",
        "readonly",
        "keyof",
        "infer",
    ])
});

static PYTHON_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
        "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True",
        "try", "while", "with", "yield", "match", "case",
    ])
});

static SWIFT_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "class",
        "deinit",
        "enum",
        "extension",
        "func",
        "import",
        "init",
        "let",
        "protocol",
        "static",
        "struct",
        "subscript",
        "typealias",
        "var",
        "break",
        "case",
        "continue",
        "default",
        "defer",
        "do",
        "else",
        "for",
        "guard",
        "if",
        "in",
        "return",
        "switch",
        "where",
        "while",
        "as",
        "catch",
        "false",
        "is",
        "nil",
        "super",
        "self",
        "Self",
        "throw",
        "throws",
        "true",
        "try",
        "async",
        "await",
        "actor",
        "some",
    ])
});

static GO_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "map",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "type",
        "var",
    ])
});

static SHELL_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done",
        "in", "function", "select", "return", "exit", "export", "local", "readonly", "declare",
        "unset", "shift", "source", "alias",
    ])
});

static CSS_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "important",
        "from",
        "to",
        "and",
        "or",
        "not",
        "only",
        "var",
        "rgb",
        "rgba",
        "hsl",
        "url",
        "calc",
        "min",
        "max",
        "clamp",
    ])
});

/// Highlight every text row of a diff in order (preserves multi-line comment/string state).
pub fn highlight_diff_rows(path: &str, rows: &[GitDiffRow]) -> Vec<Vec<SyntaxSpan>> {
    let mut highlighter = Highlighter::for_path(path);
    rows.iter()
        .map(|row| match row.kind {
            GitDiffRowKind::Context | GitDiffRowKind::Addition | GitDiffRowKind::Deletion => {
                highlighter.highlight_line(&row.text)
            }
            _ => Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_keywords_and_string() {
        let mut h = Highlighter::new(Language::Rust);
        let spans = h.highlight_line(r#"let name = "vibra";"#);
        let line = r#"let name = "vibra";"#;
        assert!(
            spans
                .iter()
                .any(|s| s.kind == SyntaxKind::Keyword && line_slice(line, &s.range) == "let"),
            "expected keyword let, got {spans:?}"
        );
        assert!(spans.iter().any(|s| s.kind == SyntaxKind::String));
    }

    #[test]
    fn rust_line_comment() {
        let mut h = Highlighter::new(Language::Rust);
        let spans = h.highlight_line("let x = 1; // trailing");
        assert!(spans.iter().any(|s| s.kind == SyntaxKind::Comment));
        assert!(spans.iter().any(|s| s.kind == SyntaxKind::Number));
    }

    #[test]
    fn block_comment_spans_lines() {
        let mut h = Highlighter::new(Language::Rust);
        let first = h.highlight_line("let x = 1; /* open");
        assert!(first.iter().any(|s| s.kind == SyntaxKind::Comment));
        let second = h.highlight_line(" still comment */ let y = 2;");
        assert!(second.iter().any(|s| s.kind == SyntaxKind::Comment));
        assert!(second.iter().any(|s| s.kind == SyntaxKind::Keyword));
    }

    #[test]
    fn language_from_extension() {
        assert_eq!(language_from_path("src/main.rs"), Language::Rust);
        assert_eq!(language_from_path("a/b/theme.ts"), Language::TypeScript);
        assert_eq!(language_from_path("README.md"), Language::Markdown);
    }

    fn line_slice<'a>(line: &'a str, range: &Range<usize>) -> &'a str {
        &line[range.clone()]
    }
}
