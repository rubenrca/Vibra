//! Load the user's login environment, then run app-owned scripts with POSIX sh.
//! The outer shell never parses the script or its quoted application arguments.

use std::path::Path;
use std::process::Command;

const SCRIPT_ENV: &str = "VIBRA_INTERNAL_POSIX_SCRIPT";

pub(super) fn command(script: &str) -> Command {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/zsh".into());
    command_with(Path::new(&shell), script)
}

fn command_with(shell: &Path, script: &str) -> Command {
    let launcher = if shell.file_name().is_some_and(|name| name == "nu") {
        "exec /bin/sh -c $env.VIBRA_INTERNAL_POSIX_SCRIPT"
    } else {
        "exec /bin/sh -c \"$VIBRA_INTERNAL_POSIX_SCRIPT\""
    };
    let mut command = Command::new(shell);
    command.args(["-l", "-c", launcher]).env(SCRIPT_ENV, script);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::process::{CommandLimits, command_output};
    use std::time::Duration;

    #[test]
    fn login_environment_and_posix_scripts_preserve_input_and_literal_arguments() {
        // Additional real shells can be supplied without installing them globally.
        let mut shells = vec!["/bin/sh".into(), "/bin/bash".into(), "/bin/zsh".into()];
        if let Some(additional) = std::env::var_os("VIBRA_TEST_LOGIN_SHELLS") {
            shells.extend(std::env::split_paths(&additional));
        }
        for shell in shells {
            let root = std::env::temp_dir().join(format!("vibra-shell-{}", uuid::Uuid::new_v4()));
            let bin = root.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(
                root.join(".zprofile"),
                format!(
                    "export VIBRA_SHELL_TEST=login\nexport PATH={}:$PATH\n",
                    bin.display(),
                ),
            )
            .unwrap();
            let config = root.join("config");
            for (directory, file, content) in [
                (
                    "fish",
                    "config.fish",
                    format!(
                        "set -gx VIBRA_SHELL_TEST login\nset -gx PATH {} $PATH\n",
                        bin.display(),
                    ),
                ),
                (
                    "nushell",
                    "env.nu",
                    format!(
                        "$env.VIBRA_SHELL_TEST = 'login'\n$env.PATH = \
                     ($env.PATH | prepend '{}')\n",
                        bin.display(),
                    ),
                ),
                (
                    "nushell",
                    "login.nu",
                    format!(
                        "$env.VIBRA_SHELL_TEST = 'login'\n$env.PATH = \
                     ($env.PATH | prepend '{}')\n",
                        bin.display(),
                    ),
                ),
            ] {
                std::fs::create_dir_all(config.join(directory)).unwrap();
                std::fs::write(config.join(directory).join(file), content).unwrap();
            }
            let executable = bin.join("vibra-login-test");
            std::fs::write(&executable, "#!/bin/sh\nprintf 'login command\\n'\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let script = concat!(
                "if test -n \"$VIBRA_SHELL_TEST\"; then ",
                "printf '%s\\n' 'a b' '$(touch injected)' \"$VIBRA_SHELL_TEST\"; ",
                "if test \"$VIBRA_SHELL_TEST\" = login; then vibra-login-test; fi; ",
                "cat; else exit 12; fi"
            );
            let mut command = command_with(&shell, script);
            command
                .current_dir(&root)
                .env("VIBRA_SHELL_TEST", "inherited")
                .env("ZDOTDIR", &root)
                .env("XDG_CONFIG_HOME", &config);
            let output = command_output(
                &mut command,
                Some(b"stdin context\n".to_vec()),
                CommandLimits {
                    timeout: Duration::from_secs(5),
                    stdout: 4096,
                    stderr: 4096,
                },
            )
            .unwrap();
            assert!(output.status.success(), "{}: {:?}", shell.display(), output);
            let configured = shell.file_name().is_some_and(|name| {
                ["zsh", "fish", "nu"]
                    .iter()
                    .any(|candidate| name == *candidate)
            });
            let expected: &[u8] = if configured {
                b"a b\n$(touch injected)\nlogin\nlogin command\nstdin context\n"
            } else {
                b"a b\n$(touch injected)\ninherited\nstdin context\n"
            };
            assert_eq!(output.stdout, expected, "{}", shell.display());
            assert!(!root.join("injected").exists());
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
