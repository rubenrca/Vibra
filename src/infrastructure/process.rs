//! Listening TCP processes descended from a terminal PTY.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const LOOPBACK: &[&str] = &["127.0.0.1", "::1", "0.0.0.0", "::", "localhost"];
const HTTP_PROCESS_HINTS: &[&str] = &[
    "node",
    "nodejs",
    "bun",
    "deno",
    "python",
    "python3",
    "ruby",
    "rails",
    "puma",
    "php",
    "caddy",
    "nginx",
    "uvicorn",
    "gunicorn",
    "hypercorn",
    "daphne",
    "next",
    "next-server",
    "vite",
    "nuxt",
    "astro",
    "remix",
    "webpack",
    "esbuild",
    "turbo",
    "wrangler",
    "storybook",
    "http-server",
    "serve",
    "json-server",
    "nest",
    "nestjs",
    "tsx",
    "nodemon",
    "fastapi",
    "flask",
    "django",
    "trunk",
    "miniserve",
    "air",
    "dotnet",
    "java",
    "gradle",
    "go",
    "busybox",
];
const HTTP_PORTS: &[u16] = &[
    80, 443, 1234, 1420, 18789, 24678, 3000, 3001, 3002, 4000, 4173, 4200, 4321, 5000, 5001, 5173,
    5174, 5500, 6006, 8000, 8001, 8080, 8081, 8088, 8443, 8888, 9000, 9090,
];
const NON_HTTP_PORTS: &[u16] = &[22, 53, 3306, 5432, 6379, 11211, 27017, 4222, 5672, 6380];
const NODE_TOOL_HINTS: &[&str] = &[
    "vite",
    "next",
    "nuxt",
    "astro",
    "remix",
    "webpack",
    "esbuild",
    "turbo",
    "wrangler",
    "storybook",
    "nest",
    "tsx",
    "nodemon",
    "serve",
    "http-server",
    "json-server",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenSocket {
    pub pid: u32,
    pub port: u16,
    pub ipv6: bool,
    pub address: String,
    pub name: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenPort {
    pub port: u16,
    pub ipv6: bool,
    pub address: String,
}

impl ListenPort {
    pub fn display_label(&self) -> String {
        if is_loopback_or_any(&self.address) {
            format!(":{}", self.port)
        } else if self.ipv6 {
            format!("[{}]:{}", self.address, self.port)
        } else {
            format!("{}:{}", self.address, self.port)
        }
    }

    pub fn http_url(&self) -> String {
        let host = if is_loopback_or_any(&self.address) {
            "localhost".to_owned()
        } else if self.ipv6 {
            format!("[{}]", self.address)
        } else {
            self.address.clone()
        };
        let https = self.port == 443;
        let scheme = if https { "https" } else { "http" };
        if (https && self.port == 443) || (!https && self.port == 80) {
            format!("{scheme}://{host}")
        } else {
            format!("{scheme}://{host}:{}", self.port)
        }
    }

    fn address_rank(&self) -> u8 {
        match self.address.as_str() {
            "127.0.0.1" | "localhost" => 0,
            "::1" => 1,
            "0.0.0.0" => 2,
            "::" => 3,
            _ if self.ipv6 => 5,
            _ => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupedServer {
    pub pid: u32,
    pub name: String,
    pub command: String,
    pub ports: Vec<ListenPort>,
}

pub fn openable_http_url(process_name: &str, ports: &[ListenPort]) -> Option<String> {
    ports
        .iter()
        .filter(|port| is_likely_http(process_name, port.port))
        .min_by_key(|port| (port.address_rank(), port.port))
        .map(ListenPort::http_url)
}

pub fn scan_listen_sockets(root_pid: u32) -> Vec<ListenSocket> {
    #[cfg(target_os = "macos")]
    {
        decode_sockets(|raw, cap| unsafe { vibra_scan_listen_sockets(root_pid, raw, cap) })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = root_pid;
        Vec::new()
    }
}

pub fn scan_pid_listen_sockets(pid: u32) -> Vec<ListenSocket> {
    #[cfg(target_os = "macos")]
    {
        decode_sockets(|raw, cap| unsafe { vibra_scan_pid_listen_sockets(pid, raw, cap) })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Vec::new()
    }
}

/// Listening sockets whose process cwd lives under one of `roots`.
pub fn scan_listen_sockets_under(roots: &[PathBuf]) -> Vec<ListenSocket> {
    let roots: Vec<&Path> = roots
        .iter()
        .map(PathBuf::as_path)
        .filter(|root| is_usable_root(root))
        .collect();
    if roots.is_empty() {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        scan_listen_sockets_under_macos(&roots)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        process_cwd_macos(pid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

pub fn path_is_under(path: &Path, root: &Path) -> bool {
    let root = normalize_path(root);
    if !is_usable_root(&root) {
        return false;
    }
    normalize_path(path).starts_with(&root)
}

fn is_usable_root(root: &Path) -> bool {
    let raw = root.as_os_str();
    !raw.is_empty() && raw != "/" && root.components().count() >= 2
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub fn group_listen_sockets(sockets: Vec<ListenSocket>) -> Vec<GroupedServer> {
    let mut grouped: HashMap<u32, GroupedServer> = HashMap::new();
    for socket in sockets {
        let command = socket.command.clone();
        let name = socket.name.clone();
        let entry = grouped.entry(socket.pid).or_insert_with(|| GroupedServer {
            pid: socket.pid,
            name: display_process_name(&name, &command),
            command: command.clone(),
            ports: Vec::new(),
        });
        let candidate = ListenPort {
            port: socket.port,
            ipv6: socket.ipv6,
            address: socket.address,
        };
        if let Some(existing) = entry
            .ports
            .iter_mut()
            .find(|port| port.port == candidate.port)
        {
            if candidate.address_rank() < existing.address_rank() {
                *existing = candidate;
            }
        } else {
            entry.ports.push(candidate);
        }
        if entry.command.is_empty() && !command.is_empty() {
            entry.command = command;
        }
    }
    let mut servers: Vec<_> = grouped.into_values().collect();
    for server in &mut servers {
        server
            .ports
            .sort_by_key(|port| (port.port, port.address_rank()));
    }
    servers.sort_by(|left, right| {
        left.ports
            .first()
            .map(|port| port.port)
            .cmp(&right.ports.first().map(|port| port.port))
            .then(left.name.cmp(&right.name))
            .then(left.pid.cmp(&right.pid))
    });
    servers
}

pub fn terminate_pid(pid: u32) -> bool {
    if pid <= 1 || pid == std::process::id() {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: SIGTERM to a user-space pid we listed as a PTY descendant.
        unsafe { libc::kill(pid as i32, libc::SIGTERM) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

pub fn display_process_name(exe: &str, command: &str) -> String {
    let base = Path::new(exe)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(exe)
        .trim();
    let base = if base.is_empty() { exe } else { base };
    let lower = base.to_ascii_lowercase();
    if matches!(lower.as_str(), "node" | "nodejs" | "bun" | "deno")
        && let Some(tool) = node_tool_from_command(command)
    {
        return tool.to_owned();
    }
    if base.is_empty() {
        "process".to_owned()
    } else {
        base.to_owned()
    }
}

pub fn is_likely_http(process_name: &str, port: u16) -> bool {
    if NON_HTTP_PORTS.contains(&port) {
        return false;
    }
    if HTTP_PORTS.contains(&port) {
        return true;
    }
    let lower = process_name.to_ascii_lowercase();
    HTTP_PROCESS_HINTS
        .iter()
        .any(|hint| lower == *hint || lower.contains(hint))
}

fn is_loopback_or_any(address: &str) -> bool {
    LOOPBACK.contains(&address)
}

fn node_tool_from_command(command: &str) -> Option<&'static str> {
    command.split_whitespace().find_map(|arg| {
        let file = Path::new(arg)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(arg)
            .to_ascii_lowercase();
        let stem = file
            .strip_suffix(".js")
            .or_else(|| file.strip_suffix(".mjs"))
            .or_else(|| file.strip_suffix(".cjs"))
            .unwrap_or(&file);
        NODE_TOOL_HINTS
            .iter()
            .copied()
            .find(|hint| stem == *hint || stem.starts_with(&format!("{hint}-")))
    })
}

#[cfg(target_os = "macos")]
fn decode_sockets(scan: impl FnOnce(*mut RawListenSocket, i32) -> i32) -> Vec<ListenSocket> {
    const CAPACITY: usize = 256;
    let mut raw = vec![RawListenSocket::default(); CAPACITY];
    // SAFETY: `raw` is a `CAPACITY`-entry buffer matching `VibraListenSocket`.
    let count = scan(raw.as_mut_ptr(), CAPACITY as i32);
    if count <= 0 {
        return Vec::new();
    }
    raw.into_iter()
        .take(count as usize)
        .filter_map(|row| {
            if row.pid == 0 || row.port == 0 {
                return None;
            }
            Some(ListenSocket {
                pid: row.pid,
                port: row.port,
                ipv6: row.ipv6 != 0,
                address: c_string(&row.address),
                name: c_string(&row.name),
                command: c_string(&row.command),
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn scan_listen_sockets_under_macos(roots: &[&Path]) -> Vec<ListenSocket> {
    const PID_CAPACITY: usize = 4096;
    let mut pids = vec![0u32; PID_CAPACITY];
    // SAFETY: buffer sized for `PID_CAPACITY` user pids.
    let count = unsafe { vibra_list_user_pids(pids.as_mut_ptr(), PID_CAPACITY as i32) };
    if count <= 0 {
        return Vec::new();
    }
    let normalized_roots: Vec<PathBuf> = roots.iter().map(|root| normalize_path(root)).collect();
    let self_pid = std::process::id();
    let mut sockets = Vec::new();
    for pid in pids.into_iter().take(count as usize) {
        if pid == 0 || pid == self_pid {
            continue;
        }
        let Some(cwd) = process_cwd_macos(pid) else {
            continue;
        };
        let cwd = normalize_path(&cwd);
        if !normalized_roots.iter().any(|root| cwd.starts_with(root)) {
            continue;
        }
        sockets.extend(scan_pid_listen_sockets(pid));
    }
    sockets
}

#[cfg(target_os = "macos")]
fn process_cwd_macos(pid: u32) -> Option<PathBuf> {
    let mut buffer = [0u8; 1024];
    // SAFETY: `buffer` matches the C cwd capacity.
    let ok = unsafe { vibra_pid_cwd(pid, buffer.as_mut_ptr().cast(), buffer.len() as i32) };
    if ok != 1 {
        return None;
    }
    let path = PathBuf::from(c_string(&buffer));
    path.is_dir().then_some(path)
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct RawListenSocket {
    pid: u32,
    port: u16,
    ipv6: u8,
    _pad: u8,
    address: [u8; 48],
    name: [u8; 64],
    command: [u8; 160],
}

#[cfg(target_os = "macos")]
impl Default for RawListenSocket {
    fn default() -> Self {
        Self {
            pid: 0,
            port: 0,
            ipv6: 0,
            _pad: 0,
            address: [0; 48],
            name: [0; 64],
            command: [0; 160],
        }
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn vibra_scan_listen_sockets(root_pid: u32, out: *mut RawListenSocket, capacity: i32) -> i32;
    fn vibra_scan_pid_listen_sockets(pid: u32, out: *mut RawListenSocket, capacity: i32) -> i32;
    fn vibra_list_user_pids(out: *mut u32, capacity: i32) -> i32;
    fn vibra_pid_cwd(pid: u32, out: *mut std::ffi::c_char, capacity: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_wrappers_surface_the_dev_tool_name() {
        assert_eq!(
            display_process_name(
                "node",
                "node /Users/me/app/node_modules/vite/bin/vite.js --port 5173"
            ),
            "vite"
        );
        assert_eq!(
            display_process_name("node", "node /x/node_modules/next/dist/bin/next dev"),
            "next"
        );
        assert_eq!(display_process_name("next-server", ""), "next-server");
        assert_eq!(
            display_process_name("/usr/bin/python3", "python3 -m http.server 8000"),
            "python3"
        );
    }

    #[test]
    fn http_urls_prefer_localhost_and_omit_default_ports() {
        let local = ListenPort {
            port: 5173,
            ipv6: false,
            address: "0.0.0.0".into(),
        };
        assert_eq!(local.http_url(), "http://localhost:5173");
        assert_eq!(local.display_label(), ":5173");
        let https = ListenPort {
            port: 443,
            ipv6: false,
            address: "127.0.0.1".into(),
        };
        assert_eq!(https.http_url(), "https://localhost");
        let lan = ListenPort {
            port: 3000,
            ipv6: false,
            address: "10.0.0.8".into(),
        };
        assert_eq!(lan.http_url(), "http://10.0.0.8:3000");
        assert_eq!(lan.display_label(), "10.0.0.8:3000");
    }

    #[test]
    fn likely_http_keeps_databases_out_of_the_browser_button() {
        assert!(is_likely_http("vite", 5173));
        assert!(is_likely_http("node", 3000));
        assert!(is_likely_http("python3", 8000));
        assert!(!is_likely_http("redis-server", 6379));
        assert!(!is_likely_http("postgres", 5432));
        assert!(!is_likely_http("sshd", 22));
    }

    #[test]
    fn grouping_collapses_ipv4_and_ipv6_aliases_for_the_same_port() {
        let grouped = group_listen_sockets(vec![
            ListenSocket {
                pid: 9,
                port: 5173,
                ipv6: true,
                address: "::".into(),
                name: "node".into(),
                command: "node vite.js".into(),
            },
            ListenSocket {
                pid: 9,
                port: 5173,
                ipv6: false,
                address: "127.0.0.1".into(),
                name: "node".into(),
                command: "node vite.js".into(),
            },
        ]);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].name, "vite");
        assert_eq!(grouped[0].ports.len(), 1);
        assert_eq!(grouped[0].ports[0].address, "127.0.0.1");
        assert_eq!(
            openable_http_url(&grouped[0].name, &grouped[0].ports).as_deref(),
            Some("http://localhost:5173")
        );
    }

    #[test]
    fn path_is_under_requires_a_real_project_prefix() {
        assert!(path_is_under(
            Path::new("/Users/me/Dev/MilenIA/apps/web"),
            Path::new("/Users/me/Dev/MilenIA")
        ));
        assert!(!path_is_under(
            Path::new("/Users/me/Dev/MilenIA-old/apps/web"),
            Path::new("/Users/me/Dev/MilenIA")
        ));
        assert!(!path_is_under(
            Path::new("/Users/me/Dev/MilenIA"),
            Path::new("/")
        ));
    }

    #[test]
    fn terminate_refuses_kernel_and_self() {
        assert!(!terminate_pid(0));
        assert!(!terminate_pid(1));
        assert!(!terminate_pid(std::process::id()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scan_sees_a_listening_socket_on_this_process() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let found = scan_listen_sockets(std::process::id());
        assert!(
            found.iter().any(|socket| socket.port == port),
            "expected :{port} in {found:?}"
        );
        let found_pid = scan_pid_listen_sockets(std::process::id());
        assert!(
            found_pid.iter().any(|socket| socket.port == port),
            "expected :{port} in {found_pid:?}"
        );
        drop(listener);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn process_cwd_matches_this_process() {
        let cwd = process_cwd(std::process::id()).expect("cwd");
        let expected = std::env::current_dir().unwrap();
        assert_eq!(cwd.canonicalize().ok(), expected.canonicalize().ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn under_scan_finds_a_child_listener_by_cwd() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};

        let root = std::env::temp_dir().join(format!("vibra-servers-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = r#"
import socket, time
s = socket.socket()
s.bind(("127.0.0.1", 0))
s.listen()
print(s.getsockname()[1], flush=True)
time.sleep(8)
"#;
        let mut child = Command::new("python3")
            .args(["-c", script])
            .current_dir(&root)
            .stdout(Stdio::piped())
            .spawn()
            .expect("python3");
        let stdout = child.stdout.take().unwrap();
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        let port: u16 = line.trim().parse().expect("port");
        let found = scan_listen_sockets_under(std::slice::from_ref(&root));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            found.iter().any(|socket| socket.port == port),
            "expected :{port} under {} in {found:?}",
            root.display()
        );
    }
}
