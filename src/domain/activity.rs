//! Activity: what runs for the user's spaces, read-only. The pieces here are
//! pure: the process tree, the listening ports `lsof` reports, the token
//! tallies and context fill read from agent transcripts. The macOS adapter
//! samples the system and feeds them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use serde_json::Value;
use uuid::Uuid;

use crate::domain::agents::AgentKind;
use crate::domain::usage::timestamp;

/// Days the token history covers, today included.
pub const TOKEN_DAYS: usize = 7;
const SECONDS_PER_DAY: i64 = 86_400;
/// Claude models before the 1M context generation.
const STANDARD_CONTEXT: u64 = 200_000;
const LARGE_CONTEXT: u64 = 1_000_000;

/// One of the user's processes, as sampled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent: u32,
    /// Short executable name (`node`, `python3.12`).
    pub name: String,
    /// Physical footprint in bytes: Activity Monitor's Memory column.
    pub memory: u64,
}

/// The user's processes and who started whom.
#[derive(Debug, Default)]
pub struct ProcessTable {
    processes: HashMap<u32, ProcessInfo>,
    children: HashMap<u32, Vec<u32>>,
}

impl ProcessTable {
    pub fn new(processes: impl IntoIterator<Item = ProcessInfo>) -> Self {
        let processes: HashMap<u32, ProcessInfo> = processes
            .into_iter()
            .map(|process| (process.pid, process))
            .collect();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for process in processes.values() {
            if process.parent != process.pid {
                children
                    .entry(process.parent)
                    .or_default()
                    .push(process.pid);
            }
        }
        for siblings in children.values_mut() {
            siblings.sort_unstable();
        }
        Self {
            processes,
            children,
        }
    }

    pub fn get(&self, pid: u32) -> Option<&ProcessInfo> {
        self.processes.get(&pid)
    }

    pub fn processes(&self) -> impl Iterator<Item = &ProcessInfo> {
        self.processes.values()
    }

    /// `root` and every process below it, parents before their children.
    /// A root that is not in the table has no tree.
    pub fn tree(&self, root: u32) -> Vec<u32> {
        if !self.processes.contains_key(&root) {
            return Vec::new();
        }
        let mut seen = HashSet::from([root]);
        let mut tree = vec![root];
        let mut index = 0;
        while let Some(&pid) = tree.get(index) {
            for &child in self.children.get(&pid).into_iter().flatten() {
                if seen.insert(child) {
                    tree.push(child);
                }
            }
            index += 1;
        }
        tree
    }

    /// The processes above `pid`, nearest first. Stops at a loop.
    pub fn ancestors(&self, pid: u32) -> Vec<u32> {
        let mut seen = HashSet::from([pid]);
        let mut ancestors = Vec::new();
        let mut current = pid;
        while let Some(parent) = self.processes.get(&current).map(|process| process.parent) {
            if !seen.insert(parent) || !self.processes.contains_key(&parent) {
                break;
            }
            ancestors.push(parent);
            current = parent;
        }
        ancestors
    }

    pub fn memory<'a>(&self, pids: impl IntoIterator<Item = &'a u32>) -> u64 {
        pids.into_iter()
            .filter_map(|pid| self.processes.get(pid))
            .map(|process| process.memory)
            .sum()
    }
}

/// A process accepting connections, and on which ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub pid: u32,
    pub name: String,
    pub ports: Vec<u16>,
}

/// The TCP ports each process listens on, from
/// `lsof -nP -iTCP -sTCP:LISTEN -Fpn`: a `p<pid>` line opens a process and
/// each `n<address>:<port>` line names one of its sockets.
pub fn parse_listening_ports(output: &str) -> BTreeMap<u32, Vec<u16>> {
    let mut ports: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
    let mut pid = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.trim().parse::<u32>().ok();
        } else if let (Some(pid), Some(address)) = (pid, line.strip_prefix('n')) {
            let port = address
                .rsplit_once(':')
                .and_then(|(_, port)| port.trim().parse::<u16>().ok());
            if let Some(port) = port {
                let list = ports.entry(pid).or_default();
                if !list.contains(&port) {
                    list.push(port);
                }
            }
        }
    }
    for list in ports.values_mut() {
        list.sort_unstable();
    }
    ports
}

/// The servers among `pids`, in tree order. A port shared by a parent and the
/// child it handed the socket to belongs to the parent.
pub fn servers_in(
    table: &ProcessTable,
    pids: &[u32],
    listening: &BTreeMap<u32, Vec<u16>>,
) -> Vec<Server> {
    let mut claimed = HashSet::new();
    pids.iter()
        .filter_map(|pid| {
            let ports: Vec<u16> = listening
                .get(pid)?
                .iter()
                .copied()
                .filter(|port| claimed.insert(*port))
                .collect();
            (!ports.is_empty()).then(|| Server {
                pid: *pid,
                name: table
                    .get(*pid)
                    .map(|process| process.name.clone())
                    .unwrap_or_default(),
                ports,
            })
        })
        .collect()
}

/// What a terminal and everything it started use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeUsage {
    pub memory: u64,
    pub servers: Vec<Server>,
}

/// Servers and agents running outside Vibra, grouped by the folder they run in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideGroup {
    pub root: PathBuf,
    pub memory: u64,
    pub servers: Vec<Server>,
    pub agents: Vec<AgentKind>,
}

/// One reading of the machine for the Activity view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessSample {
    /// Each pane's terminal and what it started, by pane.
    pub panes: HashMap<Uuid, TreeUsage>,
    pub outside: Vec<OutsideGroup>,
}

/// How full an agent's context window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextFill {
    pub used: u64,
    pub window: u64,
}

impl ContextFill {
    pub fn percent(self) -> f64 {
        if self.window == 0 {
            return 0.0;
        }
        (self.used as f64 / self.window as f64 * 100.0).clamp(0.0, 100.0)
    }
}

/// The context window of a Claude model. Current models hold 1M tokens;
/// older ones 200K, unless the name asks for 1M or the session already
/// holds more than 200K.
pub fn claude_context_window(model: &str, used: u64) -> u64 {
    const LARGE_MODELS: [&str; 9] = [
        "fable",
        "mythos",
        "opus-5",
        "sonnet-5",
        "haiku-5",
        "opus-4-6",
        "opus-4-7",
        "opus-4-8",
        "sonnet-4-6",
    ];
    let model = model.to_ascii_lowercase();
    let large = model.contains("[1m]") || LARGE_MODELS.iter().any(|name| model.contains(name));
    if large || used > STANDARD_CONTEXT {
        LARGE_CONTEXT
    } else {
        STANDARD_CONTEXT
    }
}

/// One response recorded in a Claude Code transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeTurn {
    /// Message and request: the same response is written once per content
    /// block, and resumed sessions copy earlier ones.
    pub key: Option<String>,
    pub at: Option<i64>,
    /// Input, cache writes, and output; cache reads are not new work.
    pub fresh: u64,
    /// What the model read: the context the session holds.
    pub context: u64,
    pub model: Option<String>,
    /// A subagent's turn, not the lead's context.
    pub sidechain: bool,
}

/// The response a transcript line records, if it records one.
pub fn claude_turn(line: &str) -> Option<ClaudeTurn> {
    if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let model = message
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned);
    // Errors Claude Code writes itself carry an empty usage.
    if model.as_deref() == Some("<synthetic>") {
        return None;
    }
    let count = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    let input = count("input_tokens");
    let written = count("cache_creation_input_tokens");
    let read = count("cache_read_input_tokens");
    let output = count("output_tokens");
    let key = message.get("id").and_then(Value::as_str).map(|id| {
        match value.get("requestId").and_then(Value::as_str) {
            Some(request) => format!("{id}:{request}"),
            None => id.to_owned(),
        }
    });
    Some(ClaudeTurn {
        key,
        at: value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(timestamp),
        fresh: input + written + output,
        context: input + written + read,
        model,
        sidechain: value
            .get("isSidechain")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// A Codex token report: the session's running totals and its last turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexTokens {
    pub at: Option<i64>,
    /// Uncached input and output so far in the session.
    pub total_fresh: u64,
    /// What the last turn held: the context the session carries.
    pub context: u64,
    pub window: Option<u64>,
}

/// The token report a Codex rollout line carries, if it carries one.
pub fn codex_tokens(line: &str) -> Option<CodexTokens> {
    if !line.contains("\"token_count\"") {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let info = payload.get("info").filter(|info| !info.is_null())?;
    let count = |usage: Option<&Value>, name: &str| {
        usage
            .and_then(|usage| usage.get(name))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let total = info.get("total_token_usage");
    let last = info.get("last_token_usage");
    let context = match count(last, "total_tokens") {
        0 => count(last, "input_tokens") + count(last, "output_tokens"),
        tokens => tokens,
    };
    Some(CodexTokens {
        at: value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(timestamp),
        total_fresh: count(total, "input_tokens")
            .saturating_sub(count(total, "cached_input_tokens"))
            + count(total, "output_tokens"),
        context,
        window: info
            .get("model_context_window")
            .and_then(Value::as_u64)
            .filter(|window| *window > 0),
    })
}

/// The local calendar day of a Unix time, counted from the epoch.
pub fn local_day(at: i64, utc_offset: i32) -> i64 {
    (at + i64::from(utc_offset)).div_euclid(SECONDS_PER_DAY)
}

/// Tokens per day over the last week, today last.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenHistory {
    pub days: [u64; TOKEN_DAYS],
}

impl TokenHistory {
    pub fn today(&self) -> u64 {
        self.days[TOKEN_DAYS - 1]
    }

    pub fn total(&self) -> u64 {
        self.days.iter().sum()
    }
}

/// New tokens agents spent per local day: input, cache writes, and output.
/// Cache reads repeat the context on every turn, so they are left out.
#[derive(Debug, Default)]
pub struct TokenLedger {
    days: BTreeMap<i64, u64>,
    /// Claude responses already counted, with the day each fell on.
    claude_seen: HashMap<String, i64>,
    /// Each Codex session's running total when last read.
    codex_totals: HashMap<String, u64>,
}

impl TokenLedger {
    pub fn add_claude(&mut self, turn: &ClaudeTurn, utc_offset: i32) {
        let Some(at) = turn.at else {
            return;
        };
        let day = local_day(at, utc_offset);
        if let Some(key) = &turn.key
            && self.claude_seen.insert(key.clone(), day).is_some()
        {
            return;
        }
        *self.days.entry(day).or_default() += turn.fresh;
    }

    /// Codex reports running totals; each report adds what grew since the
    /// last one of its session. A total that shrinks starts a new run.
    pub fn add_codex(&mut self, session: &str, tokens: &CodexTokens, utc_offset: i32) {
        let previous = self
            .codex_totals
            .insert(session.to_owned(), tokens.total_fresh);
        let grown = match previous {
            Some(previous) if tokens.total_fresh >= previous => tokens.total_fresh - previous,
            _ => tokens.total_fresh,
        };
        if let (Some(at), true) = (tokens.at, grown > 0) {
            *self.days.entry(local_day(at, utc_offset)).or_default() += grown;
        }
    }

    /// Forgets days before the week that ends on `today`.
    pub fn prune(&mut self, today: i64) {
        let first = today - (TOKEN_DAYS as i64 - 1);
        self.days.retain(|day, _| *day >= first);
        self.claude_seen.retain(|_, day| *day >= first);
    }

    pub fn history(&self, today: i64) -> TokenHistory {
        let mut history = TokenHistory::default();
        for (index, slot) in history.days.iter_mut().enumerate() {
            let day = today - (TOKEN_DAYS - 1 - index) as i64;
            *slot = self.days.get(&day).copied().unwrap_or(0);
        }
        history
    }
}

/// Memory as Activity Monitor writes it: `412 MB`, `1.2 GB`.
pub fn memory_label(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.0} MB", bytes / MB)
    } else {
        format!("{:.0} KB", (bytes / 1024.0).max(1.0))
    }
}

/// A token count at a glance: `950`, `12K`, `1.2M`.
pub fn token_label(tokens: u64) -> String {
    let tokens = tokens as f64;
    let short = |value: f64, unit: &str| {
        if value < 10.0 {
            format!("{:.1}{unit}", (value * 10.0).floor() / 10.0).replace(".0", "")
        } else {
            format!("{:.0}{unit}", value.floor())
        }
    };
    if tokens >= 1e9 {
        short(tokens / 1e9, "B")
    } else if tokens >= 1e6 {
        short(tokens / 1e6, "M")
    } else if tokens >= 1e3 {
        short(tokens / 1e3, "K")
    } else {
        format!("{tokens:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, parent: u32, name: &str, memory: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent,
            name: name.to_owned(),
            memory,
        }
    }

    #[test]
    fn a_tree_holds_a_terminal_and_everything_it_started() {
        let table = ProcessTable::new([
            process(1, 0, "launchd", 10),
            process(10, 1, "zsh", 5),
            process(11, 10, "node", 100),
            process(12, 11, "esbuild", 20),
            process(20, 1, "zsh", 5),
        ]);
        assert_eq!(table.tree(10), vec![10, 11, 12]);
        assert_eq!(table.memory(&table.tree(10)), 125);
        assert_eq!(table.ancestors(12), vec![11, 10, 1]);
        assert!(table.tree(99).is_empty(), "a gone terminal has no tree");
    }

    #[test]
    fn a_parent_loop_does_not_hang_the_walk() {
        let table = ProcessTable::new([process(2, 3, "a", 1), process(3, 2, "b", 1)]);
        assert_eq!(table.tree(2), vec![2, 3]);
        assert_eq!(table.ancestors(2), vec![3]);
    }

    #[test]
    fn lsof_fields_become_ports_per_process() {
        let output = "p3747\nf14\nn*:59809\nf17\nn[::1]:59809\np3943\nf9\nn127.0.0.1:7000\nf11\nn*:5000\np15\nnbroken\n";
        let ports = parse_listening_ports(output);
        assert_eq!(ports[&3747], vec![59809]);
        assert_eq!(ports[&3943], vec![5000, 7000]);
        assert!(!ports.contains_key(&15));
    }

    #[test]
    fn a_port_handed_to_a_child_belongs_to_the_parent() {
        let table = ProcessTable::new([
            process(10, 1, "zsh", 1),
            process(11, 10, "node", 1),
            process(12, 11, "next-server", 1),
        ]);
        let listening = BTreeMap::from([(11, vec![3000]), (12, vec![3000, 3001])]);
        let servers = servers_in(&table, &table.tree(10), &listening);
        assert_eq!(
            servers,
            vec![
                Server {
                    pid: 11,
                    name: "node".into(),
                    ports: vec![3000]
                },
                Server {
                    pid: 12,
                    name: "next-server".into(),
                    ports: vec![3001]
                },
            ]
        );
    }

    #[test]
    fn claude_turns_count_new_work_and_read_the_context() {
        let line = r#"{"type":"assistant","timestamp":"2026-11-11T17:36:37.702Z","requestId":"req_1","isSidechain":false,"message":{"id":"msg_1","model":"claude-opus-5-5","usage":{"input_tokens":2,"cache_creation_input_tokens":2899,"cache_read_input_tokens":119106,"output_tokens":181}}}"#;
        let turn = claude_turn(line).unwrap();
        assert_eq!(turn.key.as_deref(), Some("msg_1:req_1"));
        assert_eq!(turn.fresh, 2 + 2899 + 181);
        assert_eq!(turn.context, 2 + 2899 + 119_106);
        assert_eq!(turn.model.as_deref(), Some("claude-opus-5-5"));
        assert!(!turn.sidechain);
        assert!(
            claude_turn(r#"{"type":"user","message":{"content":"usage assistant"}}"#).is_none()
        );
        assert!(
            claude_turn(r#"{"type":"assistant","message":{"model":"<synthetic>","usage":{}}}"#)
                .is_none()
        );
    }

    #[test]
    fn codex_reports_running_totals_and_its_window() {
        let line = r#"{"timestamp":"2026-11-11T14:34:30.749Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":18705,"cached_input_tokens":7936,"output_tokens":15,"total_tokens":18720},"last_token_usage":{"input_tokens":18705,"cached_input_tokens":7936,"output_tokens":15,"total_tokens":18720},"model_context_window":475000}}}"#;
        let tokens = codex_tokens(line).unwrap();
        assert_eq!(tokens.total_fresh, 18_705 - 7_936 + 15);
        assert_eq!(tokens.context, 18_720);
        assert_eq!(tokens.window, Some(475_000));
        assert!(
            codex_tokens(r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#)
                .is_none()
        );
    }

    #[test]
    fn current_claude_models_hold_a_million_tokens() {
        assert_eq!(claude_context_window("claude-opus-5-5", 10), LARGE_CONTEXT);
        assert_eq!(
            claude_context_window("claude-sonnet-4-6", 10),
            LARGE_CONTEXT
        );
        assert_eq!(
            claude_context_window("claude-haiku-4-5", 10),
            STANDARD_CONTEXT
        );
        assert_eq!(
            claude_context_window("claude-haiku-4-5", 250_000),
            LARGE_CONTEXT,
            "a session past 200K must have the larger window"
        );
        let fill = ContextFill {
            used: 150_000,
            window: 200_000,
        };
        assert_eq!(fill.percent(), 75.0);
    }

    #[test]
    fn the_ledger_counts_each_response_once_on_its_local_day() {
        let mut ledger = TokenLedger::default();
        let day = 20_000_i64;
        let turn = |key: &str, at: i64, fresh: u64| ClaudeTurn {
            key: Some(key.to_owned()),
            at: Some(at),
            fresh,
            context: 0,
            model: None,
            sidechain: false,
        };
        let noon = day * SECONDS_PER_DAY + 12 * 3600;
        ledger.add_claude(&turn("a", noon, 100), 0);
        ledger.add_claude(&turn("a", noon, 100), 0);
        ledger.add_claude(&turn("b", noon - SECONDS_PER_DAY, 40), 0);
        // Eleven at night in UTC is the next morning three hours east.
        ledger.add_claude(&turn("c", noon + 11 * 3600, 7), 3 * 3600);
        let codex = |at: i64, total: u64| CodexTokens {
            at: Some(at),
            total_fresh: total,
            context: 0,
            window: None,
        };
        ledger.add_codex("s", &codex(noon, 50), 0);
        ledger.add_codex("s", &codex(noon, 50), 0);
        ledger.add_codex("s", &codex(noon, 80), 0);
        let history = ledger.history(day);
        assert_eq!(history.today(), 100 + 80);
        assert_eq!(history.days[TOKEN_DAYS - 2], 40);
        assert_eq!(ledger.history(day + 1).today(), 7);
        ledger.prune(day + TOKEN_DAYS as i64);
        assert_eq!(ledger.history(day + TOKEN_DAYS as i64).total(), 7);
    }

    #[test]
    fn labels_read_like_activity_monitor() {
        assert_eq!(memory_label(412 * 1024 * 1024), "412 MB");
        assert_eq!(memory_label(1_288_490_189), "1.2 GB");
        assert_eq!(memory_label(300), "1 KB");
        assert_eq!(token_label(950), "950");
        assert_eq!(token_label(12_400), "12K");
        assert_eq!(token_label(1_250_000), "1.2M");
        assert_eq!(token_label(8_000_000), "8M");
    }
}
