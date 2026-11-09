//! Subliminal on DSM: uses Synology's Python 3.9 package and installs
//! Subliminal into the package's own folder (no system changes).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::config::var_dir;
use crate::{info, warn};

/// Python interpreters to try, most preferred first.
const PYTHON_CANDIDATES: &[&str] = &[
    "/var/packages/Python3.9/target/usr/bin/python3.9",
    "/var/packages/Python3.9/target/usr/bin/python3",
    "/usr/local/bin/python3.9",
    "/usr/local/bin/python3",
    "/usr/bin/python3",
    "/bin/python3",
];

/// How long one Subliminal run may take before it's stopped.
const RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Runs Subliminal's click CLI from the module, so it works whether or not a
/// console script was installed.
const SUBLIMINAL_ENTRY: &str = "import sys\ntry:\n    from subliminal.cli import cli as main\nexcept ImportError:\n    from subliminal.cli import subliminal as main\nsys.argv[0] = 'subliminal'\nsys.exit(main())";

#[derive(Clone, Debug, serde::Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum SubliminalState {
    Checking,
    NoPython,
    Installing { step: String },
    Ready { version: String, python: String },
    Error { message: String },
}

pub fn user_base() -> PathBuf {
    var_dir().join("python")
}

fn base_command(python: &str) -> Command {
    let mut cmd = Command::new(python);
    let var = var_dir();
    cmd.env("PYTHONUSERBASE", user_base())
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
        .env("PIP_NO_CACHE_DIR", "1")
        .env("PIP_NO_WARN_SCRIPT_LOCATION", "1")
        .env("HOME", &var)
        .env("TMPDIR", var.join("tmp"))
        .stdin(Stdio::null());
    cmd
}

/// Find a Python 3.8+ interpreter. `JENOVA_PYTHON` overrides (for testing).
pub fn find_python() -> Option<String> {
    let override_path = std::env::var("JENOVA_PYTHON").ok();
    override_path
        .iter()
        .map(String::as_str)
        .chain(PYTHON_CANDIDATES.iter().copied())
        .find(|candidate| {
            Path::new(candidate).exists()
                && Command::new(candidate)
                    .args(["-c", "import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
        })
        .map(str::to_string)
}

/// Installed Subliminal version, if any.
pub fn subliminal_version(python: &str) -> Option<String> {
    let output = base_command(python)
        .args(["-c", "import subliminal; print(subliminal.__version__)"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!version.is_empty()).then_some(version)
}

fn run_logged(mut cmd: Command, what: &str) -> Result<(), String> {
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{what}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = text.lines().rev().take(6).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    warn!("{what} failed: {}", tail.join(" | "));
    Err(format!(
        "{what} failed: {}",
        tail.last().copied().unwrap_or("unknown error")
    ))
}

fn has_pip(python: &str) -> bool {
    base_command(python)
        .args(["-m", "pip", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Install (or upgrade) Subliminal into the package folder.
/// `progress` receives a short description of each step.
pub fn install(python: &str, progress: &dyn Fn(&str)) -> Result<String, String> {
    let _ = std::fs::create_dir_all(var_dir().join("tmp"));
    let _ = std::fs::create_dir_all(user_base());

    if !has_pip(python) {
        progress("Setting up pip");
        let mut ensure = base_command(python);
        ensure.args(["-m", "ensurepip", "--user"]);
        if run_logged(ensure, "ensurepip").is_err() || !has_pip(python) {
            progress("Downloading pip");
            let get_pip = var_dir().join("tmp").join("get-pip.py");
            let mut download = base_command(python);
            download.args([
                "-c",
                "import sys, urllib.request; urllib.request.urlretrieve('https://bootstrap.pypa.io/get-pip.py', sys.argv[1])",
                &get_pip.to_string_lossy(),
            ]);
            run_logged(download, "Downloading pip")?;
            let mut setup = base_command(python);
            setup.arg(&get_pip).arg("--user");
            run_logged(setup, "Installing pip")?;
            let _ = std::fs::remove_file(&get_pip);
        }
    }

    progress("Installing Subliminal (this can take a few minutes)");
    let mut pip = base_command(python);
    pip.args([
        "-m",
        "pip",
        "install",
        "--user",
        "--upgrade",
        "--prefer-binary",
        "subliminal",
    ]);
    run_logged(pip, "Installing Subliminal")?;

    subliminal_version(python).ok_or_else(|| "Subliminal installed but can't be loaded".to_string())
}

/// Result of one Subliminal run.
pub struct RunOutput {
    pub success: bool,
    /// Last meaningful line of output, with secrets redacted.
    pub summary: String,
    pub timed_out: bool,
}

/// Run `subliminal <args>` and wait for it (or for `cancel`).
pub fn run_subliminal(python: &str, args: &[String], cancel: &AtomicBool) -> RunOutput {
    let mut cmd = base_command(python);
    cmd.args(["-c", SUBLIMINAL_ENTRY])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Lower priority so the NAS stays responsive.
    let mut child = match spawn_niced(cmd) {
        Ok(child) => child,
        Err(error) => {
            return RunOutput {
                success: false,
                summary: format!("Couldn't start Subliminal: {error}"),
                timed_out: false,
            }
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let read_all = |pipe: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut p) = pipe {
                use std::io::Read;
                let _ = p.read_to_string(&mut text);
            }
            text
        })
    };
    let out_handle = read_all(stdout);
    let err_handle = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut p) = stderr {
            use std::io::Read;
            let _ = p.read_to_string(&mut text);
        }
        text
    });

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if cancel.load(Ordering::SeqCst) || started.elapsed() > RUN_TIMEOUT {
            timed_out = !cancel.load(Ordering::SeqCst);
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    let summary = last_meaningful_line(&stdout)
        .or_else(|| last_meaningful_line(&stderr))
        .map(redact)
        .unwrap_or_default();
    RunOutput {
        success: status.is_some_and(|s| s.success()),
        summary,
        timed_out,
    }
}

fn spawn_niced(mut cmd: Command) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    // SAFETY: nice() is async-signal-safe and only affects the child.
    unsafe {
        cmd.pre_exec(|| {
            libc::nice(10);
            Ok(())
        });
    }
    cmd.spawn()
}

fn last_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .rev()
        .find(|line| !line.is_empty() && !line.starts_with("Collecting") && !line.contains('\u{2588}'))
        .map(|line| line.chars().take(200).collect())
}

/// Hide anything that looks like a credential.
fn redact(line: String) -> String {
    let lower = line.to_ascii_lowercase();
    if ["password", "apikey", "api_key", "token"].iter().any(|k| lower.contains(k)) {
        return "Provider login problem (check your OpenSubtitles.com details)".into();
    }
    line
}

pub fn log_state(state: &SubliminalState) {
    match state {
        SubliminalState::Ready { version, python } => {
            info!("Subliminal {version} ready (Python: {python})");
        }
        SubliminalState::NoPython => {
            warn!("No Python 3 found. Install \"Python 3.9\" from Package Center.");
        }
        SubliminalState::Error { message } => warn!("Subliminal: {message}"),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_skip_progress_and_redact_secrets() {
        let out = "Collecting videos\n\n1 video collected / 0 video ignored\nDownloaded 1 subtitle\n";
        assert_eq!(last_meaningful_line(out).as_deref(), Some("Downloaded 1 subtitle"));
        assert!(redact("invalid password for user".into()).contains("OpenSubtitles"));
    }
}
