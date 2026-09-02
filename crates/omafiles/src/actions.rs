//! M9's file actions: terminal here, agent chat, LocalSend share, open-with.
//!
//! All four reuse Omarchy's own scripts rather than reimplementing them
//! (§6.6): omafiles inherits the user's configured terminal and agent, and
//! stays correct when Omarchy changes how either is launched.
//!
//! Two spawning disciplines, and which one an action gets is the whole design:
//!
//! - **Launch and let go** — the terminal and the agent outlive us on purpose.
//!   The child is `setsid`'d by the script itself, we report only a failure to
//!   *spawn* (the binary missing), and a reaper thread waits on the direct
//!   child so it cannot linger as a zombie.
//! - **Run and read the answer** — `xdg-open` and `omarchy-menu-share` exit
//!   quickly, and their exit status is the only place a failure is reported.
//!   These block, so the caller runs them on a background executor.
//!
//! Everything here is plain `std`: no gpui, testable headless. The pure
//! command-composition half is split from the spawning half so the tests can
//! assert what would run without running it.

use std::path::Path;
use std::process::{Child, Command, Stdio};

/// A composed command line: program and arguments, not yet a process.
///
/// The pure half of every action. Tests assert on this; the spawn functions
/// below turn it into a `Command` and add the working directory.
#[derive(Debug, PartialEq, Eq)]
pub struct CommandLine {
    pub program: &'static str,
    pub args: Vec<String>,
}

impl CommandLine {
    fn command(&self, cwd: Option<&Path>) -> Command {
        let mut command = Command::new(self.program);
        command.args(&self.args);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command
    }
}

// ----------------------------------------------------------------- terminal

/// `t`: a terminal in the given directory.
///
/// The body of `omarchy-launch-terminal`, minus its `omarchy-cmd-terminal-cwd`
/// call — that script asks the *active terminal* for its directory, and we
/// already know ours.
pub fn terminal_command(dir: &Path) -> CommandLine {
    CommandLine {
        program: "setsid",
        args: vec![
            "uwsm-app".to_string(),
            "--".to_string(),
            "xdg-terminal-exec".to_string(),
            format!("--dir={}", dir.display()),
        ],
    }
}

/// Launch a terminal and let go. Reports only a failure to spawn.
pub fn open_terminal(dir: &Path) -> Result<(), String> {
    launch(terminal_command(dir).command(Some(dir)))
        .map_err(|err| format!("could not launch a terminal: {err}"))
}

// -------------------------------------------------------------------- agent

/// The user's configured default agent, via `omarchy-default-agent`.
///
/// `Ok(None)` means Omarchy is present but no agent has been chosen — the
/// caller surfaces Omarchy's own picker rather than an error, because a
/// keypress that opens nothing explains nothing (the script's own words).
///
/// **Blocking** — it is one fork of a tiny script on a deliberate keystroke,
/// the same budget as the branch switcher's `git for-each-ref`.
pub fn default_agent() -> Result<Option<String>, String> {
    let output = Command::new("omarchy-default-agent")
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("could not run omarchy-default-agent: {err}"))?;
    let agent = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!agent.is_empty()).then_some(agent))
}

/// The default prompt offered in the dialog, editable before launch.
///
/// Composed from the entry rather than empty: the dialog should show what is
/// about to happen, and a prefilled prompt is also the documentation of what
/// `a` does.
pub fn compose_prompt(name: &str, is_dir: bool) -> String {
    if is_dir {
        format!("Look at the {name} directory and help me with it.")
    } else {
        format!("Look at the file \"{name}\" and help me with it.")
    }
}

/// Launch the agent with a prompt, in a terminal, and let go.
///
/// `omarchy-agent-prompt` inherits the working directory, which is how the
/// agent ends up beside the file it was asked about.
pub fn agent_prompt(prompt: &str, cwd: &Path) -> Result<(), String> {
    let line = CommandLine {
        program: "omarchy-agent-prompt",
        args: vec![prompt.to_string()],
    };
    launch(line.command(Some(cwd))).map_err(|err| format!("could not launch the agent: {err}"))
}

/// Omarchy's own default-agent picker, for when none is configured.
pub fn summon_agent_picker() -> Result<(), String> {
    let line = CommandLine {
        program: "omarchy-menu",
        args: vec!["summon".to_string(), "setup.default.agent".to_string()],
    };
    launch(line.command(None)).map_err(|err| format!("could not open the agent picker: {err}"))
}

// -------------------------------------------------------------------- share

/// `s`: share an entry via LocalSend, through `omarchy-menu-share`.
///
/// Through the script and never `localsend` directly — §6.6's warning stands:
/// the binary hangs on `--help`, so the one safe interface is Omarchy's, which
/// detaches it into its own systemd unit.
pub fn share_command(path: &Path, is_dir: bool) -> CommandLine {
    CommandLine {
        program: "omarchy-menu-share",
        args: vec![
            if is_dir { "folder" } else { "file" }.to_string(),
            path.display().to_string(),
        ],
    }
}

/// Share and read the answer. **Blocking** — background executor only.
///
/// The script itself exits as soon as `systemd-run` has taken LocalSend off
/// its hands, so waiting here is cheap and is what makes "sharing failed"
/// reportable at all.
pub fn share(path: &Path, is_dir: bool) -> Result<(), String> {
    let output = share_command(path, is_dir)
        .command(None)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("could not run omarchy-menu-share: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        "sharing failed".to_string()
    } else {
        stderr
    })
}

// ---------------------------------------------------------------- open-with

/// `⏎` on a file: hand it to the default application.
///
/// Our own `xdg-open` rather than gpui's `open_with_system`, deliberately:
/// gpui fires and forgets, logging failures where no user will see them, and
/// the entire reason this waited for M9 was to be able to say *in the window*
/// that opening failed (PLAN §7 M3). **Blocking** — background executor only.
pub fn open_path(path: &Path) -> Result<(), String> {
    let output = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("could not run xdg-open: {err}"))?;
    match output.status.code() {
        Some(0) => Ok(()),
        code => Err(open_failure(code, &String::from_utf8_lossy(&output.stderr))),
    }
}

/// xdg-open's documented exit codes, turned into a sentence.
///
/// Its stderr is usually empty — the codes are the actual interface — so the
/// mapping is written here rather than hoping for a message.
fn open_failure(code: Option<i32>, stderr: &str) -> String {
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    match code {
        Some(2) => "xdg-open: the file does not exist".to_string(),
        Some(3) => "xdg-open: no application is available to open this file".to_string(),
        Some(4) => "xdg-open: the application failed to launch".to_string(),
        Some(code) => format!("xdg-open failed (exit {code})"),
        None => "xdg-open was killed by a signal".to_string(),
    }
}

// ------------------------------------------------------------------ spawning

/// Spawn without waiting, and without leaving a zombie.
///
/// The child is expected to detach itself (`setsid` in the scripts), but the
/// *direct* child — `setsid`, or the script before it execs — still needs a
/// `wait`, or it sits defunct in the process table until we exit. A thread per
/// launch is cheap at the rate humans press `t`.
fn launch(mut command: Command) -> std::io::Result<()> {
    let child: Child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_terminal_line_matches_omarchys_own_script() {
        let line = terminal_command(Path::new("/home/x/Documents"));
        assert_eq!(line.program, "setsid");
        assert_eq!(
            line.args,
            vec![
                "uwsm-app",
                "--",
                "xdg-terminal-exec",
                "--dir=/home/x/Documents"
            ]
        );
    }

    #[test]
    fn share_distinguishes_files_from_folders() {
        let file = share_command(Path::new("/x/a.txt"), false);
        assert_eq!(file.args[0], "file");
        let folder = share_command(Path::new("/x/photos"), true);
        assert_eq!(folder.args[0], "folder");
        assert_eq!(folder.args[1], "/x/photos");
    }

    #[test]
    fn the_prompt_names_what_it_is_about() {
        assert!(compose_prompt("notes.md", false).contains("\"notes.md\""));
        assert!(compose_prompt("src", true).contains("src directory"));
    }

    #[test]
    fn open_failure_prefers_stderr_then_decodes_the_codes() {
        assert_eq!(open_failure(Some(3), "  real message  "), "real message");
        assert!(open_failure(Some(2), "").contains("does not exist"));
        assert!(open_failure(Some(3), "").contains("no application"));
        assert!(open_failure(Some(4), "").contains("failed to launch"));
        assert!(open_failure(Some(7), "").contains("exit 7"));
        assert!(open_failure(None, "").contains("signal"));
    }

    #[test]
    fn opening_a_missing_file_reports_rather_than_panics() {
        // Skip on a machine with no xdg-open at all — the error path we are
        // testing is the *exit code*, not a missing binary.
        if Command::new("xdg-open").arg("--version").output().is_err() {
            return;
        }
        let missing = PathBuf::from("/omafiles-definitely-does-not-exist");
        let result = open_path(&missing);
        assert!(result.is_err(), "opening a missing path must fail");
    }

    #[test]
    fn a_missing_binary_is_an_error_not_a_hang() {
        let line = CommandLine {
            program: "omafiles-no-such-binary",
            args: vec![],
        };
        assert!(launch(line.command(None)).is_err());
    }
}
