//! How each supported agent is launched in ACP mode.
//!
//! Resolution only looks at the file system. It never spawns a process, so an
//! availability check cannot start a session or a sign-in.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::domain::agent_chat::ChatAgent;

/// Pinned adapter packages. `npx` downloads them on first use.
const CLAUDE_ACP_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp@0.89.1";
const CODEX_ACP_PACKAGE: &str = "@agentclientprotocol/codex-acp@2.2.2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Extra environment on top of the user's login environment.
    pub env: Vec<(String, String)>,
}

/// Why an agent cannot be launched. `Display` text is user-facing English.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The agent CLI itself is not installed.
    AgentNotFound { agent: ChatAgent },
    /// The ACP adapter needs Node.js and none was found.
    NodeNotFound { agent: ChatAgent },
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentNotFound { agent } => write!(f, "{} is not installed.", agent.label()),
            Self::NodeNotFound { agent } => {
                write!(
                    f,
                    "{} chat needs Node.js, which was not found.",
                    agent.label()
                )
            }
        }
    }
}

impl std::error::Error for LaunchError {}

/// Resolves the launch command for `agent` from the process environment.
pub fn resolve_launch(agent: ChatAgent) -> Result<AgentLaunch, LaunchError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cache = std::env::var_os("npm_config_cache")
        .or_else(|| std::env::var_os("NPM_CONFIG_CACHE"))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".npm")));
    resolve_with_cache(
        agent,
        &search_dirs(home.as_deref(), std::env::var_os("PATH").as_deref()),
        cache.as_deref(),
    )
}

/// Whether `agent` can be launched right now. Used by the UI to enable chat.
// Until the chat view calls it, only the tests do.
pub fn is_available(agent: ChatAgent) -> bool {
    resolve_launch(agent).is_ok()
}

/// `resolve_launch` with its inputs injected, so tests control the search.
#[cfg(test)]
fn resolve_with_environment(
    agent: ChatAgent,
    home: Option<&Path>,
    path: Option<&OsStr>,
) -> Result<AgentLaunch, LaunchError> {
    resolve_in(agent, &search_dirs(home, path))
}

/// Directories searched for executables, in priority order. A Finder-started
/// app gets a minimal PATH, so the usual install locations are appended.
fn search_dirs(home: Option<&Path>, path: Option<&OsStr>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = path
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        // An empty PATH entry means the current directory; never search it.
        .filter(|dir| !dir.as_os_str().is_empty())
        .collect();
    if let Some(home) = home {
        dirs.push(home.join(".local/bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    if let Some(home) = home {
        dirs.push(home.join(".bun/bin"));
        dirs.push(home.join(".volta/bin"));
        dirs.extend(nvm_default_bin(home));
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    dirs
}

/// The `bin` directory of nvm's default Node.js, when it is installed exactly
/// as the alias file names it. Other nvm aliases are not followed.
fn nvm_default_bin(home: &Path) -> Option<PathBuf> {
    let alias = std::fs::read_to_string(home.join(".nvm/alias/default")).ok()?;
    let alias = alias.trim();
    if alias.is_empty() {
        return None;
    }
    let version = if alias.starts_with('v') {
        alias.to_owned()
    } else {
        format!("v{alias}")
    };
    let bin = home.join(".nvm/versions/node").join(version).join("bin");
    bin.is_dir().then_some(bin)
}

#[cfg(test)]
fn resolve_in(agent: ChatAgent, dirs: &[PathBuf]) -> Result<AgentLaunch, LaunchError> {
    resolve_with_cache(agent, dirs, None)
}

fn resolve_with_cache(
    agent: ChatAgent,
    dirs: &[PathBuf],
    cache: Option<&Path>,
) -> Result<AgentLaunch, LaunchError> {
    let path_env = ("PATH".to_owned(), path_value(dirs));
    match agent {
        ChatAgent::Grok => {
            let program =
                find_executable("grok", dirs).ok_or(LaunchError::AgentNotFound { agent })?;
            Ok(AgentLaunch {
                program,
                args: vec!["agent".to_owned(), "stdio".to_owned()],
                env: vec![path_env],
            })
        }
        ChatAgent::Claude => resolve_adapter(
            agent,
            dirs,
            AdapterSpec {
                cli: "claude",
                cli_env: "CLAUDE_CODE_EXECUTABLE",
                adapter_bin: "claude-agent-acp",
                package: CLAUDE_ACP_PACKAGE,
            },
            cache,
            path_env,
        ),
        ChatAgent::Codex => resolve_adapter(
            agent,
            dirs,
            AdapterSpec {
                cli: "codex",
                cli_env: "CODEX_PATH",
                adapter_bin: "codex-acp",
                package: CODEX_ACP_PACKAGE,
            },
            cache,
            path_env,
        ),
    }
}

struct AdapterSpec {
    /// The user's agent CLI, which the adapter must drive.
    cli: &'static str,
    /// Env var that points the adapter at `cli`. Without it the adapter runs
    /// a bundled copy of the CLI, which may differ from the user's install.
    cli_env: &'static str,
    /// Binary name when the adapter is installed globally.
    adapter_bin: &'static str,
    package: &'static str,
}

fn resolve_adapter(
    agent: ChatAgent,
    dirs: &[PathBuf],
    spec: AdapterSpec,
    cache: Option<&Path>,
    path_env: (String, String),
) -> Result<AgentLaunch, LaunchError> {
    // Check the CLI first: without it the chat cannot work at all, whatever Node.js does.
    let cli_path = find_executable(spec.cli, dirs).ok_or(LaunchError::AgentNotFound { agent })?;
    // Both adapter forms are Node programs, so Node.js is always required.
    let node = find_executable("node", dirs).ok_or(LaunchError::NodeNotFound { agent })?;
    let (program, args) = match find_executable(spec.adapter_bin, dirs) {
        Some(adapter) => (adapter, Vec::new()),
        None => {
            if let Some(entry) = cache.and_then(|cache| cached_adapter_entry(cache, &spec)) {
                // The exact pinned adapter is already installed. Starting it
                // with Node skips npx's package resolution on every new chat.
                (node, vec![entry.to_string_lossy().into_owned()])
            } else {
                let npx =
                    find_executable("npx", dirs).ok_or(LaunchError::NodeNotFound { agent })?;
                (npx, vec!["-y".to_owned(), spec.package.to_owned()])
            }
        }
    };
    Ok(AgentLaunch {
        program,
        args,
        env: vec![
            path_env,
            (spec.cli_env.to_owned(), cli_path.display().to_string()),
        ],
    })
}

/// Reuses only a complete entry point of the pinned package. Missing, stale or
/// unrecognized npm cache layouts fall back to npx's normal install path.
fn cached_adapter_entry(cache: &Path, spec: &AdapterSpec) -> Option<PathBuf> {
    let (name, version) = spec.package.rsplit_once('@')?;
    for directory in std::fs::read_dir(cache.join("_npx")).ok()?.flatten() {
        let package = directory.path().join("node_modules").join(name);
        let Some(manifest) = std::fs::read(package.join("package.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            continue;
        };
        if manifest["name"].as_str() != Some(name) || manifest["version"].as_str() != Some(version)
        {
            continue;
        }
        let Some(bin) = manifest["bin"]
            .as_str()
            .or_else(|| manifest["bin"][spec.adapter_bin].as_str())
        else {
            continue;
        };
        let bin = Path::new(bin);
        if !bin
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            continue;
        }
        let entry = package.join(bin);
        if entry.is_file() {
            return Some(entry);
        }
    }
    None
}

/// The first executable regular file named `name` in `dirs`.
fn find_executable(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    // `metadata` follows symlinks, so a broken link is not a candidate.
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn path_value(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .filter_map(|dir| dir.to_str())
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A temporary directory tree with fake executables, removed on drop.
    struct Sandbox {
        root: PathBuf,
    }

    impl Sandbox {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("vibra-launch-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let dir = self.root.join(relative);
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        /// Creates an executable script named `name` in `dir`.
        fn exe(&self, dir: &Path, name: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn env_value(launch: &AgentLaunch, key: &str) -> Option<String> {
        launch
            .env
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }

    #[test]
    fn grok_runs_agent_stdio_from_the_search_path() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        let grok = sandbox.exe(&bin, "grok");
        let dirs = vec![bin.clone()];

        let launch = resolve_in(ChatAgent::Grok, &dirs).unwrap();

        assert_eq!(launch.program, grok);
        assert_eq!(launch.args, ["agent", "stdio"]);
        assert_eq!(
            env_value(&launch, "PATH").as_deref(),
            Some(bin.to_str().unwrap())
        );
    }

    #[test]
    fn missing_grok_is_agent_not_found() {
        let sandbox = Sandbox::new();
        let empty = sandbox.dir("empty");

        assert_eq!(
            resolve_in(ChatAgent::Grok, &[empty]),
            Err(LaunchError::AgentNotFound {
                agent: ChatAgent::Grok
            })
        );
    }

    #[test]
    fn non_executable_file_is_not_a_launch_candidate() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        fs::write(bin.join("grok"), "not a program").unwrap();

        assert!(matches!(
            resolve_in(ChatAgent::Grok, &[bin]),
            Err(LaunchError::AgentNotFound { .. })
        ));
    }

    #[test]
    fn claude_without_cli_is_agent_not_found_even_with_node() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        sandbox.exe(&bin, "node");
        sandbox.exe(&bin, "npx");

        assert_eq!(
            resolve_in(ChatAgent::Claude, &[bin]),
            Err(LaunchError::AgentNotFound {
                agent: ChatAgent::Claude
            })
        );
    }

    #[test]
    fn claude_without_node_is_node_not_found() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        sandbox.exe(&bin, "claude");

        assert_eq!(
            resolve_in(ChatAgent::Claude, &[bin]),
            Err(LaunchError::NodeNotFound {
                agent: ChatAgent::Claude
            })
        );
    }

    #[test]
    fn claude_without_npx_or_adapter_is_node_not_found() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        sandbox.exe(&bin, "claude");
        sandbox.exe(&bin, "node");

        assert_eq!(
            resolve_in(ChatAgent::Claude, &[bin]),
            Err(LaunchError::NodeNotFound {
                agent: ChatAgent::Claude
            })
        );
    }

    #[test]
    fn claude_prefers_an_adapter_binary_on_the_path() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        let adapter = sandbox.exe(&bin, "claude-agent-acp");
        let claude = sandbox.exe(&bin, "claude");
        sandbox.exe(&bin, "node");
        sandbox.exe(&bin, "npx");

        let launch = resolve_in(ChatAgent::Claude, &[bin]).unwrap();

        assert_eq!(launch.program, adapter);
        assert!(launch.args.is_empty());
        assert_eq!(
            env_value(&launch, "CLAUDE_CODE_EXECUTABLE"),
            Some(claude.display().to_string())
        );
    }

    #[test]
    fn claude_falls_back_to_the_pinned_npx_package_with_the_user_cli() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        let claude = sandbox.exe(&bin, "claude");
        sandbox.exe(&bin, "node");
        let npx = sandbox.exe(&bin, "npx");

        let launch = resolve_in(ChatAgent::Claude, &[bin]).unwrap();

        assert_eq!(launch.program, npx);
        assert_eq!(
            launch.args,
            ["-y", "@agentclientprotocol/claude-agent-acp@0.89.1"]
        );
        assert_eq!(
            env_value(&launch, "CLAUDE_CODE_EXECUTABLE"),
            Some(claude.display().to_string())
        );
    }

    fn cache_adapter(sandbox: &Sandbox, name: &str, version: &str, entry: &str) -> PathBuf {
        let package = sandbox.dir(&format!("cache/_npx/{version}/node_modules/{name}"));
        fs::write(
            package.join("package.json"),
            serde_json::json!({
                "name": name,
                "version": version,
                "bin": {name.rsplit('/').next().unwrap(): entry}
            })
            .to_string(),
        )
        .unwrap();
        let path = package.join(entry);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "// cached adapter entry point").unwrap();
        path
    }

    #[test]
    fn cached_pinned_adapters_run_with_node_without_needing_npx() {
        for (agent, cli, name, version, variable) in [
            (
                ChatAgent::Claude,
                "claude",
                "@agentclientprotocol/claude-agent-acp",
                "0.89.1",
                "CLAUDE_CODE_EXECUTABLE",
            ),
            (
                ChatAgent::Codex,
                "codex",
                "@agentclientprotocol/codex-acp",
                "2.2.2",
                "CODEX_PATH",
            ),
        ] {
            let sandbox = Sandbox::new();
            let bin = sandbox.dir("bin");
            let cli = sandbox.exe(&bin, cli);
            let node = sandbox.exe(&bin, "node");
            let entry = cache_adapter(&sandbox, name, version, "dist/index.js");
            let launch =
                resolve_with_cache(agent, &[bin], Some(&sandbox.root.join("cache"))).unwrap();
            assert_eq!(launch.program, node);
            assert_eq!(launch.args, [entry.to_string_lossy()]);
            assert_eq!(
                env_value(&launch, variable),
                Some(cli.to_string_lossy().into_owned())
            );
        }
    }

    #[test]
    fn stale_or_incomplete_cache_falls_back_to_the_pinned_npx_package() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        sandbox.exe(&bin, "claude");
        sandbox.exe(&bin, "node");
        let npx = sandbox.exe(&bin, "npx");
        cache_adapter(
            &sandbox,
            "@agentclientprotocol/claude-agent-acp",
            "0.66.0",
            "dist/index.js",
        );
        let pinned = cache_adapter(
            &sandbox,
            "@agentclientprotocol/claude-agent-acp",
            "0.89.1",
            "dist/index.js",
        );
        fs::remove_file(pinned).unwrap();
        let launch =
            resolve_with_cache(ChatAgent::Claude, &[bin], Some(&sandbox.root.join("cache")))
                .unwrap();
        assert_eq!(launch.program, npx);
        assert_eq!(launch.args, ["-y", CLAUDE_ACP_PACKAGE]);
    }

    #[test]
    fn codex_falls_back_to_the_pinned_npx_package_with_the_user_cli() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        let codex = sandbox.exe(&bin, "codex");
        sandbox.exe(&bin, "node");
        let npx = sandbox.exe(&bin, "npx");

        let launch = resolve_in(ChatAgent::Codex, &[bin]).unwrap();

        assert_eq!(launch.program, npx);
        assert_eq!(launch.args, ["-y", "@agentclientprotocol/codex-acp@2.2.2"]);
        assert_eq!(
            env_value(&launch, "CODEX_PATH"),
            Some(codex.display().to_string())
        );
    }

    #[test]
    fn codex_prefers_an_adapter_binary_on_the_path() {
        let sandbox = Sandbox::new();
        let bin = sandbox.dir("bin");
        let adapter = sandbox.exe(&bin, "codex-acp");
        sandbox.exe(&bin, "codex");
        sandbox.exe(&bin, "node");

        let launch = resolve_in(ChatAgent::Codex, &[bin]).unwrap();

        assert_eq!(launch.program, adapter);
        assert!(launch.args.is_empty());
    }

    #[test]
    fn install_locations_are_found_when_the_path_is_minimal() {
        let sandbox = Sandbox::new();
        let home = sandbox.dir("home");
        let local_bin = sandbox.dir("home/.local/bin");
        let claude = sandbox.exe(&local_bin, "claude");
        sandbox.exe(&local_bin, "node");
        let npx = sandbox.exe(&local_bin, "npx");
        let empty_path = sandbox.dir("finder-path");

        let launch =
            resolve_with_environment(ChatAgent::Claude, Some(&home), Some(empty_path.as_os_str()))
                .unwrap();

        assert_eq!(launch.program, npx);
        assert_eq!(
            env_value(&launch, "CLAUDE_CODE_EXECUTABLE"),
            Some(claude.display().to_string())
        );
        let path = env_value(&launch, "PATH").unwrap();
        assert!(
            path.split(':')
                .any(|dir| Path::new(dir) == local_bin.as_path()),
            "PATH should include ~/.local/bin: {path}"
        );
    }

    #[test]
    fn nvm_default_node_bin_is_searched() {
        let sandbox = Sandbox::new();
        let home = sandbox.dir("home");
        let nvm_bin = sandbox.dir("home/.nvm/versions/node/v22.23.1/bin");
        fs::create_dir_all(home.join(".nvm/alias")).unwrap();
        fs::write(home.join(".nvm/alias/default"), "22.23.1\n").unwrap();

        let dirs = search_dirs(Some(&home), Some(OsStr::new("")));

        assert!(dirs.contains(&nvm_bin));
    }

    #[test]
    fn search_path_entries_come_before_install_locations() {
        let sandbox = Sandbox::new();
        let home = sandbox.dir("home");
        let custom = sandbox.dir("custom");

        let dirs = search_dirs(Some(&home), Some(custom.as_os_str()));

        assert_eq!(dirs.first(), Some(&custom));
        assert!(dirs.contains(&home.join(".local/bin")));
        let unique: std::collections::HashSet<_> = dirs.iter().collect();
        assert_eq!(unique.len(), dirs.len());
    }

    #[test]
    fn launch_errors_have_english_messages() {
        assert_eq!(
            LaunchError::AgentNotFound {
                agent: ChatAgent::Codex
            }
            .to_string(),
            "Codex is not installed."
        );
        assert_eq!(
            LaunchError::NodeNotFound {
                agent: ChatAgent::Claude
            }
            .to_string(),
            "Claude chat needs Node.js, which was not found."
        );
    }
}
