//! Lightweight, line-oriented syntax highlighting for diffs and the file viewer.
//!
//! Enough to make code read like an IDE (keywords, strings, comments, numbers,
//! types, functions, keys, tags) for the languages Vibra users hit most, plus
//! Markdown prose whose fenced code is highlighted in its own language.

use std::ops::Range;

use gpui::{FontStyle, FontWeight, HighlightStyle, Rgba, rgb};

use crate::ports::git::{GitDiffRow, GitDiffRowKind};
use crate::ui::theme::{colors, mix};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Swift,
    Go,
    /// C, C++, and Objective-C.
    C,
    /// Java and the JVM languages close enough to share its keywords.
    Java,
    Kotlin,
    CSharp,
    Ruby,
    Php,
    Sql,
    Lua,
    Dockerfile,
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
    /// Object, table, and mapping keys.
    Property,
    /// Markup element names.
    Tag,
    Heading,
    Strong,
    Emphasis,
    Link,
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
    /// Inside a block comment, until its closing delimiter.
    BlockComment(&'static [u8]),
    String(StringDelimiter),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringDelimiter {
    Quoted(u8),
    /// Rust raw string `r##"..."##` — number of `#` delimiters.
    Raw(u8),
}

impl StringDelimiter {
    fn scan_end(self, bytes: &[u8], start: usize) -> (usize, bool) {
        match self {
            Self::Quoted(quote) => scan_string_end(bytes, start, quote),
            Self::Raw(hashes) => scan_raw_string_end(bytes, start, hashes),
        }
    }
}

/// An open Markdown code fence: its marker and the highlighter of its code.
#[derive(Debug, Clone)]
struct Fence {
    marker: u8,
    width: usize,
    code: Box<Highlighter>,
}

/// Multi-line state so block comments / strings survive across hunk lines.
#[derive(Debug, Clone)]
pub struct Highlighter {
    language: Language,
    mode: Mode,
    /// Markup: inside a tag's angle brackets, which may span lines.
    in_tag: bool,
    /// Markdown: the fenced code block being read.
    fence: Option<Fence>,
}

impl Highlighter {
    pub fn new(language: Language) -> Self {
        Self {
            language,
            mode: Mode::Code,
            in_tag: false,
            fence: None,
        }
    }

    pub fn for_path(path: &str) -> Self {
        Self::new(language_from_path(path))
    }

    pub fn highlight_line(&mut self, line: &str) -> Vec<SyntaxSpan> {
        match self.language {
            Language::Plain => return Vec::new(),
            Language::Markdown => return self.highlight_markdown(line),
            Language::Html => return self.highlight_markup(line),
            _ => {}
        }
        let bytes = line.as_bytes();
        let mut spans = Vec::new();
        let mut i = 0;
        let mut code_start = 0usize;
        if self.mode == Mode::Code
            && let Some(end) = self.leading_key(line, &mut spans)
        {
            i = end;
            code_start = end;
        }

        while i < bytes.len() {
            match self.mode {
                Mode::BlockComment(close) => {
                    i = self.close_block_comment(bytes, i, i, close, &mut spans);
                    code_start = i;
                }
                Mode::String(delimiter) => {
                    let start = i;
                    let (end, closed) = delimiter.scan_end(bytes, i);
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
                    if self.line_comment_at(bytes, i) {
                        flush_code(line, code_start, i, self.language, &mut spans);
                        spans.push(SyntaxSpan {
                            range: i..bytes.len(),
                            kind: SyntaxKind::Comment,
                        });
                        return spans;
                    }
                    if let Some((open, close)) = self.block_comment_at(bytes, i) {
                        flush_code(line, code_start, i, self.language, &mut spans);
                        self.mode = Mode::BlockComment(close);
                        i = self.close_block_comment(bytes, i, i + open, close, &mut spans);
                        code_start = i;
                        continue;
                    }
                    if let Some((delimiter, content_start)) = self.string_opener(bytes, i) {
                        flush_code(line, code_start, i, self.language, &mut spans);
                        let start = i;
                        let (end, closed) = delimiter.scan_end(bytes, content_start);
                        spans.push(SyntaxSpan {
                            range: start..end,
                            kind: if closed && self.is_json_key(bytes, end) {
                                SyntaxKind::Property
                            } else {
                                SyntaxKind::String
                            },
                        });
                        i = end;
                        if closed {
                            self.mode = Mode::Code;
                            code_start = i;
                        } else {
                            self.mode = Mode::String(delimiter);
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

    /// Color a block comment from `start`, searching for `close` from `from`;
    /// returns where code resumes (the line's end while the comment is open).
    fn close_block_comment(
        &mut self,
        bytes: &[u8],
        start: usize,
        from: usize,
        close: &'static [u8],
        spans: &mut Vec<SyntaxSpan>,
    ) -> usize {
        let end = match find(bytes, from, close) {
            Some(at) => {
                self.mode = Mode::Code;
                at + close.len()
            }
            None => bytes.len(),
        };
        spans.push(SyntaxSpan {
            range: start..end,
            kind: SyntaxKind::Comment,
        });
        end
    }

    fn string_opener(&self, bytes: &[u8], i: usize) -> Option<(StringDelimiter, usize)> {
        if self.language == Language::Rust
            && bytes[i] == b'r'
            && let Some((hashes, end)) = rust_raw_opener(bytes, i)
        {
            return Some((StringDelimiter::Raw(hashes), end));
        }
        is_string_quote(bytes[i], self.language, bytes, i)
            .then_some((StringDelimiter::Quoted(bytes[i]), i + 1))
    }

    fn line_comment_at(&self, bytes: &[u8], i: usize) -> bool {
        let rest = &bytes[i..];
        match self.language {
            Language::Rust
            | Language::JavaScript
            | Language::TypeScript
            | Language::Swift
            | Language::Go
            | Language::Css
            | Language::C
            | Language::Java
            | Language::Kotlin
            | Language::CSharp => rest.starts_with(b"//"),
            Language::Php => rest.starts_with(b"//") || rest[0] == b'#',
            Language::Python
            | Language::Shell
            | Language::Toml
            | Language::Yaml
            | Language::Ruby
            | Language::Dockerfile => {
                // `$#` and `${#name}` are shell expansions, not comments.
                rest[0] == b'#'
                    && !(self.language == Language::Shell
                        && i > 0
                        && matches!(bytes[i - 1], b'$' | b'{'))
            }
            Language::Sql | Language::Lua => rest.starts_with(b"--"),
            Language::Json | Language::Markdown | Language::Html | Language::Plain => false,
        }
    }

    /// Length of the opener and the closer of a block comment starting at `i`.
    fn block_comment_at(&self, bytes: &[u8], i: usize) -> Option<(usize, &'static [u8])> {
        let c_style = matches!(
            self.language,
            Language::Rust
                | Language::JavaScript
                | Language::TypeScript
                | Language::Swift
                | Language::Go
                | Language::Css
                | Language::C
                | Language::Java
                | Language::Kotlin
                | Language::CSharp
                | Language::Php
                | Language::Sql
        );
        (c_style && bytes[i..].starts_with(b"/*")).then_some((2, b"*/".as_slice()))
    }

    /// A JSON string followed by `:` is a key.
    fn is_json_key(&self, bytes: &[u8], end: usize) -> bool {
        self.language == Language::Json && next_non_blank(bytes, end) == Some(b':')
    }

    /// Keys and table headers at the start of a configuration line, before
    /// the general scan colors the value: YAML `key:` (also after `- `),
    /// TOML `key =` and `[table]`.
    fn leading_key(&self, line: &str, spans: &mut Vec<SyntaxSpan>) -> Option<usize> {
        let bytes = line.as_bytes();
        let mut start = bytes.iter().take_while(|b| b.is_ascii_whitespace()).count();
        match self.language {
            Language::Yaml => {
                while bytes[start..].starts_with(b"- ") {
                    start += 2;
                    start += bytes[start..]
                        .iter()
                        .take_while(|b| b.is_ascii_whitespace())
                        .count();
                }
                let end = yaml_key_end(bytes, start)?;
                spans.push(SyntaxSpan {
                    range: start..end,
                    kind: SyntaxKind::Property,
                });
                spans.push(SyntaxSpan {
                    range: end..end + 1,
                    kind: SyntaxKind::Punctuation,
                });
                Some(end + 1)
            }
            Language::Toml => {
                if bytes.get(start) == Some(&b'[') {
                    let close = find(bytes, start, b"]")?;
                    let end = if bytes.get(close + 1) == Some(&b']') {
                        close + 2
                    } else {
                        close + 1
                    };
                    spans.push(SyntaxSpan {
                        range: start..end,
                        kind: SyntaxKind::Type,
                    });
                    return Some(end);
                }
                let key = bytes[start..]
                    .iter()
                    .take_while(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'"')
                    })
                    .count();
                let end = start + key;
                (key > 0 && next_non_blank(bytes, end) == Some(b'=')).then(|| {
                    spans.push(SyntaxSpan {
                        range: start..end,
                        kind: SyntaxKind::Property,
                    });
                    end
                })
            }
            _ => None,
        }
    }

    /// Markdown structure and inline marks; fenced code takes the colors of
    /// the language its fence names.
    fn highlight_markdown(&mut self, line: &str) -> Vec<SyntaxSpan> {
        let bytes = line.as_bytes();
        let indent = bytes.iter().take_while(|b| **b == b' ').count();
        let trimmed = &line[indent..];
        let fence = trimmed
            .bytes()
            .next()
            .filter(|marker| matches!(marker, b'`' | b'~'))
            .map(|marker| (marker, trimmed.bytes().take_while(|b| *b == marker).count()))
            .filter(|(_, width)| *width >= 3);

        if let Some(open) = &mut self.fence {
            if fence.is_some_and(|(marker, width)| {
                marker == open.marker && width >= open.width && trimmed[width..].trim().is_empty()
            }) {
                self.fence = None;
                return vec![SyntaxSpan {
                    range: indent..line.len(),
                    kind: SyntaxKind::Punctuation,
                }];
            }
            return open.code.highlight_line(line);
        }
        if let Some((marker, width)) = fence {
            let tag = trimmed[width..]
                .trim()
                .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                .next()
                .unwrap_or("");
            self.fence = Some(Fence {
                marker,
                width,
                code: Box::new(Highlighter::new(language_from_fence(tag))),
            });
            let mut spans = vec![SyntaxSpan {
                range: indent..indent + width,
                kind: SyntaxKind::Punctuation,
            }];
            let tag_start =
                indent + width + (trimmed[width..].len() - trimmed[width..].trim_start().len());
            if !tag.is_empty() {
                spans.push(SyntaxSpan {
                    range: tag_start..tag_start + tag.len(),
                    kind: SyntaxKind::Attribute,
                });
            }
            return spans;
        }

        let mut spans = Vec::new();
        let heading = trimmed.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&heading)
            && trimmed[heading..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
        {
            spans.push(SyntaxSpan {
                range: indent..line.len(),
                kind: SyntaxKind::Heading,
            });
            return spans;
        }
        let rule = trimmed.trim_end();
        if [b'-', b'*', b'_'].iter().any(|mark| {
            rule.bytes().all(|c| c == b' ' || c == *mark)
                && rule.bytes().filter(|c| c == mark).count() >= 3
        }) {
            spans.push(SyntaxSpan {
                range: indent..line.len(),
                kind: SyntaxKind::Punctuation,
            });
            return spans;
        }
        if trimmed.starts_with('>') {
            spans.push(SyntaxSpan {
                range: indent..indent + 1,
                kind: SyntaxKind::Keyword,
            });
            spans.push(SyntaxSpan {
                range: indent + 1..line.len(),
                kind: SyntaxKind::Comment,
            });
            return spans;
        }
        let mut start = indent;
        let marker = list_marker(trimmed);
        if marker > 0 {
            spans.push(SyntaxSpan {
                range: indent..indent + marker,
                kind: SyntaxKind::Keyword,
            });
            start += marker;
            // Task lists: `- [ ]` and `- [x]`.
            let rest = &line[start..];
            let after_blank = rest.len() - rest.trim_start().len();
            let task = &rest[after_blank..];
            if task.len() >= 3
                && task.as_bytes()[0] == b'['
                && task.as_bytes()[2] == b']'
                && matches!(task.as_bytes()[1], b' ' | b'x' | b'X')
            {
                spans.push(SyntaxSpan {
                    range: start + after_blank..start + after_blank + 3,
                    kind: SyntaxKind::Constant,
                });
                start += after_blank + 3;
            }
        }
        markdown_inline(line, start, &mut spans);
        spans
    }

    /// HTML and XML: tags, attributes, values, comments, and entities. Text
    /// between tags stays plain.
    fn highlight_markup(&mut self, line: &str) -> Vec<SyntaxSpan> {
        let bytes = line.as_bytes();
        let mut spans = Vec::new();
        let mut i = 0;
        let push = |spans: &mut Vec<SyntaxSpan>, range: Range<usize>, kind| {
            spans.push(SyntaxSpan { range, kind });
        };
        while i < bytes.len() {
            if let Mode::BlockComment(close) = self.mode {
                i = self.close_block_comment(bytes, i, i, close, &mut spans);
                continue;
            }
            if let Mode::String(delimiter) = self.mode {
                let start = i;
                let (end, closed) = delimiter.scan_end(bytes, i);
                push(&mut spans, start..end, SyntaxKind::String);
                i = end;
                if closed {
                    self.mode = Mode::Code;
                }
                continue;
            }
            let b = bytes[i];
            if self.in_tag {
                match b {
                    b'>' | b'/' | b'?' => {
                        if b == b'>' {
                            self.in_tag = false;
                        }
                        push(&mut spans, i..i + 1, SyntaxKind::Punctuation);
                        i += 1;
                    }
                    b'"' | b'\'' => {
                        let (end, closed) = scan_string_end(bytes, i + 1, b);
                        push(&mut spans, i..end, SyntaxKind::String);
                        if !closed {
                            self.mode = Mode::String(StringDelimiter::Quoted(b));
                        }
                        i = end;
                    }
                    b'=' => {
                        push(&mut spans, i..i + 1, SyntaxKind::Operator);
                        i += 1;
                    }
                    _ if is_markup_name(b) => {
                        let end = i + bytes[i..]
                            .iter()
                            .take_while(|b| is_markup_name(**b))
                            .count();
                        push(&mut spans, i..end, SyntaxKind::Attribute);
                        i = end;
                    }
                    _ => i += 1,
                }
                continue;
            }
            if bytes[i..].starts_with(b"<!--") {
                self.mode = Mode::BlockComment(b"-->");
                i = self.close_block_comment(bytes, i, i + 4, b"-->", &mut spans);
                continue;
            }
            if b == b'<'
                && bytes.get(i + 1).is_some_and(|next| {
                    next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?')
                })
            {
                let start = i;
                i += 1;
                if matches!(bytes[i], b'/' | b'!' | b'?') {
                    i += 1;
                }
                push(&mut spans, start..i, SyntaxKind::Punctuation);
                let name = bytes[i..]
                    .iter()
                    .take_while(|b| is_markup_name(**b))
                    .count();
                if name > 0 {
                    push(&mut spans, i..i + name, SyntaxKind::Tag);
                    i += name;
                }
                self.in_tag = true;
                continue;
            }
            if b == b'&' {
                let name = bytes[i + 1..]
                    .iter()
                    .take_while(|b| b.is_ascii_alphanumeric() || **b == b'#')
                    .count();
                if name > 0 && bytes.get(i + 1 + name) == Some(&b';') {
                    push(&mut spans, i..i + name + 2, SyntaxKind::Constant);
                    i += name + 2;
                    continue;
                }
            }
            i += 1;
        }
        spans
    }
}

fn is_markup_name(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.' | b'@')
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

fn next_non_blank(bytes: &[u8], from: usize) -> Option<u8> {
    bytes
        .get(from..)?
        .iter()
        .copied()
        .find(|b| !b.is_ascii_whitespace())
}

/// End of a YAML mapping key starting at `start`: a quoted key, or a bare
/// one without blanks, followed by `:` and then a blank or the line's end.
fn yaml_key_end(bytes: &[u8], start: usize) -> Option<usize> {
    let first = *bytes.get(start)?;
    let end = match first {
        b'"' | b'\'' => match scan_string_end(bytes, start + 1, first) {
            (end, true) => end,
            _ => return None,
        },
        b'#' | b'[' | b'{' | b'&' | b'*' | b'!' | b'|' | b'>' | b'%' | b'@' | b'`' => return None,
        _ => {
            let len = bytes[start..]
                .iter()
                .take_while(|b| !matches!(b, b':' | b'#' | b' ' | b'\t'))
                .count();
            start + len
        }
    };
    let colon =
        bytes.get(end) == Some(&b':') && bytes.get(end + 1).is_none_or(|b| b.is_ascii_whitespace());
    (end > start && colon).then_some(end)
}

/// Width of a list marker (`- `, `* `, `+ `, `1. `, `1) `) at the start of `text`.
fn list_marker(text: &str) -> usize {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*' | b'+') && bytes[1] == b' ' {
        return 1;
    }
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if (1..=9).contains(&digits)
        && matches!(bytes.get(digits), Some(b'.' | b')'))
        && bytes.get(digits + 1) == Some(&b' ')
    {
        return digits + 1;
    }
    0
}

/// Inline Markdown from `start`: code, strong, emphasis, links, autolinks,
/// and HTML comments. Every mark is ASCII, so ranges stay on char boundaries.
fn markdown_inline(line: &str, start: usize, spans: &mut Vec<SyntaxSpan>) {
    let bytes = line.as_bytes();
    let mut i = start;
    let push = |spans: &mut Vec<SyntaxSpan>, range: Range<usize>, kind| {
        spans.push(SyntaxSpan { range, kind });
    };
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'`' {
            let ticks = bytes[i..].iter().take_while(|b| **b == b'`').count();
            let fence = &bytes[i..i + ticks];
            if let Some(close) = find(bytes, i + ticks, fence) {
                push(spans, i..close + ticks, SyntaxKind::String);
                i = close + ticks;
                continue;
            }
            i += ticks;
            continue;
        }
        if (bytes[i..].starts_with(b"**") || bytes[i..].starts_with(b"__"))
            && let Some(close) = find(bytes, i + 2, &bytes[i..i + 2])
            && close > i + 2
        {
            push(spans, i..close + 2, SyntaxKind::Strong);
            i = close + 2;
            continue;
        }
        if matches!(b, b'*' | b'_')
            && bytes
                .get(i + 1)
                .is_some_and(|next| !next.is_ascii_whitespace() && *next != b)
            && (b == b'*' || i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
            && let Some(close) = find(bytes, i + 1, &[b])
            && !bytes[close - 1].is_ascii_whitespace()
        {
            push(spans, i..close + 1, SyntaxKind::Emphasis);
            i = close + 1;
            continue;
        }
        if b == b'['
            && let Some(close) = find(bytes, i + 1, b"]")
        {
            if bytes.get(close + 1) == Some(&b'(')
                && let Some(end) = find(bytes, close + 2, b")")
            {
                push(spans, i..close + 1, SyntaxKind::Link);
                push(spans, close + 1..end + 1, SyntaxKind::Comment);
                i = end + 1;
                continue;
            }
            if bytes.get(close + 1) == Some(&b':') {
                // Reference definition: `[label]: url`.
                push(spans, i..close + 2, SyntaxKind::Link);
                push(spans, close + 2..bytes.len(), SyntaxKind::Comment);
                return;
            }
        }
        if bytes[i..].starts_with(b"<!--") {
            let end = find(bytes, i + 4, b"-->").map_or(bytes.len(), |at| at + 3);
            push(spans, i..end, SyntaxKind::Comment);
            i = end;
            continue;
        }
        if (bytes[i..].starts_with(b"https://") || bytes[i..].starts_with(b"http://"))
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            let end = i + bytes[i..]
                .iter()
                .take_while(|b| !b.is_ascii_whitespace() && !matches!(b, b')' | b'>' | b'"'))
                .count();
            push(spans, i..end, SyntaxKind::Link);
            i = end;
            continue;
        }
        i += 1;
    }
}

/// The language a Markdown fence names (`rust`, `ts`, `sh`, …).
pub fn language_from_fence(tag: &str) -> Language {
    match tag.to_ascii_lowercase().as_str() {
        "" => Language::Plain,
        "rust" => Language::Rust,
        "javascript" | "node" => Language::JavaScript,
        "typescript" => Language::TypeScript,
        "python" | "python3" => Language::Python,
        "golang" => Language::Go,
        "shell" | "console" | "shellsession" | "sh-session" => Language::Shell,
        "c++" | "objective-c" | "objc" => Language::C,
        "kotlin" => Language::Kotlin,
        "csharp" | "c#" => Language::CSharp,
        "ruby" => Language::Ruby,
        "docker" => Language::Dockerfile,
        "markdown" => Language::Markdown,
        "xml" | "svg" | "vue" | "svelte" => Language::Html,
        tag => language_from_path(&format!("snippet.{tag}")),
    }
}

fn is_string_quote(b: u8, language: Language, bytes: &[u8], i: usize) -> bool {
    if b == b'"' || b == b'`' {
        return true;
    }
    if b != b'\'' {
        return false;
    }
    // An apostrophe inside a word (`don't`) is prose in configuration and
    // shell files, where an opened string would color the following lines.
    if matches!(
        language,
        Language::Yaml | Language::Toml | Language::Shell | Language::Dockerfile
    ) && i > 0
        && bytes[i - 1].is_ascii_alphanumeric()
    {
        return false;
    }
    // Rust lifetime `'a` / `'static` — not a char/string.
    if language == Language::Rust
        && i + 1 < bytes.len()
        && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_')
        && bytes.get(i + 2) != Some(&b'\'')
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
        // `#include` / `#define` in C, `#[derive` / `#![allow` in Rust.
        if b == b'#' && matches!(language, Language::C | Language::Rust) {
            let s = i;
            i += 1;
            if language == Language::Rust {
                if bytes.get(i) == Some(&b'!') {
                    i += 1;
                }
                if bytes.get(i) != Some(&b'[') {
                    continue;
                }
                i += 1;
            }
            while i < end && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            spans.push(SyntaxSpan {
                range: s..i,
                kind: if language == Language::C {
                    SyntaxKind::Macro
                } else {
                    SyntaxKind::Attribute
                },
            });
            continue;
        }
        // Decorators and annotations: `@Override`, `@media`, `@property`.
        if b == b'@' {
            let s = i;
            i += 1;
            while i < end
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'-' | b'.'))
            {
                i += 1;
            }
            spans.push(SyntaxSpan {
                range: s..i,
                kind: SyntaxKind::Attribute,
            });
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
        // Two-char operators — compare bytes so UTF-8 lines cannot panic on a
        // mid-character slice (e.g. "é" in a diff).
        if i + 1 < end {
            let two = [bytes[i], bytes[i + 1]];
            if matches!(
                two,
                [b'=', b'>']
                    | [b'-', b'>']
                    | [b':', b':']
                    | [b'=', b'=']
                    | [b'!', b'=']
                    | [b'<', b'=']
                    | [b'>', b'=']
                    | [b'&', b'&']
                    | [b'|', b'|']
                    | [b'+', b'=']
                    | [b'-', b'=']
                    | [b'*', b'=']
                    | [b'/', b'=']
                    | [b'<', b'<']
                    | [b'>', b'>']
                    | [b'.', b'.']
                    | [b'?', b'?']
                    | [b'?', b'.']
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
            b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b';' | b'.'
        ) {
            spans.push(SyntaxSpan {
                range: i..i + 1,
                kind: SyntaxKind::Punctuation,
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
    match language {
        // Plain YAML scalars are text; Dockerfile arguments are shell words.
        Language::Yaml => return SyntaxKind::String,
        Language::Dockerfile => return SyntaxKind::Default,
        // `$name` and `${name}` expand variables.
        Language::Shell | Language::Php if word.starts_with('$') => {
            return SyntaxKind::Constant;
        }
        _ => {}
    }
    // `MAX_LEN` reads as a constant before its capital makes it a type.
    if word.len() > 1
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && word.chars().any(|c| c.is_ascii_uppercase())
        && language != Language::Sql
    {
        return SyntaxKind::Constant;
    }
    if word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return SyntaxKind::Type;
    }
    if matches!(after, AfterIdent::Call) {
        return SyntaxKind::Function;
    }
    SyntaxKind::Default
}

fn is_literal_constant(word: &str, language: Language) -> bool {
    matches!(
        word,
        "true" | "false" | "null" | "undefined" | "None" | "nil"
    ) || (language == Language::Python && matches!(word, "True" | "False"))
        || (language == Language::Rust && matches!(word, "Some" | "Ok" | "Err"))
        || (language == Language::Yaml
            && matches!(word, "yes" | "no" | "on" | "off" | "True" | "False"))
        || (language == Language::C && matches!(word, "NULL" | "nullptr" | "YES" | "NO"))
}

fn is_keyword(word: &str, language: Language) -> bool {
    match language {
        Language::Rust => RUST_KEYWORDS.contains(&word),
        Language::JavaScript | Language::TypeScript => JS_KEYWORDS.contains(&word),
        Language::Python => PYTHON_KEYWORDS.contains(&word),
        Language::Swift => SWIFT_KEYWORDS.contains(&word),
        Language::Go => GO_KEYWORDS.contains(&word),
        Language::Shell => SHELL_KEYWORDS.contains(&word),
        Language::Css => CSS_KEYWORDS.contains(&word),
        Language::C => C_KEYWORDS.contains(&word),
        Language::Java => JAVA_KEYWORDS.contains(&word),
        Language::Kotlin => KOTLIN_KEYWORDS.contains(&word),
        Language::CSharp => CSHARP_KEYWORDS.contains(&word),
        Language::Ruby => RUBY_KEYWORDS.contains(&word),
        Language::Php => PHP_KEYWORDS.contains(&word.to_ascii_lowercase().as_str()),
        Language::Lua => LUA_KEYWORDS.contains(&word),
        // SQL is case-insensitive; Dockerfile instructions are uppercase by
        // convention, so lowercase shell words after them stay plain.
        Language::Sql => SQL_KEYWORDS.contains(&word.to_ascii_uppercase().as_str()),
        Language::Dockerfile => DOCKERFILE_KEYWORDS.contains(&word),
        Language::Json | Language::Toml | Language::Yaml => {
            matches!(word, "true" | "false" | "null")
        }
        Language::Html | Language::Markdown | Language::Plain => false,
    }
}

pub fn language_from_path(path: &str) -> Language {
    let path = std::path::Path::new(path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // Files known by name rather than extension.
    match name.as_str() {
        "dockerfile" | "containerfile" => return Language::Dockerfile,
        name if name.starts_with("dockerfile.") || name.ends_with(".dockerfile") => {
            return Language::Dockerfile;
        }
        "makefile" | "gnumakefile" | "justfile" | ".gitignore" | ".dockerignore" | ".env"
        | ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" | ".zprofile" | ".npmrc"
        | ".editorconfig" => return Language::Shell,
        name if name.starts_with(".env.") => return Language::Shell,
        "gemfile" | "rakefile" | "podfile" | "fastfile" | "brewfile" | "vagrantfile" => {
            return Language::Ruby;
        }
        "cargo.lock" | "uv.lock" | "poetry.lock" => return Language::Toml,
        _ => {}
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Language::Rust,
        "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
        "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
        "py" | "pyi" | "pyw" => Language::Python,
        "swift" => Language::Swift,
        "go" => Language::Go,
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "ipp" | "m" | "mm" | "ino"
        | "cu" | "metal" => Language::C,
        "java" | "groovy" | "gradle" | "scala" | "sc" | "dart" => Language::Java,
        "kt" | "kts" => Language::Kotlin,
        "cs" => Language::CSharp,
        "rb" | "rake" | "gemspec" | "ru" | "erb" => Language::Ruby,
        "php" | "phtml" => Language::Php,
        "sql" | "psql" | "pgsql" | "mysql" => Language::Sql,
        "lua" => Language::Lua,
        "json" | "jsonc" | "json5" | "jsonl" | "geojson" | "webmanifest" => Language::Json,
        "toml" | "ini" | "cfg" | "conf" | "properties" => Language::Toml,
        "md" | "mdx" | "markdown" => Language::Markdown,
        "sh" | "bash" | "zsh" | "fish" | "env" | "mk" | "make" => Language::Shell,
        "css" | "scss" | "sass" | "less" => Language::Css,
        "html" | "htm" | "xhtml" | "xml" | "svg" | "plist" | "xsd" | "xsl" | "vue" | "svelte"
        | "astro" | "storyboard" | "xib" => Language::Html,
        "yml" | "yaml" => Language::Yaml,
        _ => Language::Plain,
    }
}

impl SyntaxKind {
    pub fn color(self) -> Rgba {
        let theme = colors();
        let dark = theme.is_dark();
        match self {
            Self::Keyword | Self::Macro => mix(
                theme.accent,
                rgb(if dark { 0xc4b5fd } else { 0x6d28d9 }),
                0.62,
            ),
            Self::String => mix(
                theme.success,
                rgb(if dark { 0xc3e88d } else { 0x166534 }),
                0.28,
            ),
            Self::Comment => mix(theme.muted, theme.subtle, 0.18),
            Self::Number | Self::Constant => mix(
                theme.warning,
                rgb(if dark { 0xffcb6b } else { 0xb45309 }),
                0.35,
            ),
            Self::Type | Self::Attribute => mix(
                theme.warning,
                rgb(if dark { 0xfde68a } else { 0xa16207 }),
                0.42,
            ),
            Self::Function => theme.accent,
            Self::Operator => mix(theme.muted, theme.accent, 0.22),
            Self::Punctuation => theme.muted,
            Self::Property => mix(
                theme.accent,
                rgb(if dark { 0x7dd3fc } else { 0x0369a1 }),
                0.55,
            ),
            Self::Tag => mix(
                theme.danger,
                rgb(if dark { 0xf472b6 } else { 0xbe185d }),
                0.45,
            ),
            Self::Heading => mix(
                theme.accent,
                rgb(if dark { 0x93c5fd } else { 0x1d4ed8 }),
                0.35,
            ),
            Self::Link => mix(
                theme.accent,
                rgb(if dark { 0x67e8f9 } else { 0x0e7490 }),
                0.4,
            ),
            Self::Strong | Self::Emphasis | Self::Default => theme.foreground,
        }
    }

    pub fn highlight_style(self) -> HighlightStyle {
        let mut style = HighlightStyle {
            color: Some(self.color().into()),
            ..Default::default()
        };
        match self {
            Self::Comment | Self::Emphasis => style.font_style = Some(FontStyle::Italic),
            Self::Keyword | Self::Macro => style.font_weight = Some(FontWeight::MEDIUM),
            Self::Heading | Self::Strong => style.font_weight = Some(FontWeight::BOLD),
            _ => {}
        }
        style
    }
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "try", "union", "box",
];

const JS_KEYWORDS: &[&str] = &[
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
];

const PYTHON_KEYWORDS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while",
    "with", "yield", "match", "case",
];

const SWIFT_KEYWORDS: &[&str] = &[
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
];

const GO_KEYWORDS: &[&str] = &[
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
];

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done",
    "in", "function", "select", "return", "exit", "export", "local", "readonly", "declare",
    "unset", "shift", "source", "alias",
];

const CSS_KEYWORDS: &[&str] = &[
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
];

const C_KEYWORDS: &[&str] = &[
    "auto",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "constexpr",
    "continue",
    "decltype",
    "default",
    "delete",
    "do",
    "double",
    "else",
    "enum",
    "explicit",
    "extern",
    "false",
    "final",
    "float",
    "for",
    "friend",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "operator",
    "override",
    "private",
    "protected",
    "public",
    "register",
    "restrict",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "static_cast",
    "dynamic_cast",
    "reinterpret_cast",
    "const_cast",
    "struct",
    "switch",
    "template",
    "this",
    "throw",
    "true",
    "try",
    "typedef",
    "typename",
    "union",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
    "concept",
    "requires",
    "co_await",
    "co_return",
    "co_yield",
    "self",
    "super",
    "id",
    "instancetype",
];

const JAVA_KEYWORDS: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "record",
    "return",
    "sealed",
    "permits",
    "short",
    "static",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "var",
    "void",
    "volatile",
    "while",
    "yield",
    "def",
    "val",
    "object",
    "trait",
    "async",
    "await",
    "late",
    "required",
    "dynamic",
    "extension",
    "mixin",
    "with",
];

const KOTLIN_KEYWORDS: &[&str] = &[
    "abstract",
    "annotation",
    "as",
    "break",
    "by",
    "catch",
    "class",
    "companion",
    "const",
    "constructor",
    "continue",
    "crossinline",
    "data",
    "do",
    "else",
    "enum",
    "external",
    "final",
    "finally",
    "for",
    "fun",
    "get",
    "if",
    "import",
    "in",
    "infix",
    "init",
    "inline",
    "inner",
    "interface",
    "internal",
    "is",
    "lateinit",
    "noinline",
    "object",
    "open",
    "operator",
    "out",
    "override",
    "package",
    "private",
    "protected",
    "public",
    "reified",
    "return",
    "sealed",
    "set",
    "super",
    "suspend",
    "tailrec",
    "this",
    "throw",
    "try",
    "typealias",
    "val",
    "var",
    "vararg",
    "when",
    "where",
    "while",
];

const CSHARP_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "base",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "checked",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "do",
    "double",
    "dynamic",
    "else",
    "enum",
    "event",
    "explicit",
    "extern",
    "finally",
    "fixed",
    "float",
    "for",
    "foreach",
    "get",
    "goto",
    "if",
    "implicit",
    "in",
    "init",
    "int",
    "interface",
    "internal",
    "is",
    "lock",
    "long",
    "nameof",
    "namespace",
    "new",
    "object",
    "operator",
    "out",
    "override",
    "params",
    "partial",
    "private",
    "protected",
    "public",
    "readonly",
    "record",
    "ref",
    "return",
    "sbyte",
    "sealed",
    "set",
    "short",
    "sizeof",
    "stackalloc",
    "static",
    "string",
    "struct",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "uint",
    "ulong",
    "unchecked",
    "unsafe",
    "ushort",
    "using",
    "var",
    "virtual",
    "void",
    "volatile",
    "where",
    "while",
    "yield",
];

const RUBY_KEYWORDS: &[&str] = &[
    "alias",
    "and",
    "begin",
    "break",
    "case",
    "class",
    "def",
    "defined",
    "do",
    "else",
    "elsif",
    "end",
    "ensure",
    "for",
    "if",
    "in",
    "module",
    "next",
    "not",
    "or",
    "redo",
    "rescue",
    "retry",
    "return",
    "self",
    "super",
    "then",
    "undef",
    "unless",
    "until",
    "when",
    "while",
    "yield",
    "require",
    "require_relative",
    "attr_accessor",
    "attr_reader",
    "attr_writer",
    "private",
    "protected",
    "public",
    "include",
    "extend",
    "raise",
    "lambda",
    "proc",
];

const PHP_KEYWORDS: &[&str] = &[
    "abstract",
    "and",
    "array",
    "as",
    "break",
    "callable",
    "case",
    "catch",
    "class",
    "clone",
    "const",
    "continue",
    "declare",
    "default",
    "do",
    "echo",
    "else",
    "elseif",
    "empty",
    "enum",
    "extends",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "if",
    "implements",
    "include",
    "include_once",
    "instanceof",
    "interface",
    "isset",
    "list",
    "match",
    "namespace",
    "new",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "require_once",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "try",
    "unset",
    "use",
    "var",
    "while",
    "xor",
    "yield",
];

const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if", "in", "local",
    "not", "or", "repeat", "return", "then", "until", "while", "require", "self",
];

/// Uppercase: SQL compares case-insensitively.
const SQL_KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "AND",
    "AS",
    "ASC",
    "BEGIN",
    "BETWEEN",
    "BIGINT",
    "BIGSERIAL",
    "BOOL",
    "BOOLEAN",
    "BY",
    "CASCADE",
    "CASE",
    "CHAR",
    "CHECK",
    "COLUMN",
    "COMMIT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "DATE",
    "DECIMAL",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DO",
    "DOUBLE",
    "DROP",
    "ELSE",
    "ENABLE",
    "END",
    "EXECUTE",
    "EXISTS",
    "EXTENSION",
    "FALSE",
    "FLOAT",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "FUNCTION",
    "GRANT",
    "GROUP",
    "HAVING",
    "IF",
    "ILIKE",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INT",
    "INTEGER",
    "INTO",
    "IS",
    "JOIN",
    "JSON",
    "JSONB",
    "KEY",
    "LANGUAGE",
    "LEFT",
    "LEVEL",
    "LIKE",
    "LIMIT",
    "NOT",
    "NULL",
    "NUMERIC",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "POLICY",
    "PRECISION",
    "PRIMARY",
    "PROCEDURE",
    "REAL",
    "RECURSIVE",
    "REFERENCES",
    "RENAME",
    "REPLACE",
    "RETURNING",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "ROW",
    "SCHEMA",
    "SECURITY",
    "SELECT",
    "SERIAL",
    "SET",
    "SMALLINT",
    "TABLE",
    "TEXT",
    "THEN",
    "TIME",
    "TIMESTAMP",
    "TIMESTAMPTZ",
    "TO",
    "TRANSACTION",
    "TRIGGER",
    "TRUE",
    "TYPE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "UUID",
    "VALUES",
    "VARCHAR",
    "VIEW",
    "WHEN",
    "WHERE",
    "WITH",
];

const DOCKERFILE_KEYWORDS: &[&str] = &[
    "ADD",
    "ARG",
    "AS",
    "CMD",
    "COPY",
    "ENTRYPOINT",
    "ENV",
    "EXPOSE",
    "FROM",
    "HEALTHCHECK",
    "LABEL",
    "MAINTAINER",
    "ONBUILD",
    "RUN",
    "SHELL",
    "STOPSIGNAL",
    "USER",
    "VOLUME",
    "WORKDIR",
];

/// Expand tabs to spaces so diff columns line up like an editor (width 4).
pub fn expand_tabs(text: &str) -> String {
    const TAB_WIDTH: usize = 4;
    let mut out = String::with_capacity(text.len());
    let mut column = 0usize;
    for character in text.chars() {
        if character == '\t' {
            let spaces = TAB_WIDTH - (column % TAB_WIDTH);
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(character);
            column = if character == '\n' { 0 } else { column + 1 };
        }
    }
    out
}

/// Highlight a multi-line snippet as one sequence, with byte ranges into
/// `code` (lines split on `\n`), ready for a `StyledText`.
pub fn highlight_code(code: &str, language: Language) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut highlighter = Highlighter::new(language);
    let mut runs = Vec::new();
    let mut offset = 0;
    for line in code.split('\n') {
        for span in highlighter.highlight_line(line) {
            runs.push((
                offset + span.range.start..offset + span.range.end,
                span.kind.highlight_style(),
            ));
        }
        offset += line.len() + 1;
    }
    runs
}

/// Highlight every text row of a diff in order (preserves multi-line comment/string state).
/// `texts` must already be tab-expanded so span offsets match the painted lines.
pub fn highlight_diff_rows(
    path: &str,
    rows: &[GitDiffRow],
    texts: &[&str],
) -> Vec<Vec<SyntaxSpan>> {
    let mut highlighter = Highlighter::for_path(path);
    rows.iter()
        .zip(texts)
        .map(|(row, text)| match row.kind {
            GitDiffRowKind::Context | GitDiffRowKind::Addition | GitDiffRowKind::Deletion => {
                highlighter.highlight_line(text)
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
    fn rust_character_literals_do_not_start_a_multiline_string() {
        let mut highlighter = Highlighter::new(Language::Rust);
        let line = "let chars = ['a', '_', 'é', '\\n']; let after = 1;";
        let spans = highlighter.highlight_line(line);
        let strings: Vec<_> = spans
            .iter()
            .filter(|span| span.kind == SyntaxKind::String)
            .map(|span| line_slice(line, &span.range))
            .collect();

        assert_eq!(strings, ["'a'", "'_'", "'é'", "'\\n'"]);
        assert_eq!(
            spans
                .iter()
                .filter(|span| span.kind == SyntaxKind::Keyword)
                .count(),
            2
        );
        assert!(
            highlighter
                .highlight_line("let next = 2;")
                .iter()
                .any(|span| span.kind == SyntaxKind::Keyword)
        );
    }

    #[test]
    fn rust_lifetimes_are_not_character_literals() {
        let mut highlighter = Highlighter::new(Language::Rust);
        let spans = highlighter.highlight_line("fn borrow<'a>(value: &'a str) -> &'static str {");
        assert!(spans.iter().all(|span| span.kind != SyntaxKind::String));
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
    fn multiline_strings_keep_their_delimiter_and_resume_code_after_closing() {
        for (opening, closing, string) in [
            (
                "let value = \"open\\\" still",
                "close\"; let after = 2;",
                "close\"",
            ),
            (
                "let value = r##\"open \"# // still raw",
                "close\"##; let after = 2;",
                "close\"##",
            ),
        ] {
            let mut highlighter = Highlighter::new(Language::Rust);
            let first = highlighter.highlight_line(opening);
            assert!(first.iter().any(|span| span.kind == SyntaxKind::String));
            assert!(first.iter().all(|span| span.kind != SyntaxKind::Comment));
            let spans = highlighter.highlight_line(closing);
            assert_eq!(spans[0].kind, SyntaxKind::String);
            assert_eq!(line_slice(closing, &spans[0].range), string);
            assert!(spans.iter().any(|span| {
                span.kind == SyntaxKind::Keyword && line_slice(closing, &span.range) == "let"
            }));
        }
    }

    #[test]
    fn language_from_extension() {
        assert_eq!(language_from_path("src/main.rs"), Language::Rust);
        assert_eq!(language_from_path("a/b/theme.ts"), Language::TypeScript);
        assert_eq!(language_from_path("README.md"), Language::Markdown);
    }

    #[test]
    fn expand_tabs_aligns_to_four_columns() {
        assert_eq!(expand_tabs("a\tb"), "a   b");
        assert_eq!(expand_tabs("\tfn"), "    fn");
        assert_eq!(expand_tabs("abcd\tx"), "abcd    x");
    }

    #[test]
    fn utf8_diff_lines_do_not_panic() {
        let rows = [
            GitDiffRow {
                old_line: Some(1),
                new_line: Some(1),
                kind: GitDiffRowKind::Context,
                text: "autor = \"Rubén Collao\"".into(),
            },
            GitDiffRow {
                old_line: None,
                new_line: Some(2),
                kind: GitDiffRowKind::Addition,
                text: "let ok => true; // café".into(),
            },
        ];
        let texts: Vec<_> = rows.iter().map(|row| expand_tabs(&row.text)).collect();
        let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let spans = highlight_diff_rows("src/main.rs", &rows, &text_refs);
        assert_eq!(spans.len(), 2);
        assert!(
            spans[1]
                .iter()
                .any(|span| span.kind == SyntaxKind::Operator)
        );
    }

    fn kinds_of<'a>(line: &'a str, spans: &[SyntaxSpan]) -> Vec<(&'a str, SyntaxKind)> {
        spans
            .iter()
            .map(|span| (line_slice(line, &span.range), span.kind))
            .collect()
    }

    fn has(line: &str, spans: &[SyntaxSpan], text: &str, kind: SyntaxKind) -> bool {
        kinds_of(line, spans).contains(&(text, kind))
    }

    #[test]
    fn markdown_structure_and_inline_marks() {
        let mut h = Highlighter::new(Language::Markdown);
        let heading = "## Install Vibra";
        assert!(has(
            heading,
            &h.highlight_line(heading),
            heading,
            SyntaxKind::Heading
        ));
        let item = "- Run `cargo build` with **care** and *patience*, see [docs](https://x.dev).";
        let spans = h.highlight_line(item);
        assert!(has(item, &spans, "-", SyntaxKind::Keyword));
        assert!(has(item, &spans, "`cargo build`", SyntaxKind::String));
        assert!(has(item, &spans, "**care**", SyntaxKind::Strong));
        assert!(has(item, &spans, "*patience*", SyntaxKind::Emphasis));
        assert!(has(item, &spans, "[docs]", SyntaxKind::Link));
        // snake_case words are not emphasis.
        let prose = "call snake_case_name here";
        assert!(h.highlight_line(prose).is_empty());
        let quote = "> Note: careful";
        assert!(has(
            quote,
            &h.highlight_line(quote),
            " Note: careful",
            SyntaxKind::Comment
        ));
    }

    #[test]
    fn markdown_fences_highlight_their_language_until_closed() {
        let mut h = Highlighter::new(Language::Markdown);
        let open = "```rust";
        let spans = h.highlight_line(open);
        assert!(has(open, &spans, "rust", SyntaxKind::Attribute));
        let code = "let total = 1;";
        assert!(has(
            code,
            &h.highlight_line(code),
            "let",
            SyntaxKind::Keyword
        ));
        assert!(has(
            "```",
            &h.highlight_line("```"),
            "```",
            SyntaxKind::Punctuation
        ));
        // Back to prose: `let` is just a word again.
        assert!(h.highlight_line(code).is_empty());
    }

    #[test]
    fn configuration_keys_read_as_properties() {
        let mut json = Highlighter::new(Language::Json);
        let line = r#"  "name": "vibra", "port": 8080"#;
        let spans = json.highlight_line(line);
        assert!(has(line, &spans, r#""name""#, SyntaxKind::Property));
        assert!(has(line, &spans, r#""vibra""#, SyntaxKind::String));
        assert!(has(line, &spans, r#""port""#, SyntaxKind::Property));

        let mut yaml = Highlighter::new(Language::Yaml);
        let line = "  - name: Build the app # note";
        let spans = yaml.highlight_line(line);
        assert!(has(line, &spans, "name", SyntaxKind::Property));
        assert!(has(line, &spans, "Build", SyntaxKind::String));
        assert!(has(line, &spans, "# note", SyntaxKind::Comment));
        // An apostrophe in prose does not open a string for the next lines.
        let prose = "description: Don't stop";
        yaml.highlight_line(prose);
        let next = "version: 2";
        assert!(has(
            next,
            &yaml.highlight_line(next),
            "version",
            SyntaxKind::Property
        ));

        let mut toml = Highlighter::new(Language::Toml);
        let table = "[dependencies]";
        assert!(has(
            table,
            &toml.highlight_line(table),
            table,
            SyntaxKind::Type
        ));
        let line = r#"serde = { version = "1.0" }"#;
        assert!(has(
            line,
            &toml.highlight_line(line),
            "serde",
            SyntaxKind::Property
        ));
    }

    #[test]
    fn markup_tags_attributes_and_comments() {
        let mut h = Highlighter::new(Language::Html);
        let line = r#"<a href="/docs" class='x'>Don't &amp; go</a> <!-- open"#;
        let spans = h.highlight_line(line);
        assert!(has(line, &spans, "a", SyntaxKind::Tag));
        assert!(has(line, &spans, "href", SyntaxKind::Attribute));
        assert!(has(line, &spans, r#""/docs""#, SyntaxKind::String));
        assert!(has(line, &spans, "'x'", SyntaxKind::String));
        assert!(has(line, &spans, "&amp;", SyntaxKind::Constant));
        // Text between tags stays plain, apostrophes included.
        assert!(
            !kinds_of(line, &spans)
                .iter()
                .any(|(text, _)| text.contains("Don"))
        );
        let next = "still a comment -->";
        assert!(has(
            next,
            &h.highlight_line(next),
            next,
            SyntaxKind::Comment
        ));
    }

    #[test]
    fn more_languages_by_extension_and_name() {
        assert_eq!(language_from_path("src/main.cpp"), Language::C);
        assert_eq!(language_from_path("App.kt"), Language::Kotlin);
        assert_eq!(language_from_path("db/schema.sql"), Language::Sql);
        assert_eq!(language_from_path("Dockerfile"), Language::Dockerfile);
        assert_eq!(language_from_path("Gemfile"), Language::Ruby);
        assert_eq!(language_from_path(".env.local"), Language::Shell);

        let mut c = Highlighter::new(Language::C);
        let line = "#include <stdio.h> // io";
        let spans = c.highlight_line(line);
        assert!(has(line, &spans, "#include", SyntaxKind::Macro));
        assert!(has(line, &spans, "// io", SyntaxKind::Comment));

        let mut sql = Highlighter::new(Language::Sql);
        let line = "select id from users -- all";
        let spans = sql.highlight_line(line);
        assert!(has(line, &spans, "select", SyntaxKind::Keyword));
        assert!(has(line, &spans, "-- all", SyntaxKind::Comment));

        let mut rust = Highlighter::new(Language::Rust);
        let line = "#[derive(Debug)] const MAX_LEN: usize = 4;";
        let spans = rust.highlight_line(line);
        assert!(has(line, &spans, "#[derive", SyntaxKind::Attribute));
        assert!(has(line, &spans, "MAX_LEN", SyntaxKind::Constant));
    }

    #[test]
    fn every_language_keeps_spans_on_char_boundaries() {
        let languages = [
            Language::Rust,
            Language::JavaScript,
            Language::TypeScript,
            Language::Python,
            Language::Swift,
            Language::Go,
            Language::C,
            Language::Java,
            Language::Kotlin,
            Language::CSharp,
            Language::Ruby,
            Language::Php,
            Language::Sql,
            Language::Lua,
            Language::Dockerfile,
            Language::Json,
            Language::Toml,
            Language::Markdown,
            Language::Shell,
            Language::Css,
            Language::Html,
            Language::Yaml,
        ];
        let lines = [
            "café: «ñandú» # comentário",
            "- [x] **négrita** _énfasis_ `código` [enlace](https://ejemplo.es/ñ)",
            "<élément attr=\"valör\">texto &eacute; ☃</élément>",
            "\"clé\": \"日本語\", 'unclosed ü",
            "#[attr] /* 注释 */ let s = r#\"raw ☃\"#; -- ✓",
            "```ñ",
            "  ☃ ☃ ☃",
            "[[tabla]] ü = 'ö'",
            "```",
        ];
        for language in languages {
            let mut highlighter = Highlighter::new(language);
            for line in lines {
                for span in highlighter.highlight_line(line) {
                    assert!(
                        span.range.start <= span.range.end
                            && span.range.end <= line.len()
                            && line.is_char_boundary(span.range.start)
                            && line.is_char_boundary(span.range.end),
                        "{language:?} split {line:?} at {:?}",
                        span.range
                    );
                }
            }
        }
    }

    fn line_slice<'a>(line: &'a str, range: &Range<usize>) -> &'a str {
        &line[range.clone()]
    }
}
