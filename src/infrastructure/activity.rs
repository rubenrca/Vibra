//! Samples what runs for the Activity view: the user's processes, the ports
//! they listen on, and the agents' transcripts. Read-only: nothing here
//! signals, starts, or stops a process.

mod transcripts;

pub use transcripts::{AgentSession, ContextReader, TokenCounter};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use uuid::Uuid;

use super::process::{CommandLimits, command_output};
use super::terminal_support::{process_invoked_name, process_working_directory};
use crate::domain::activity::{
    OutsideGroup, ProcessInfo, ProcessSample, ProcessTable, TreeUsage, parse_listening_ports,
    servers_in,
};
use crate::domain::agents::AgentKind;

const LSOF: &str = "/usr/sbin/lsof";
/// `lsof` answers in tens of milliseconds; a stuck one must not hold a sample.
const LSOF_TIMEOUT: Duration = Duration::from_secs(3);
const LSOF_OUTPUT_LIMIT: usize = 1024 * 1024;
/// Agent CLIs shipped as scripts run as one of these; their script names them.
const INTERPRETERS: [&str; 4] = ["node", "bun", "deno", "ruby"];

/// A pane to measure: its terminal's shell and everything that shell started.
#[derive(Debug, Clone, Copy)]
pub struct PaneProbe {
    pub pane_id: Uuid,
    pub shell_pid: u32,
}

/// Each pane's processes, and the servers and agents running outside Vibra.
pub fn sample_processes(panes: &[PaneProbe], home: Option<&Path>) -> ProcessSample {
    let table = ProcessTable::new(user_processes());
    let listening = listening_ports();
    let mut sample = ProcessSample::default();
    // Everything Vibra started, its panes included, is Vibra's.
    let mut own: HashSet<u32> = table.tree(std::process::id()).into_iter().collect();
    for pane in panes {
        let tree = table.tree(pane.shell_pid);
        own.extend(tree.iter().copied());
        sample.panes.insert(
            pane.pane_id,
            TreeUsage {
                memory: table.memory(&tree),
                servers: servers_in(&table, &tree, &listening),
            },
        );
    }
    if let Some(home) = home {
        sample.outside = outside_groups(&table, &listening, &own, home);
    }
    sample
}

/// Servers and agents Vibra did not start, grouped by the repository (or
/// folder) they run in. Apps launched from the Dock run in `/` and are not
/// development work, so only processes in the user's folders count.
fn outside_groups(
    table: &ProcessTable,
    listening: &BTreeMap<u32, Vec<u16>>,
    own: &HashSet<u32>,
    home: &Path,
) -> Vec<OutsideGroup> {
    let mut servers = Vec::new();
    let mut agents: HashMap<u32, AgentKind> = HashMap::new();
    for process in table.processes() {
        if own.contains(&process.pid) {
            continue;
        }
        if listening.contains_key(&process.pid) {
            servers.push(process.pid);
        }
        if let Some(kind) = agent_kind(process) {
            agents.insert(process.pid, kind);
        }
    }
    // A wrapper (`npx`, a launcher script) and the agent it started are one agent.
    let agents: HashMap<u32, AgentKind> = agents
        .iter()
        .filter(|(pid, kind)| {
            !table
                .ancestors(**pid)
                .iter()
                .any(|ancestor| agents.get(ancestor) == Some(*kind))
        })
        .map(|(pid, kind)| (*pid, *kind))
        .collect();

    struct Group {
        members: Vec<u32>,
        servers: Vec<u32>,
        agents: Vec<AgentKind>,
    }
    let mut groups: BTreeMap<PathBuf, Group> = BTreeMap::new();
    let candidates = servers
        .iter()
        .map(|pid| (*pid, None))
        .chain(agents.iter().map(|(pid, kind)| (*pid, Some(*kind))));
    for (pid, kind) in candidates {
        let Some(cwd) = process_working_directory(pid) else {
            continue;
        };
        if !in_user_folders(&cwd, home) {
            continue;
        }
        let group = groups
            .entry(project_root(&cwd, home))
            .or_insert_with(|| Group {
                members: Vec::new(),
                servers: Vec::new(),
                agents: Vec::new(),
            });
        group.members.push(pid);
        match kind {
            Some(kind) => group.agents.push(kind),
            None => group.servers.push(pid),
        }
    }
    groups
        .into_iter()
        .map(|(root, mut group)| {
            let pids: HashSet<u32> = group
                .members
                .iter()
                .flat_map(|pid| table.tree(*pid))
                .collect();
            group.servers.sort_unstable();
            group.agents.sort_by_key(|kind| kind.display_name());
            group.agents.dedup();
            OutsideGroup {
                root,
                memory: table.memory(&pids),
                servers: servers_in(table, &group.servers, listening),
                agents: group.agents,
            }
        })
        .collect()
}

fn agent_kind(process: &ProcessInfo) -> Option<AgentKind> {
    if let Some(kind) = AgentKind::from_process_name(&process.name) {
        return Some(kind);
    }
    let name = process.name.to_ascii_lowercase();
    let interpreted = INTERPRETERS.contains(&name.as_str()) || name.starts_with("python");
    if !interpreted {
        return None;
    }
    process_invoked_name(process.pid).and_then(|name| AgentKind::from_process_name(&name))
}

/// The home folder and what is under it, except Library and hidden folders,
/// where apps and editors keep their own helpers.
fn in_user_folders(cwd: &Path, home: &Path) -> bool {
    let Ok(relative) = cwd.strip_prefix(home) else {
        return false;
    };
    match relative.components().next() {
        None => true,
        Some(first) => {
            let first = first.as_os_str().to_string_lossy();
            first != "Library" && !first.starts_with('.')
        }
    }
}

/// The repository holding `cwd`, else `cwd` itself; never above the home folder.
fn project_root(cwd: &Path, home: &Path) -> PathBuf {
    cwd.ancestors()
        .take_while(|folder| *folder != home)
        .find(|folder| folder.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

fn listening_ports() -> BTreeMap<u32, Vec<u16>> {
    let mut command = Command::new(LSOF);
    command.args(["-nP", "-w", "-iTCP", "-sTCP:LISTEN", "-Fpn"]);
    let limits = CommandLimits {
        timeout: LSOF_TIMEOUT,
        stdout: LSOF_OUTPUT_LIMIT,
        stderr: 0,
    };
    // `lsof` exits with 1 when nothing listens; its output is still the answer.
    command_output(&mut command, None, limits)
        .map(|output| parse_listening_ports(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn user_processes() -> Vec<ProcessInfo> {
    use std::mem::{MaybeUninit, size_of};

    let uid = unsafe { libc::getuid() };
    // With no buffer, this counts the processes instead of listing them.
    let capacity = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let Ok(capacity) = usize::try_from(capacity) else {
        return Vec::new();
    };
    // Room for processes started between the two calls.
    let mut pids = vec![0 as libc::c_int; capacity + 64];
    let count = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * size_of::<libc::c_int>()) as libc::c_int,
        )
    };
    let count = usize::try_from(count).unwrap_or(0).min(pids.len());
    pids[..count]
        .iter()
        .copied()
        .filter(|pid| *pid > 0)
        .filter_map(|pid| {
            let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
            let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let read = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    info.as_mut_ptr().cast(),
                    size,
                )
            };
            if read != size {
                return None;
            }
            // SAFETY: the kernel filled the whole structure.
            let info = unsafe { info.assume_init() };
            if info.pbi_uid != uid {
                return None;
            }
            let name = Some(c_text(&info.pbi_name))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| c_text(&info.pbi_comm));
            Some(ProcessInfo {
                pid: pid as u32,
                parent: info.pbi_ppid,
                name,
                memory: physical_footprint(pid),
            })
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn user_processes() -> Vec<ProcessInfo> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn c_text(chars: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|ch| **ch != 0)
        .map(|ch| *ch as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// What Activity Monitor shows as a process's Memory.
#[cfg(target_os = "macos")]
fn physical_footprint(pid: libc::c_int) -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    let result =
        unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, usage.as_mut_ptr().cast()) };
    if result != 0 {
        return 0;
    }
    // SAFETY: a successful call filled the structure.
    unsafe { usage.assume_init() }.ri_phys_footprint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_work_in_the_users_folders_is_outside_work() {
        let home = Path::new("/Users/me");
        assert!(in_user_folders(Path::new("/Users/me"), home));
        assert!(in_user_folders(Path::new("/Users/me/Dev/api"), home));
        assert!(!in_user_folders(Path::new("/"), home));
        assert!(!in_user_folders(
            Path::new("/Users/me/Library/Caches"),
            home
        ));
        assert!(!in_user_folders(
            Path::new("/Users/me/.cursor/server"),
            home
        ));
        assert!(!in_user_folders(Path::new("/Users/other/Dev"), home));
    }

    #[test]
    fn work_belongs_to_its_repository() {
        let home = std::env::temp_dir().join(format!("vibra-activity-{}", Uuid::new_v4()));
        let repository = home.join("Dev/api");
        let nested = repository.join("packages/web");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        assert_eq!(project_root(&nested, &home), repository);
        let loose = home.join("notes");
        std::fs::create_dir_all(&loose).unwrap();
        assert_eq!(project_root(&loose, &home), loose);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_process_is_sampled_with_its_memory() {
        let processes = user_processes();
        let me = processes
            .iter()
            .find(|process| process.pid == std::process::id())
            .expect("the test runner is one of the user's processes");
        assert!(me.memory > 0);
        assert!(!me.name.is_empty());
    }
}
