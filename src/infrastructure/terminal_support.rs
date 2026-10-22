use crate::domain::agents::AgentKind;
use crate::ports::terminal::{TerminalPalette, TerminalRgb};
use std::sync::{OnceLock, RwLock};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Interpreter flags rarely exceed a few entries before the script path.
const MAX_PROCESS_ARGUMENTS: usize = 8;
/// A foreground job is a pipeline or a wrapper and its child, not a fleet.
const MAX_PROCESS_GROUP_MEMBERS: usize = 64;

/// Host tools (CI, agent shells, cargo wrappers) often export these to force
/// monochrome output. Interactive agent CLIs inside Vibra panes should not
/// inherit that — dock-launched and agent-launched builds must look the same.
pub(super) const COLOR_SUPPRESSING_ENV: &[&str] = &[
    "NO_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "FORCE_COLOR",
    "CARGO_TERM_COLOR",
    "NPM_CONFIG_COLOR",
    "PIP_NO_COLOR",
    "PY_COLORS",
    "NOCOLOR",
];

#[cfg(target_os = "macos")]
pub(super) fn process_working_directory(process_id: u32) -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::mem::{MaybeUninit, size_of};

    let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let info_size = size_of::<libc::proc_vnodepathinfo>();
    let bytes = unsafe {
        libc::proc_pidinfo(
            process_id as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            info_size as libc::c_int,
        )
    };
    if bytes != info_size as libc::c_int {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast::<libc::c_char>()) };
    let path = PathBuf::from(path.to_string_lossy().into_owned());
    path.is_dir().then_some(path)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn process_working_directory(process_id: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{process_id}/cwd")).ok()
}

#[cfg(not(unix))]
pub(super) fn process_working_directory(_: u32) -> Option<PathBuf> {
    None
}

/// Name used to invoke a process. This preserves wrapper identities such as
/// Cursor's `cursor-agent`, whose script replaces itself with a `node` binary
/// while retaining the original argv[0]. Agents shipped as scripts (`gemini`
/// is `#!/usr/bin/env node`) run as their interpreter, so a script argument
/// that names a known agent identifies the process instead.
pub(super) fn process_invoked_name(process_id: u32) -> Option<String> {
    let arguments = process_arguments(process_id).unwrap_or_default();
    let invoked = arguments
        .first()
        .cloned()
        .or_else(|| process_executable_name(process_id))
        .and_then(|name| path_basename(&name))?;
    Some(
        interpreted_agent_name(&invoked, arguments.get(1..).unwrap_or_default()).unwrap_or(invoked),
    )
}

/// An agent running beneath the leader of a foreground job, when the leader
/// is a wrapper that forks rather than execs it: `caffeinate -i claude`,
/// `npx @openai/codex`, `op run -- claude`.
#[cfg(target_os = "macos")]
pub(super) fn process_group_agent_name(process_group: u32) -> Option<String> {
    use std::mem::size_of_val;

    let mut members = [0 as libc::pid_t; MAX_PROCESS_GROUP_MEMBERS];
    // Unlike `proc_listpids`, this returns a count of PIDs, not bytes.
    let count = unsafe {
        libc::proc_listpgrppids(
            process_group as libc::pid_t,
            members.as_mut_ptr().cast(),
            size_of_val(&members) as libc::c_int,
        )
    };
    let count = usize::try_from(count).ok()?.min(members.len());
    members[..count]
        .iter()
        .filter(|member| **member > 0 && **member as u32 != process_group)
        .filter_map(|member| process_invoked_name(*member as u32))
        .find(|name| AgentKind::from_process_name(name).is_some())
}

#[cfg(not(target_os = "macos"))]
pub(super) fn process_group_agent_name(_: u32) -> Option<String> {
    None
}

fn path_basename(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

/// The agent an interpreter is running, judged by its script path: either the
/// script's own name or, inside `node_modules`, the package it belongs to.
fn interpreted_agent_name(interpreter: &str, arguments: &[String]) -> Option<String> {
    let interpreter = interpreter.trim_start_matches('-').to_ascii_lowercase();
    let is_shell = matches!(
        interpreter.as_str(),
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh"
    );
    let is_runtime = matches!(
        interpreter.as_str(),
        "node" | "nodejs" | "bun" | "deno" | "ruby"
    ) || interpreter.starts_with("python");
    if !is_shell && !is_runtime {
        return None;
    }
    // Shell flags such as `-c` take the script text as an argument, so only a
    // shell whose first argument is a path is running a script file.
    let script = if is_shell {
        arguments
            .first()
            .filter(|argument| !argument.starts_with('-'))?
    } else {
        arguments
            .iter()
            .find(|argument| !argument.starts_with('-') && argument.as_str() != "run")?
    };
    let path = Path::new(script);
    let components: Vec<_> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    let package = components
        .iter()
        .rposition(|component| component == "node_modules")
        .and_then(|index| {
            let scope_or_name = components.get(index + 1)?;
            if scope_or_name.starts_with('@') {
                components.get(index + 2)
            } else {
                Some(scope_or_name)
            }
        })
        .cloned();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned());
    [stem, package]
        .into_iter()
        .flatten()
        .find(|name| AgentKind::from_process_name(name).is_some())
}

#[cfg(target_os = "macos")]
fn process_arguments(process_id: u32) -> Option<Vec<String>> {
    use std::mem::size_of;

    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        process_id as libc::c_int,
    ];
    let mut size = 0usize;
    let size_result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if size_result != 0 || size <= size_of::<libc::c_int>() || size > 1024 * 1024 {
        return None;
    }

    let mut buffer = vec![0u8; size];
    let read_result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read_result != 0 {
        return None;
    }
    buffer.truncate(size);
    arguments_from_macos_procargs(&buffer)
}

/// `KERN_PROCARGS2` is `argc`, the executable path, NUL padding, then argv.
/// Only the first few arguments are needed to name the process.
#[cfg(target_os = "macos")]
fn arguments_from_macos_procargs(buffer: &[u8]) -> Option<Vec<String>> {
    use std::mem::size_of;

    let argc = i32::from_ne_bytes(buffer.get(..size_of::<libc::c_int>())?.try_into().ok()?);
    let mut offset = size_of::<libc::c_int>();
    offset += buffer.get(offset..)?.iter().position(|byte| *byte == 0)?;
    while buffer.get(offset) == Some(&0) {
        offset += 1;
    }
    let arguments: Vec<_> = buffer
        .get(offset..)?
        .split(|byte| *byte == 0)
        .take(argc.clamp(0, MAX_PROCESS_ARGUMENTS as i32) as usize)
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect();
    arguments
        .first()
        .is_some_and(|argv0| !argv0.is_empty())
        .then_some(arguments)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_arguments(process_id: u32) -> Option<Vec<String>> {
    let command_line = std::fs::read(format!("/proc/{process_id}/cmdline")).ok()?;
    let arguments: Vec<_> = command_line
        .split(|byte| *byte == 0)
        .take(MAX_PROCESS_ARGUMENTS)
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect();
    arguments
        .first()
        .is_some_and(|argv0| !argv0.is_empty())
        .then_some(arguments)
}

#[cfg(not(unix))]
fn process_arguments(_: u32) -> Option<Vec<String>> {
    None
}

/// Executable basename of a process (e.g. `codex`, `claude`, `grok`).
#[cfg(target_os = "macos")]
fn process_executable_name(process_id: u32) -> Option<String> {
    use std::ffi::CStr;

    let mut buffer = [0i8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let bytes = unsafe {
        libc::proc_pidpath(
            process_id as libc::c_int,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        )
    };
    if bytes <= 0 {
        return None;
    }
    let path = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_executable_name(process_id: u32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{process_id}/exe")).ok()?;
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

#[cfg(not(unix))]
fn process_executable_name(_: u32) -> Option<String> {
    None
}

fn palette_slot() -> &'static RwLock<(u64, TerminalPalette)> {
    static SLOT: OnceLock<RwLock<(u64, TerminalPalette)>> = OnceLock::new();
    SLOT.get_or_init(|| {
        RwLock::new((
            0,
            TerminalPalette {
                background: TerminalRgb::new(0, 0, 0),
                foreground: TerminalRgb::new(255, 255, 255),
                cursor: TerminalRgb::new(255, 255, 255),
                ansi: [TerminalRgb::new(0, 0, 0); 16],
            },
        ))
    })
}

pub(crate) fn publish_terminal_palette(generation: u64, palette: TerminalPalette) {
    *palette_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = (generation, palette);
}

pub(super) fn current_terminal_palette() -> (u64, TerminalPalette) {
    *palette_slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn indexed_color_with(index: usize, palette: &TerminalPalette) -> TerminalRgb {
    match index {
        0..=15 => palette.ansi[index],
        16..=231 => {
            let value = index - 16;
            let component = |part: usize| [0, 95, 135, 175, 215, 255][part];
            TerminalRgb::new(
                component(value / 36),
                component((value / 6) % 6),
                component(value % 6),
            )
        }
        232..=255 => {
            let gray = 8 + ((index - 232) * 10) as u8;
            TerminalRgb::new(gray, gray, gray)
        }
        256 => palette.foreground,
        257 => palette.background,
        258 => palette.cursor,
        _ => palette.foreground,
    }
}

fn is_color_suppressing_env(key: &str) -> bool {
    COLOR_SUPPRESSING_ENV
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(key))
}

pub(super) fn terminal_child_environment(
    session_id: Uuid,
    extra_environment: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("TERM_PROGRAM".into(), "Vibra".into());
    env.insert(
        "TERM_PROGRAM_VERSION".into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    env.insert("VIBRA_SESSION_ID".into(), session_id.to_string());
    for (key, value) in extra_environment {
        if is_color_suppressing_env(key) {
            continue;
        }
        env.insert(key.clone(), value.clone());
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    fn indexed_color(i: usize) -> TerminalRgb {
        indexed_color_with(i, &current_terminal_palette().1)
    }

    #[test]
    fn macos_procargs_preserves_a_wrapper_argv0() {
        let mut procargs = 2i32.to_ne_bytes().to_vec();
        procargs.extend_from_slice(
            b"/private/cursor/node\0\0/usr/local/bin/cursor-agent\0index.js\0HOME=/x\0",
        );
        assert_eq!(
            arguments_from_macos_procargs(&procargs).unwrap(),
            ["/usr/local/bin/cursor-agent", "index.js"]
        );
    }

    #[test]
    fn interpreters_are_named_after_the_agent_script_they_run() {
        let args = |values: &[&str]| {
            values
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            interpreted_agent_name("node", &args(&["/opt/homebrew/bin/gemini", "-y"])).as_deref(),
            Some("gemini")
        );
        assert_eq!(
            interpreted_agent_name(
                "node",
                &args(&[
                    "--max-old-space-size=8192",
                    "/opt/homebrew/lib/node_modules/@google/gemini-cli/dist/index.js",
                ])
            )
            .as_deref(),
            Some("gemini-cli")
        );
        assert_eq!(
            interpreted_agent_name(
                "bun",
                &args(&["/x/node_modules/opencode-ai/bin/opencode.js"])
            )
            .as_deref(),
            Some("opencode")
        );
        assert_eq!(
            interpreted_agent_name("python3", &args(&["/Users/me/.local/bin/aider"])).as_deref(),
            Some("aider")
        );
        assert!(interpreted_agent_name("node", &args(&["server.js"])).is_none());
        assert!(interpreted_agent_name("zsh", &args(&["-c", "codex"])).is_none());
        assert!(interpreted_agent_name("-zsh", &[]).is_none());
        assert!(interpreted_agent_name("codex", &args(&["/x/claude"])).is_none());
    }

    #[test]
    fn invoked_name_uses_argv0_before_the_runtime_executable() {
        use std::os::unix::process::CommandExt;

        let mut child = std::process::Command::new("/bin/sleep")
            .arg0("/usr/local/bin/cursor-agent")
            .arg("5")
            .spawn()
            .unwrap();
        assert_eq!(
            process_invoked_name(child.id()).as_deref(),
            Some("cursor-agent")
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn process_group_scan_finds_an_agent_beneath_a_wrapper() {
        use std::os::unix::process::CommandExt;

        let mut wrapper = std::process::Command::new("/bin/sleep")
            .arg("5")
            .process_group(0)
            .spawn()
            .unwrap();
        let group = wrapper.id();
        assert_eq!(process_group_agent_name(group), None);
        let mut agent = std::process::Command::new("/bin/sleep")
            .arg0("/usr/local/bin/codex")
            .arg("5")
            .process_group(group as i32)
            .spawn()
            .unwrap();
        assert_eq!(process_group_agent_name(group).as_deref(), Some("codex"));
        for child in [&mut agent, &mut wrapper] {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    #[test]
    fn maps_the_entire_xterm_color_cube() {
        assert_eq!(indexed_color(16), TerminalRgb::new(0, 0, 0));
        assert_eq!(indexed_color(21), TerminalRgb::new(0, 0, 255));
        assert_eq!(indexed_color(231), TerminalRgb::new(255, 255, 255));
        assert_eq!(indexed_color(255), TerminalRgb::new(238, 238, 238));
    }

    #[test]
    fn child_environment_drops_color_suppression_keys() {
        let mut extra = HashMap::new();
        extra.insert("NO_COLOR".into(), "1".into());
        extra.insert("FORCE_COLOR".into(), "0".into());
        extra.insert("VIBRA_PANE_ID".into(), "pane".into());
        let env = terminal_child_environment(Uuid::nil(), &extra);
        assert!(!env.contains_key("NO_COLOR"));
        assert!(!env.contains_key("FORCE_COLOR"));
        assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));
        assert_eq!(env.get("VIBRA_PANE_ID").map(String::as_str), Some("pane"));
    }

    #[test]
    fn reads_the_current_process_working_directory() {
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        let actual = process_working_directory(std::process::id())
            .unwrap()
            .canonicalize()
            .unwrap();

        assert_eq!(actual, expected);
    }
}
