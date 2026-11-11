//! Agent transcripts on disk: how full each session's context is, and the
//! tokens spent per day. Claude Code writes
//! `~/.claude/projects/<folder>/<session>.jsonl`; Codex writes
//! `~/.codex/sessions/<yyyy>/<mm>/<dd>/rollout-…-<session>.jsonl`. Both only
//! append, so a file is read once and afterwards from where it ended.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use uuid::Uuid;

use crate::domain::activity::{
    ContextFill, TOKEN_DAYS, TokenHistory, TokenLedger, claude_context_window, claude_turn,
    codex_tokens, local_day,
};
use crate::domain::agents::AgentKind;

/// The end of a transcript holds its latest turn; tool output can make a
/// single line long, so the first look is generous and the second more so.
const TAIL_BYTES: [u64; 2] = [2 * 1024 * 1024, 16 * 1024 * 1024];
/// A session that has not written its transcript yet is looked for again after this.
const MISSING_RETRY: Duration = Duration::from_secs(10);
/// How deep the transcript folders nest: subagents sit below their session.
const MAX_SCAN_DEPTH: usize = 4;
/// Codex day folders searched for a session before giving up.
const MAX_CODEX_DAYS: usize = 400;

/// An agent a pane runs, as its hooks named its session.
#[derive(Debug, Clone)]
pub struct AgentSession {
    pub pane_id: Uuid,
    pub kind: AgentKind,
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn of(kind: AgentKind) -> Option<Self> {
        match kind {
            AgentKind::Claude => Some(Self::Claude),
            AgentKind::Codex => Some(Self::Codex),
            _ => None,
        }
    }
}

/// Reads each session's latest turn for how full its context is.
#[derive(Default)]
pub struct ContextReader {
    paths: HashMap<String, PathBuf>,
    missing: HashMap<String, Instant>,
    /// The fill last read from each transcript, and the length it was read at.
    fills: HashMap<PathBuf, (u64, Option<ContextFill>)>,
}

impl ContextReader {
    pub fn read(&mut self, sessions: &[AgentSession], home: &Path) -> HashMap<Uuid, ContextFill> {
        let mut fills = HashMap::new();
        let mut live = HashSet::new();
        for session in sessions {
            let Some(provider) = Provider::of(session.kind) else {
                continue;
            };
            if !is_session_id(&session.session_id) {
                continue;
            }
            let Some(path) = self.locate(provider, &session.session_id, home) else {
                continue;
            };
            let Ok(length) = fs::metadata(&path).map(|metadata| metadata.len()) else {
                self.paths.remove(&session.session_id);
                continue;
            };
            let fill = match self.fills.get(&path) {
                Some((read_at, fill)) if *read_at == length => *fill,
                _ => {
                    let fill = latest_context(&path, provider, length);
                    self.fills.insert(path.clone(), (length, fill));
                    fill
                }
            };
            if let Some(fill) = fill {
                fills.insert(session.pane_id, fill);
            }
            live.insert(path);
        }
        self.fills.retain(|path, _| live.contains(path));
        let ids: HashSet<&str> = sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        self.paths.retain(|id, _| ids.contains(id.as_str()));
        self.missing.retain(|id, _| ids.contains(id.as_str()));
        fills
    }

    fn locate(&mut self, provider: Provider, session_id: &str, home: &Path) -> Option<PathBuf> {
        if let Some(path) = self.paths.get(session_id)
            && path.is_file()
        {
            return Some(path.clone());
        }
        if self
            .missing
            .get(session_id)
            .is_some_and(|since| since.elapsed() < MISSING_RETRY)
        {
            return None;
        }
        let found = match provider {
            Provider::Claude => claude_transcript(home, session_id),
            Provider::Codex => codex_rollout(home, session_id),
        };
        match &found {
            Some(path) => {
                self.missing.remove(session_id);
                self.paths.insert(session_id.to_owned(), path.clone());
            }
            None => {
                self.missing.insert(session_id.to_owned(), Instant::now());
            }
        }
        found
    }
}

/// Counts the tokens agents spent over the last week, reading only what
/// transcripts gained since the last count.
#[derive(Default)]
pub struct TokenCounter {
    ledger: TokenLedger,
    /// How far each transcript has been read.
    cursors: HashMap<PathBuf, u64>,
}

impl TokenCounter {
    pub fn refresh(&mut self, home: &Path, now: i64, utc_offset: i32) -> TokenHistory {
        // A day of slack covers the time zone and a file touched at midnight.
        let since = now - (TOKEN_DAYS as i64 + 1) * 86_400;
        let mut visited = HashSet::new();
        for (provider, root) in [
            (Provider::Claude, claude_projects(home)),
            (Provider::Codex, codex_sessions(home)),
        ] {
            for path in transcripts_since(&root, since) {
                self.read(&path, provider, utc_offset);
                visited.insert(path);
            }
        }
        // A file left out of the window is read from the start if it grows
        // again; responses already counted are still recognized.
        self.cursors.retain(|path, _| visited.contains(path));
        let today = local_day(now, utc_offset);
        self.ledger.prune(today);
        self.ledger.history(today)
    }

    fn read(&mut self, path: &Path, provider: Provider, utc_offset: i32) {
        let Ok(length) = fs::metadata(path).map(|metadata| metadata.len()) else {
            return;
        };
        let mut start = self.cursors.get(path).copied().unwrap_or(0);
        if length < start {
            // Rewritten rather than appended: read it again.
            start = 0;
        }
        if length == start {
            return;
        }
        let Ok(mut file) = File::open(path) else {
            return;
        };
        if file.seek(SeekFrom::Start(start)).is_err() {
            return;
        }
        let session = path.to_string_lossy().into_owned();
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut line = Vec::new();
        let mut consumed = start;
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                // A line still being written is read next time, whole.
                Ok(_) if line.last() != Some(&b'\n') => break,
                Ok(read) => consumed += read as u64,
            }
            let Ok(text) = std::str::from_utf8(&line) else {
                continue;
            };
            match provider {
                Provider::Claude => {
                    if let Some(turn) = claude_turn(text) {
                        self.ledger.add_claude(&turn, utc_offset);
                    }
                }
                Provider::Codex => {
                    if let Some(tokens) = codex_tokens(text) {
                        self.ledger.add_codex(&session, &tokens, utc_offset);
                    }
                }
            }
        }
        self.cursors.insert(path.to_path_buf(), consumed);
    }
}

/// The context the session held at its latest turn.
fn latest_context(path: &Path, provider: Provider, length: u64) -> Option<ContextFill> {
    for tail in TAIL_BYTES {
        let start = length.saturating_sub(tail);
        let text = read_from(path, start)?;
        // A tail that starts mid-line drops that first, partial line.
        let lines = text.lines().skip(usize::from(start > 0));
        let lines: Vec<&str> = lines.collect();
        let fill = lines.iter().rev().find_map(|line| match provider {
            Provider::Claude => claude_turn(line)
                .filter(|turn| !turn.sidechain && turn.context > 0)
                .map(|turn| ContextFill {
                    used: turn.context,
                    window: claude_context_window(
                        turn.model.as_deref().unwrap_or_default(),
                        turn.context,
                    ),
                }),
            Provider::Codex => codex_tokens(line).and_then(|tokens| {
                Some(ContextFill {
                    used: tokens.context,
                    window: tokens.window?,
                })
                .filter(|fill| fill.used > 0)
            }),
        });
        if fill.is_some() || start == 0 {
            return fill;
        }
    }
    None
}

fn read_from(path: &Path, start: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Session ids name files, so only plain ids are looked up.
fn is_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn claude_projects(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"))
        .join("projects")
}

fn codex_sessions(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("sessions")
}

/// Claude Code keeps a session in the folder of the directory it started in.
fn claude_transcript(home: &Path, session_id: &str) -> Option<PathBuf> {
    let name = format!("{session_id}.jsonl");
    fs::read_dir(claude_projects(home))
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&name))
        .find(|path| path.is_file())
}

/// Codex files a session under the day it started, newest days first.
fn codex_rollout(home: &Path, session_id: &str) -> Option<PathBuf> {
    let suffix = format!("-{session_id}.jsonl");
    let descending = |path: &Path| -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        entries.sort_unstable_by(|left, right| right.cmp(left));
        entries
    };
    let days = descending(&codex_sessions(home))
        .into_iter()
        .flat_map(|year| descending(&year))
        .flat_map(|month| descending(&month))
        .take(MAX_CODEX_DAYS);
    for day in days {
        let found = fs::read_dir(&day)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&suffix))
            });
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Transcripts under `root` changed since `since` (Unix seconds).
fn transcripts_since(root: &Path, since: i64) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![(root.to_path_buf(), 0)];
    while let Some((folder, depth)) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                if depth < MAX_SCAN_DEPTH {
                    folders.push((path, depth + 1));
                }
                continue;
            }
            if !kind.is_file()
                || path
                    .extension()
                    .is_none_or(|extension| extension != "jsonl")
            {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|age| age.as_secs() as i64);
            if modified.is_some_and(|modified| modified >= since) {
                found.push(path);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_home() -> PathBuf {
        let home = std::env::temp_dir().join(format!("vibra-transcripts-{}", Uuid::new_v4()));
        fs::create_dir_all(&home).unwrap();
        home
    }

    fn claude_line(id: &str, at: &str, context: u64, sidechain: bool) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","requestId":"r-{id}","isSidechain":{sidechain},"message":{{"id":"{id}","model":"claude-haiku-4-5","usage":{{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":{context},"output_tokens":5}}}}}}"#
        )
    }

    #[test]
    fn the_latest_lead_turn_is_the_sessions_context() {
        let home = temp_home();
        let folder = home.join(".claude/projects/-Users-me-api");
        fs::create_dir_all(&folder).unwrap();
        let session = "0c85621c-61cd-450f-aa30-ffdc2cb48ea4";
        let mut file = File::create(folder.join(format!("{session}.jsonl"))).unwrap();
        writeln!(
            file,
            "{}",
            claude_line("a", "2026-11-11T10:00:00Z", 50_000, false)
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            claude_line("b", "2026-11-11T10:01:00Z", 90_000, false)
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            claude_line("c", "2026-11-11T10:02:00Z", 1, true)
        )
        .unwrap();
        drop(file);

        let mut reader = ContextReader::default();
        let pane = Uuid::new_v4();
        let sessions = [AgentSession {
            pane_id: pane,
            kind: AgentKind::Claude,
            session_id: session.into(),
        }];
        let fills = reader.read(&sessions, &home);
        assert_eq!(
            fills[&pane],
            ContextFill {
                used: 90_010,
                window: 200_000
            },
            "the subagent's turn is not the lead's context"
        );

        let unsafe_id = [AgentSession {
            pane_id: pane,
            kind: AgentKind::Claude,
            session_id: "../../etc".into(),
        }];
        assert!(reader.read(&unsafe_id, &home).is_empty());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn codex_sessions_are_found_under_their_day() {
        let home = temp_home();
        let day = home.join(".codex/sessions/2026/11/11");
        fs::create_dir_all(&day).unwrap();
        let session = "01a1cb08-0a02-7691-b5c1-555e52a39d1b";
        let line = r#"{"timestamp":"2026-11-11T14:34:30.749Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":5},"last_token_usage":{"total_tokens":105},"model_context_window":475000}}}"#;
        fs::write(
            day.join(format!("rollout-2026-11-11T11-34-24-{session}.jsonl")),
            format!("{line}\n"),
        )
        .unwrap();
        let mut reader = ContextReader::default();
        let pane = Uuid::new_v4();
        let fills = reader.read(
            &[AgentSession {
                pane_id: pane,
                kind: AgentKind::Codex,
                session_id: session.into(),
            }],
            &home,
        );
        assert_eq!(
            fills[&pane],
            ContextFill {
                used: 105,
                window: 475_000
            }
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn tokens_are_counted_once_as_transcripts_grow() {
        let home = temp_home();
        let folder = home.join(".claude/projects/-Users-me-api");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("s.jsonl");
        // The file was just written, so the lines are dated by the real clock.
        let now = crate::domain::usage::now_timestamp();
        let at = chrono::DateTime::from_timestamp(now - 60, 0)
            .unwrap()
            .to_rfc3339();
        let mut file = File::create(&path).unwrap();
        writeln!(file, "{}", claude_line("a", &at, 0, false)).unwrap();
        // The same response, written again for its next content block.
        writeln!(file, "{}", claude_line("a", &at, 0, false)).unwrap();
        file.flush().unwrap();

        let mut counter = TokenCounter::default();
        let counted = counter.refresh(&home, now, 0).total();
        assert_eq!(counted, 15, "one response: ten in, five out");

        // A half-written line waits until it is whole.
        write!(file, "{}", claude_line("b", &at, 0, false)).unwrap();
        file.flush().unwrap();
        assert_eq!(counter.refresh(&home, now, 0).total(), counted);
        writeln!(file).unwrap();
        file.flush().unwrap();
        assert_eq!(counter.refresh(&home, now, 0).total(), 30);
        fs::remove_dir_all(home).unwrap();
    }
}
