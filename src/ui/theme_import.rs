//! Parsers for user-supplied Warp YAML and Ghostty theme files.

use std::path::Path;

const MAX_THEME_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedScheme {
    pub name: Option<String>,
    /// `Some(true)` is Warp `details: darker`; `Some(false)` is `lighter`.
    pub dark: Option<bool>,
    pub background: u32,
    pub foreground: u32,
    pub accent: Option<u32>,
    pub cursor: Option<u32>,
    pub ansi: [Option<u32>; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    message: String,
}

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

pub fn parse_theme_text(text: &str) -> Result<ImportedScheme, ParseError> {
    if looks_like_ghostty(text) {
        parse_ghostty(text)
    } else {
        parse_warp_yaml(text).or_else(|_| parse_ghostty(text))
    }
}

pub fn parse_theme_file(path: &Path) -> Result<ImportedScheme, ParseError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| ParseError::new(format!("no se pudo leer {}: {error}", path.display())))?;
    if metadata.len() > MAX_THEME_BYTES {
        return Err(ParseError::new(format!(
            "{} supera el límite de 64 KiB",
            path.display()
        )));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| ParseError::new(format!("no se pudo leer {}: {error}", path.display())))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| ParseError::new(format!("{} no está en UTF-8", path.display())))?;
    parse_theme_text(text)
}

pub fn is_theme_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        None | Some("yaml" | "yml" | "conf" | "ghostty" | "txt") => true,
        Some(_) => false,
    }
}

fn looks_like_ghostty(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = strip_comment(line).trim();
        let Some((key, _)) = split_assignment(trimmed) else {
            return false;
        };
        key.eq_ignore_ascii_case("palette")
            || ((key.eq_ignore_ascii_case("background") || key.eq_ignore_ascii_case("foreground"))
                && trimmed.contains('=')
                && !trimmed.contains(':'))
    })
}

fn parse_ghostty(text: &str) -> Result<ImportedScheme, ParseError> {
    let mut background = None;
    let mut foreground = None;
    let mut accent = None;
    let mut cursor = None;
    let mut ansi = [None; 16];
    for line in text.lines() {
        let trimmed = strip_comment(line).trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = split_assignment(trimmed) else {
            continue;
        };
        match key.as_str() {
            "background" => background = parse_hex(&value),
            "foreground" => foreground = parse_hex(&value),
            "cursor-color" | "cursor" => cursor = parse_hex(&value),
            "accent" => accent = parse_hex(&value),
            "palette" => {
                let Some((index_text, color_text)) = value.split_once('=') else {
                    continue;
                };
                let Ok(index) = index_text.trim().parse::<usize>() else {
                    continue;
                };
                if index < 16 {
                    ansi[index] = parse_hex(color_text.trim());
                }
            }
            _ => {}
        }
    }
    finish_scheme(None, None, background, foreground, accent, cursor, ansi)
}

fn parse_warp_yaml(text: &str) -> Result<ImportedScheme, ParseError> {
    let mut background = None;
    let mut foreground = None;
    let mut accent = None;
    let mut cursor = None;
    let mut details = None;
    let mut name = None;
    let mut ansi = [None; 16];
    let mut section = WarpSection::Root;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let content = strip_yaml_comment(line).trim();
        if content.is_empty() {
            continue;
        }
        if indent == 0 {
            section = WarpSection::Root;
        }
        let Some((key, value)) = split_yaml_key_value(content) else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        if value.is_empty() {
            section = match key.as_str() {
                "terminal_colors" | "colors" => WarpSection::Terminal,
                "normal" => WarpSection::Normal,
                "bright" => WarpSection::Bright,
                _ => section,
            };
            continue;
        }
        match (section, key.as_str()) {
            (WarpSection::Root, "background") => background = parse_hex(&value),
            (WarpSection::Root, "foreground") => foreground = parse_hex(&value),
            (WarpSection::Root, "accent") => accent = parse_hex(&value),
            (WarpSection::Root, "cursor") => cursor = parse_hex(&value),
            (WarpSection::Root, "name") => name = Some(unquote(&value)),
            (WarpSection::Root, "details") => {
                let details_value = unquote(&value).to_ascii_lowercase();
                details = Some(!matches!(details_value.as_str(), "lighter" | "light"));
            }
            (WarpSection::Normal, key) => {
                if let Some(index) = ansi_name_index(key) {
                    ansi[index] = parse_hex(&value);
                }
            }
            (WarpSection::Bright, key) => {
                if let Some(index) = ansi_name_index(key) {
                    ansi[index + 8] = parse_hex(&value);
                }
            }
            (WarpSection::Terminal, key) => {
                if let Some(index) = ansi_name_index(key) {
                    ansi[index] = parse_hex(&value);
                }
            }
            _ => {}
        }
    }
    finish_scheme(name, details, background, foreground, accent, cursor, ansi)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WarpSection {
    Root,
    Terminal,
    Normal,
    Bright,
}

fn finish_scheme(
    name: Option<String>,
    dark: Option<bool>,
    background: Option<u32>,
    foreground: Option<u32>,
    accent: Option<u32>,
    cursor: Option<u32>,
    ansi: [Option<u32>; 16],
) -> Result<ImportedScheme, ParseError> {
    let background = background.ok_or_else(|| ParseError::new("falta background"))?;
    let foreground = foreground.ok_or_else(|| ParseError::new("falta foreground"))?;
    Ok(ImportedScheme {
        name: name.filter(|name| !name.is_empty()),
        dark,
        background,
        foreground,
        accent,
        cursor,
        ansi,
    })
}

pub fn parse_hex(value: &str) -> Option<u32> {
    let value = unquote(value);
    let value = value.strip_prefix('#').unwrap_or(value.as_str());
    match value.len() {
        3 => {
            let red = u32::from_str_radix(&value[0..1], 16).ok()?;
            let green = u32::from_str_radix(&value[1..2], 16).ok()?;
            let blue = u32::from_str_radix(&value[2..3], 16).ok()?;
            Some((red << 20) | (red << 16) | (green << 12) | (green << 8) | (blue << 4) | blue)
        }
        6 => u32::from_str_radix(value, 16).ok(),
        8 => u32::from_str_radix(&value[..6], 16).ok(),
        _ => None,
    }
}

fn ansi_name_index(name: &str) -> Option<usize> {
    Some(match name {
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "yellow" => 3,
        "blue" => 4,
        "magenta" | "purple" => 5,
        "cyan" => 6,
        "white" => 7,
        _ => return None,
    })
}

fn split_assignment(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || key.contains(':') {
        return None;
    }
    Some((key.to_ascii_lowercase(), value.trim().to_string()))
}

fn split_yaml_key_value(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), value.trim().to_string()))
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn strip_comment(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return "";
    }
    line
}

fn strip_yaml_comment(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return "";
    }
    if let Some(index) = line.find(" #") {
        return &line[..index];
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORD_YAML: &str = r###"
name: Nord
accent: "#81A1C1"
background: "#2E3440"
details: darker
foreground: "#D8DEE9"
terminal_colors:
  bright:
    black: "#4C566A"
    blue: "#81A1C1"
    cyan: "#8FBCBB"
    green: "#A3BE8C"
    magenta: "#B48EAD"
    red: "#BF616A"
    white: "#ECEFF4"
    yellow: "#EBCB8B"
  normal:
    black: "#3B4252"
    blue: "#81A1C1"
    cyan: "#88C0D0"
    green: "#A3BE8C"
    magenta: "#B48EAD"
    red: "#BF616A"
    white: "#E5E9F0"
    yellow: "#EBCB8B"
"###;

    const TOKYO_GHOSTTY: &str = r#"
palette = 0=#15161e
palette = 1=#f7768e
palette = 2=#9ece6a
palette = 3=#e0af68
palette = 4=#7aa2f7
palette = 5=#bb9af7
palette = 6=#7dcfff
palette = 7=#a9b1d6
palette = 8=#414868
palette = 9=#ff899d
palette = 10=#9fe044
palette = 11=#faba4a
palette = 12=#8db0ff
palette = 13=#c7a9ff
palette = 14=#a4daff
palette = 15=#c0caf5
background = #1a1b26
foreground = #c0caf5
cursor-color = #c0caf5
"#;

    #[test]
    fn parse_hex_accepts_hash_and_short_forms() {
        assert_eq!(parse_hex("#2E3440"), Some(0x2e3440));
        assert_eq!(parse_hex("2e3440"), Some(0x2e3440));
        assert_eq!(parse_hex("#abc"), Some(0xaabbcc));
        assert_eq!(parse_hex("#2e3440ff"), Some(0x2e3440));
        assert_eq!(parse_hex("nope"), None);
    }

    #[test]
    fn warp_yaml_reads_nord() {
        let scheme = parse_theme_text(NORD_YAML).unwrap();
        assert_eq!(scheme.name.as_deref(), Some("Nord"));
        assert_eq!(scheme.dark, Some(true));
        assert_eq!(scheme.background, 0x2e3440);
        assert_eq!(scheme.foreground, 0xd8dee9);
        assert_eq!(scheme.accent, Some(0x81a1c1));
        assert_eq!(scheme.ansi[0], Some(0x3b4252));
        assert_eq!(scheme.ansi[1], Some(0xbf616a));
        assert_eq!(scheme.ansi[8], Some(0x4c566a));
        assert_eq!(scheme.ansi[14], Some(0x8fbcbb));
    }

    #[test]
    fn ghostty_reads_tokyo_night() {
        let scheme = parse_theme_text(TOKYO_GHOSTTY).unwrap();
        assert_eq!(scheme.background, 0x1a1b26);
        assert_eq!(scheme.foreground, 0xc0caf5);
        assert_eq!(scheme.cursor, Some(0xc0caf5));
        assert_eq!(scheme.ansi[0], Some(0x15161e));
        assert_eq!(scheme.ansi[1], Some(0xf7768e));
        assert_eq!(scheme.ansi[15], Some(0xc0caf5));
    }

    #[test]
    fn missing_background_is_an_error() {
        assert!(parse_theme_text("foreground: \"#ffffff\"\n").is_err());
    }

    #[test]
    fn warp_light_details_are_not_dark() {
        let scheme =
            parse_theme_text("background: '#eceff4'\nforeground: '#2e3440'\ndetails: lighter\n")
                .unwrap();
        assert_eq!(scheme.dark, Some(false));
    }
}
