use crate::{ports::terminal::TerminalRgb, ui::theme::TerminalPalette};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use uuid::Uuid;

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
/// while retaining the original argv[0].
pub(super) fn process_invoked_name(process_id: u32) -> Option<String> {
    process_argv0(process_id)
        .or_else(|| process_executable_name(process_id))
        .and_then(|name| {
            Path::new(&name)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
}

#[cfg(target_os = "macos")]
fn process_argv0(process_id: u32) -> Option<String> {
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
    argv0_from_macos_procargs(&buffer)
}

#[cfg(target_os = "macos")]
fn argv0_from_macos_procargs(buffer: &[u8]) -> Option<String> {
    use std::mem::size_of;

    let mut offset = size_of::<libc::c_int>();
    offset += buffer.get(offset..)?.iter().position(|byte| *byte == 0)?;
    while buffer.get(offset) == Some(&0) {
        offset += 1;
    }
    let end = offset + buffer.get(offset..)?.iter().position(|byte| *byte == 0)?;
    (!buffer[offset..end].is_empty())
        .then(|| String::from_utf8_lossy(&buffer[offset..end]).into_owned())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_argv0(process_id: u32) -> Option<String> {
    let command_line = std::fs::read(format!("/proc/{process_id}/cmdline")).ok()?;
    let end = command_line.iter().position(|byte| *byte == 0)?;
    (!command_line[..end].is_empty())
        .then(|| String::from_utf8_lossy(&command_line[..end]).into_owned())
}

#[cfg(not(unix))]
fn process_argv0(_: u32) -> Option<String> {
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
    use crate::ui::theme;
    fn indexed_color(i: usize) -> TerminalRgb {
        indexed_color_with(i, &theme::terminal_palette())
    }

    #[test]
    fn macos_procargs_preserves_a_wrapper_argv0() {
        let mut procargs = 3i32.to_ne_bytes().to_vec();
        procargs
            .extend_from_slice(b"/private/cursor/node\0\0/usr/local/bin/cursor-agent\0index.js\0");
        assert_eq!(
            argv0_from_macos_procargs(&procargs).as_deref(),
            Some("/usr/local/bin/cursor-agent")
        );
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
