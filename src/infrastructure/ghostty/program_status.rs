//! Program Status Protocol (OSC 7501): a program reports idle, working,
//! blocked, done or error for itself and for child records beneath it.
//! Spec: https://www.superlogical.com/rex/docs/build/program-status
//!
//! The scanner reads the PTY stream before libghostty-vt, which drops the
//! sequence as unknown, so the probe reply is queued ahead of the DA1 reply a
//! program sends right after it. Nothing is stripped from the stream.

use std::time::Instant;

use crate::ports::terminal::{ProgramBlock, ProgramState, TerminalProgramStatus};

/// The only bytes this terminal ever writes back. The explicit lists tell a
/// later-revision program exactly which states and kinds are understood.
pub(super) const QUERY_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

const PREFIX: &[u8] = b"\x1b]7501;";
// The spec ties record lifetime to a new shell prompt and to a full reset.
const PROMPT_START: &[u8] = b"\x1b]133;A";
const FULL_RESET: &[u8] = b"\x1bc";
/// Whole sequence cap, OSC through ST.
const MAX_SEQUENCE_BYTES: usize = 4096;
const MAX_BODY_BYTES: usize = MAX_SEQUENCE_BYTES - PREFIX.len() - 1;
/// Bounds the `;key=value` options a prompt-start mark may carry.
const MAX_PROMPT_START_OPTIONS: usize = 512;
const MAX_KEY_BYTES: usize = 16;
const MAX_APP_BYTES: usize = 32;
const MAX_MSG_ENCODED: usize = 2732;
const MAX_MSG_DECODED: usize = 2048;
const MAX_TITLE_ENCODED: usize = 256;
const MAX_TITLE_DECODED: usize = 192;
const MAX_ID_BYTES: usize = 128;
const MAX_ID_DEPTH: usize = 8;
const MAX_SEGMENT_BYTES: usize = 32;
/// Spec ceiling; the least recently updated record goes past it.
const MAX_RECORDS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReportState {
    Set(ProgramState),
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Report {
    /// `None` addresses the root record.
    id: Option<String>,
    state: ReportState,
    kind: Option<ProgramBlock>,
    app: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Message {
    Query,
    Report(Report),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamEvent {
    Message(Message),
    PromptStart,
    FullReset,
}

fn parse_state(value: &str) -> Option<ReportState> {
    Some(ReportState::Set(match value {
        "idle" => ProgramState::Idle,
        "working" => ProgramState::Working,
        "blocked" => ProgramState::Blocked,
        "done" => ProgramState::Done,
        "error" => ProgramState::Error,
        "clear" => return Some(ReportState::Clear),
        _ => return None,
    }))
}

fn parse_kind(value: &str) -> Option<ProgramBlock> {
    match value {
        "permission" => Some(ProgramBlock::Permission),
        "question" => Some(ProgramBlock::Question),
        "auth" => Some(ProgramBlock::Auth),
        _ => None,
    }
}

fn is_segment_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-')
}

fn is_value_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-')
}

fn valid_id(id: &str) -> bool {
    id.len() <= MAX_ID_BYTES
        && id.split('/').count() <= MAX_ID_DEPTH
        && id.split('/').all(|segment| {
            !segment.is_empty()
                && segment.len() <= MAX_SEGMENT_BYTES
                && segment.bytes().all(is_segment_byte)
        })
}

/// Validates base64 UTF-8 text; invalid text voids the whole report.
fn valid_text(encoded: &str, max_encoded: usize, max_decoded: usize) -> bool {
    use base64::Engine as _;

    // The encoded cap bounds the work before anything is decoded.
    if encoded.len() > max_encoded {
        return false;
    }
    let unpadded = encoded.trim_end_matches('=');
    if encoded.len() - unpadded.len() > 2 {
        return false;
    }
    let Ok(bytes) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(unpadded) else {
        return false;
    };
    if bytes.len() > max_decoded {
        return false;
    }
    // `is_control` covers C0, DEL and C1, so newlines and tabs are refused too.
    std::str::from_utf8(&bytes).is_ok_and(|text| !text.chars().any(char::is_control))
}

/// The body between `OSC 7501;` and ST. `None` means ignore it whole: no
/// state, a bad id, or a limit broken anywhere, even by a later duplicate.
fn parse_body(body: &[u8]) -> Option<Message> {
    if body.len() > MAX_BODY_BYTES {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    if body.trim() == "?" {
        return Some(Message::Query);
    }
    let mut state = None;
    let mut id = None;
    let mut kind = None;
    let mut app = None;
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.len() > MAX_KEY_BYTES
            || (key == "app" && value.len() > MAX_APP_BYTES)
            || (key == "id" && !valid_id(value))
        {
            return None;
        }
        // A malformed pair is skipped; the rest of the report still applies.
        if key.is_empty()
            || !key.bytes().all(|byte| byte.is_ascii_lowercase())
            || !value.bytes().all(is_value_byte)
        {
            continue;
        }
        match key {
            "msg" if !valid_text(value, MAX_MSG_ENCODED, MAX_MSG_DECODED) => return None,
            "title" if !valid_text(value, MAX_TITLE_ENCODED, MAX_TITLE_DECODED) => return None,
            "state" => state = Some(value),
            "id" => id = Some(value),
            "kind" => kind = Some(value),
            "app" => app = Some(value),
            _ => {}
        }
    }
    let state = parse_state(state?)?;
    let blocked = state == ReportState::Set(ProgramState::Blocked);
    Some(Message::Report(Report {
        id: id.map(str::to_owned),
        kind: kind.filter(|_| blocked).and_then(parse_kind),
        app: app
            .filter(|app| !app.is_empty() && app.bytes().all(is_segment_byte))
            .map(str::to_owned),
        state,
    }))
}

/// Where an OSC frame ends: BEL or ST, an unconsumed non-ST ESC (as xterm
/// does), or an abort by CAN/SUB.
struct FrameEnd {
    index: usize,
    length: usize,
    aborted: bool,
}

fn find_terminator(data: &[u8], from: usize, limit: usize) -> Option<FrameEnd> {
    let end = limit.min(data.len());
    (from..end).find_map(|index| match data[index] {
        0x07 => Some(FrameEnd {
            index,
            length: 1,
            aborted: false,
        }),
        0x18 | 0x1a => Some(FrameEnd {
            index,
            length: 1,
            aborted: true,
        }),
        0x1b if index + 1 < data.len() => Some(FrameEnd {
            index,
            length: if data[index + 1] == b'\\' { 2 } else { 0 },
            aborted: false,
        }),
        _ => None,
    })
}

/// Stateful scanner for one PTY stream; frames may split across reads.
#[derive(Default)]
struct Scanner {
    pending: Vec<u8>,
}

impl Scanner {
    fn feed(&mut self, data: &[u8]) -> Vec<StreamEvent> {
        if self.pending.is_empty() && !data.contains(&0x1b) {
            return Vec::new();
        }
        let combined = if self.pending.is_empty() {
            std::borrow::Cow::Borrowed(data)
        } else {
            let mut combined = std::mem::take(&mut self.pending);
            combined.extend_from_slice(data);
            std::borrow::Cow::Owned(combined)
        };
        let data = combined.as_ref();
        let mut events = Vec::new();
        let mut cursor = 0;
        while let Some(offset) = data[cursor..].iter().position(|byte| *byte == 0x1b) {
            let escape = cursor + offset;
            let rest = &data[escape..];
            if rest.starts_with(FULL_RESET) {
                events.push(StreamEvent::FullReset);
                cursor = escape + FULL_RESET.len();
            } else if rest.starts_with(PROMPT_START) {
                let options = escape + PROMPT_START.len();
                // `133;AB` would be another mark; only BEL, ESC or `;` continue A.
                let continues = data
                    .get(options)
                    .is_none_or(|byte| matches!(byte, 0x07 | 0x1b | b';'));
                if !continues {
                    cursor = options;
                    continue;
                }
                match find_terminator(data, options, options + MAX_PROMPT_START_OPTIONS) {
                    Some(end) => {
                        if !end.aborted {
                            events.push(StreamEvent::PromptStart);
                        }
                        cursor = end.index + end.length;
                    }
                    None if data.len() - options < MAX_PROMPT_START_OPTIONS => {
                        self.pending = rest.to_vec();
                        break;
                    }
                    None => cursor = options,
                }
            } else if rest.starts_with(PREFIX) {
                let body = escape + PREFIX.len();
                match find_terminator(data, body, body + MAX_BODY_BYTES + 1) {
                    Some(end) => {
                        if !end.aborted
                            && let Some(message) = parse_body(&data[body..end.index])
                        {
                            events.push(StreamEvent::Message(message));
                        }
                        cursor = end.index + end.length;
                    }
                    // Keep a partial frame for the next read.
                    None if data.len() - body <= MAX_BODY_BYTES => {
                        self.pending = rest.to_vec();
                        break;
                    }
                    // Over the sequence cap: discard it and resume after the introducer.
                    None => cursor = body,
                }
            } else if [PREFIX, PROMPT_START, FULL_RESET]
                .iter()
                .any(|marker| marker.len() > rest.len() && marker.starts_with(rest))
            {
                self.pending = rest.to_vec();
                break;
            } else {
                cursor = escape + 1;
            }
        }
        events
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    id: Option<String>,
    state: ProgramState,
    kind: Option<ProgramBlock>,
    app: Option<String>,
}

fn is_at_or_beneath(id: &str, ancestor: &str) -> bool {
    id == ancestor
        || id
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// One terminal's records, least recently updated first.
#[derive(Default)]
struct Records(Vec<Record>);

impl Records {
    fn apply(&mut self, report: Report) -> bool {
        let state = match report.state {
            ReportState::Clear => {
                let Some(id) = report.id else {
                    return self.reset();
                };
                let before = self.0.len();
                self.0.retain(|record| {
                    !record
                        .id
                        .as_deref()
                        .is_some_and(|existing| is_at_or_beneath(existing, &id))
                });
                return self.0.len() != before;
            }
            ReportState::Set(state) => state,
        };
        // Each report replaces its record whole.
        let record = Record {
            id: report.id,
            state,
            kind: report.kind,
            app: report.app,
        };
        // Redraws resend identical reports; only a change is news.
        if self.0.last() == Some(&record) {
            return false;
        }
        let changed = self
            .0
            .iter()
            .find(|existing| existing.id == record.id)
            .is_none_or(|existing| *existing != record);
        self.0.retain(|existing| existing.id != record.id);
        if self.0.len() >= MAX_RECORDS {
            self.0.remove(0);
        }
        self.0.push(record);
        changed
    }

    /// The program ended: `done` and `error` outlive it, live states do not.
    fn end_program(&mut self) -> bool {
        let before = self.0.len();
        self.0
            .retain(|record| matches!(record.state, ProgramState::Done | ProgramState::Error));
        self.0.len() != before
    }

    fn reset(&mut self) -> bool {
        let changed = !self.0.is_empty();
        self.0.clear();
        changed
    }

    fn root(&self) -> Option<&Record> {
        self.0.iter().find(|record| record.id.is_none())
    }

    fn summary(&self) -> Option<(ProgramState, Option<ProgramBlock>, Option<String>)> {
        let root = self.root();
        let app = root
            .and_then(|root| root.app.clone())
            .or_else(|| self.0.iter().rev().find_map(|record| record.app.clone()));
        // A block anywhere needs the user, even beneath an idle program.
        let blocked = root
            .filter(|root| root.state == ProgramState::Blocked)
            .or_else(|| {
                self.0
                    .iter()
                    .rev()
                    .find(|record| record.state == ProgramState::Blocked)
            });
        if let Some(blocked) = blocked {
            return Some((ProgramState::Blocked, blocked.kind, app));
        }
        if self
            .0
            .iter()
            .any(|record| record.state == ProgramState::Working)
        {
            return Some((ProgramState::Working, None, app));
        }
        root.map(|root| (root.state, None, app))
    }
}

/// OSC 9;4 progress, as libghostty-vt reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProgressState {
    Remove,
    Set,
    Error,
    Indeterminate,
    Pause,
}

impl ProgressState {
    pub(super) fn from_raw(state: u8) -> Option<Self> {
        Some(match state {
            0 => Self::Remove,
            1 => Self::Set,
            2 => Self::Error,
            3 => Self::Indeterminate,
            4 => Self::Pause,
            _ => return None,
        })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScanOutcome {
    /// The program probed for support and is waiting for `QUERY_REPLY`.
    pub(super) query: bool,
    pub(super) changed: bool,
}

/// One terminal's program status, shared by its PTY worker and its handle.
#[derive(Default)]
pub(super) struct ProgramStatusTracker {
    scanner: Scanner,
    records: Records,
    process_group: Option<u32>,
    started_at: Option<Instant>,
    /// OSC 9;4 maps to the root only until the program speaks OSC 7501.
    spoke_program_status: bool,
    generation: u64,
}

impl ProgramStatusTracker {
    /// `foreground` names the process group owning the TTY; it is read only
    /// when a report arrives.
    pub(super) fn scan(
        &mut self,
        data: &[u8],
        foreground: impl Fn() -> Option<u32>,
    ) -> ScanOutcome {
        let mut outcome = ScanOutcome::default();
        for event in self.scanner.feed(data) {
            outcome.changed |= match event {
                StreamEvent::Message(Message::Query) => {
                    outcome.query = true;
                    false
                }
                StreamEvent::Message(Message::Report(report)) => {
                    let adopted = self.adopt(foreground());
                    self.spoke_program_status = true;
                    self.records.apply(report) | adopted
                }
                StreamEvent::PromptStart => self.records.end_program(),
                StreamEvent::FullReset => {
                    self.spoke_program_status = false;
                    self.records.reset()
                }
            };
        }
        self.settle(outcome.changed);
        outcome
    }

    /// Maps OSC 9;4 onto the root record for programs that report no OSC 7501.
    pub(super) fn apply_progress(
        &mut self,
        progress: ProgressState,
        foreground: impl Fn() -> Option<u32>,
    ) -> bool {
        if self.spoke_program_status {
            return false;
        }
        let adopted = self.adopt(foreground());
        let state = match progress {
            ProgressState::Remove => {
                let changed = self.records.reset() | adopted;
                self.settle(changed);
                return changed;
            }
            ProgressState::Set | ProgressState::Indeterminate => ProgramState::Working,
            ProgressState::Error => ProgramState::Error,
            ProgressState::Pause => ProgramState::Idle,
        };
        let changed = self.records.apply(Report {
            id: None,
            state: ReportState::Set(state),
            kind: None,
            app: None,
        }) | adopted;
        self.settle(changed);
        changed
    }

    pub(super) fn status(&self) -> Option<TerminalProgramStatus> {
        let (state, block, app) = self.records.summary()?;
        Some(TerminalProgramStatus {
            process_group: self.process_group,
            started_at: self.started_at?,
            app,
            state,
            block,
        })
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// A report from another process group is a new program: what the last
    /// one left behind is not its state.
    fn adopt(&mut self, process_group: Option<u32>) -> bool {
        if self.process_group == process_group {
            return false;
        }
        self.process_group = process_group;
        self.started_at = None;
        self.spoke_program_status = false;
        self.records.reset()
    }

    fn settle(&mut self, changed: bool) {
        if self.records.0.is_empty() {
            self.started_at = None;
        } else if self.started_at.is_none() {
            self.started_at = Some(Instant::now());
        }
        if changed {
            self.generation = self.generation.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(body: &str) -> Option<Report> {
        match parse_body(body.as_bytes())? {
            Message::Report(report) => Some(report),
            Message::Query => None,
        }
    }

    fn encode(text: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    #[test]
    fn parses_reports_queries_and_optional_keys() {
        assert_eq!(parse_body(b" ? "), Some(Message::Query));
        assert_eq!(
            report("state=idle:app=claude-code"),
            Some(Report {
                id: None,
                state: ReportState::Set(ProgramState::Idle),
                kind: None,
                app: Some("claude-code".into()),
            })
        );
        let blocked = report(&format!(
            " state = blocked : kind=permission : id=build/test : msg={} : extra=1",
            encode("Approve Write: src/main.rs?")
        ))
        .unwrap();
        assert_eq!(blocked.state, ReportState::Set(ProgramState::Blocked));
        assert_eq!(blocked.kind, Some(ProgramBlock::Permission));
        assert_eq!(blocked.id.as_deref(), Some("build/test"));
        // `kind` only means something on a blocked report.
        assert_eq!(report("state=working:kind=question").unwrap().kind, None);
        assert_eq!(report("state=blocked:kind=unknown").unwrap().kind, None);
        assert_eq!(report("state=clear").unwrap().state, ReportState::Clear);
        // The last duplicate wins.
        assert_eq!(
            report("state=working:state=done").unwrap().state,
            ReportState::Set(ProgramState::Done)
        );
    }

    #[test]
    fn malformed_pairs_are_skipped_but_broken_limits_void_the_report() {
        assert!(report("app=codex").is_none(), "state is required");
        assert!(report("state=sleeping").is_none());
        assert_eq!(
            report("state=idle:Bad=1:app=co dex:noequals").unwrap().app,
            None
        );
        assert!(report("state=idle:id=").is_none());
        assert!(report("state=idle:id=a//b").is_none());
        assert!(report(&format!("state=idle:id={}", "a/".repeat(8) + "a")).is_none());
        assert!(report(&format!("state=idle:id={}", "x".repeat(33))).is_none());
        assert!(report(&format!("state=idle:app={}", "x".repeat(33))).is_none());
        assert!(report(&format!("state=idle:{}=1", "k".repeat(17))).is_none());
        assert!(report("state=idle:msg=a-b").is_none(), "invalid base64");
        assert!(
            report("state=idle:msg=abcde").is_none(),
            "impossible length"
        );
        assert!(report(&format!("state=idle:msg={}", encode("two\nlines"))).is_none());
        assert!(report(&format!("state=idle:msg={}", encode("c1 \u{85}"))).is_none());
        assert!(report(&format!("state=idle:title={}", encode(&"t".repeat(193)))).is_none());
        // A later valid duplicate cannot launder an over-limit one.
        assert!(
            report(&format!(
                "state=idle:msg={}:msg={}",
                encode(&"m".repeat(2049)),
                encode("ok")
            ))
            .is_none()
        );
        assert!(parse_body(&[b's', 0xff, b'=']).is_none());
    }

    #[test]
    fn scanner_reads_split_frames_and_lifetime_marks() {
        let mut scanner = Scanner::default();
        assert!(scanner.feed(b"plain output").is_empty());
        assert!(scanner.feed(b"before\x1b]75").is_empty());
        assert!(scanner.feed(b"01;state=work").is_empty());
        let events = scanner.feed(b"ing\x07after\x1b]133;A\x1b\\\x1bc");
        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[0],
            StreamEvent::Message(Message::Report(Report {
                state: ReportState::Set(ProgramState::Working),
                ..
            }))
        ));
        assert_eq!(events[1], StreamEvent::PromptStart);
        assert_eq!(events[2], StreamEvent::FullReset);
        assert_eq!(
            scanner.feed(b"\x1b]7501;?\x1b\\\x1b[c"),
            [StreamEvent::Message(Message::Query)]
        );
        // `133;AB` is another mark, and CAN aborts a frame.
        assert!(scanner.feed(b"\x1b]133;AB\x07").is_empty());
        assert!(scanner.feed(b"\x1b]7501;state=idle\x18").is_empty());
        // An ESC that is not ST ends a frame and starts the next sequence.
        assert_eq!(
            scanner
                .feed(b"\x1b]7501;state=idle\x1b]7501;state=working\x07")
                .len(),
            2
        );
        // A trailing lone ESC may still become a marker.
        assert!(scanner.feed(b"x\x1b").is_empty());
        assert_eq!(scanner.feed(b"c"), [StreamEvent::FullReset]);
    }

    #[test]
    fn scanner_discards_oversized_frames_without_retaining_them() {
        let mut scanner = Scanner::default();
        let mut frame = b"\x1b]7501;state=idle:x=".to_vec();
        frame.extend(std::iter::repeat_n(b'a', MAX_SEQUENCE_BYTES));
        assert!(scanner.feed(&frame).is_empty());
        assert!(scanner.pending.is_empty());
        assert!(scanner.feed(b"\x07").is_empty());
        assert_eq!(
            scanner.feed(b"\x1b]7501;state=done\x1b\\").len(),
            1,
            "the stream recovers after an oversized frame"
        );
    }

    #[test]
    fn records_replace_whole_clear_subtrees_and_fold_children() {
        let mut records = Records::default();
        let apply = |records: &mut Records, body: &str| records.apply(report(body).unwrap());
        assert!(apply(&mut records, "state=idle:app=claude-code"));
        assert!(!apply(&mut records, "state=idle:app=claude-code"));
        assert!(apply(&mut records, "state=working:id=agent1"));
        assert!(apply(&mut records, "state=working:id=agent1/tool"));
        assert_eq!(
            records.summary(),
            Some((ProgramState::Working, None, Some("claude-code".into())))
        );
        assert!(apply(&mut records, "state=blocked:kind=question:id=agent2"));
        assert_eq!(
            records.summary().unwrap().1,
            Some(ProgramBlock::Question),
            "a blocked child needs the user even while the program is idle"
        );
        assert!(apply(&mut records, "state=clear:id=agent1"));
        assert_eq!(records.0.len(), 2);
        assert!(apply(&mut records, "state=done:id=agent2"));
        assert_eq!(records.summary().unwrap().0, ProgramState::Idle);
        assert!(apply(&mut records, "state=working"));
        assert!(records.end_program());
        assert_eq!(records.0.len(), 1, "done survives the program");
        assert!(
            records.summary().is_none(),
            "a finished child is not a program"
        );
        assert!(apply(&mut records, "state=clear"));
        assert!(records.0.is_empty());
    }

    #[test]
    fn records_evict_the_least_recently_updated() {
        let mut records = Records::default();
        for index in 0..=MAX_RECORDS {
            records.apply(report(&format!("state=working:id=r{index}")).unwrap());
        }
        assert_eq!(records.0.len(), MAX_RECORDS);
        assert_eq!(records.0[0].id.as_deref(), Some("r1"));
    }

    #[test]
    fn tracker_answers_probes_and_belongs_to_the_reporting_process_group() {
        let mut tracker = ProgramStatusTracker::default();
        let outcome = tracker.scan(b"\x1b]7501;?\x1b\\\x1b[c", || Some(10));
        assert!(outcome.query);
        assert!(!outcome.changed);
        assert!(tracker.status().is_none());

        let outcome = tracker.scan(b"\x1b]7501;state=working:app=codex\x07", || Some(10));
        assert!(outcome.changed);
        let status = tracker.status().unwrap();
        assert_eq!(status.process_group, Some(10));
        assert_eq!(status.state, ProgramState::Working);
        assert_eq!(status.app.as_deref(), Some("codex"));
        let generation = tracker.generation();
        let started_at = status.started_at;

        assert!(
            !tracker
                .scan(b"\x1b]7501;state=working:app=codex\x07", || Some(10))
                .changed
        );
        assert_eq!(tracker.generation(), generation);
        assert_eq!(tracker.status().unwrap().started_at, started_at);

        // Another group reporting is another program; the old records go.
        tracker.scan(b"\x1b]7501;state=idle\x07", || Some(11));
        let status = tracker.status().unwrap();
        assert_eq!(status.process_group, Some(11));
        assert_eq!(status.app, None);
        assert_eq!(status.state, ProgramState::Idle);
        assert!(status.started_at >= started_at);

        assert!(tracker.scan(b"\x1b]133;A\x07", || Some(1)).changed);
        assert!(tracker.status().is_none());
    }

    #[test]
    fn progress_maps_to_the_root_until_the_program_speaks_osc_7501() {
        let mut tracker = ProgramStatusTracker::default();
        assert!(tracker.apply_progress(ProgressState::Indeterminate, || Some(5)));
        assert_eq!(tracker.status().unwrap().state, ProgramState::Working);
        assert!(tracker.apply_progress(ProgressState::Remove, || Some(5)));
        assert!(tracker.status().is_none());

        tracker.scan(b"\x1b]7501;state=idle:app=claude-code\x07", || Some(5));
        assert!(!tracker.apply_progress(ProgressState::Set, || Some(5)));
        assert_eq!(tracker.status().unwrap().state, ProgramState::Idle);

        tracker.scan(b"\x1bc", || Some(5));
        assert!(tracker.apply_progress(ProgressState::Error, || Some(5)));
        assert_eq!(tracker.status().unwrap().state, ProgramState::Error);
        assert_eq!(ProgressState::from_raw(9), None);
    }
}
