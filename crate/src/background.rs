//! Starting with Windows, and making sure only one Jenova runs.
//!
//! `jenova --tray` starts hidden in the tray (that's what the startup entry
//! runs). Older versions ran a separate `--background` copy next to the
//! window; that copy is stopped on launch and the flag now means `--tray`.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::settings::Settings;
#[allow(unused_imports)]
use crate::warn;

pub const TRAY_ARG: &str = "--tray";
/// What older versions' startup entry passes.
pub const LEGACY_BACKGROUND_ARG: &str = "--background";
const INSTANCE_LOCK: &str = "jenova_app.lock";
/// Held by older versions' background copy.
const LEGACY_WATCHER_LOCK: &str = "jenova_watcher.lock";
const LEGACY_GUI_LOCK: &str = "jenova_gui.lock";

/// Folder that holds the lock files (same place as the settings file).
fn state_dir() -> PathBuf {
    Settings::get_path()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

fn open_lock(name: &str) -> Option<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state_dir().join(name))
        .ok()
}

/// Whether some process holds the named lock.
fn lock_held(name: &str) -> bool {
    let path = state_dir().join(name);
    if !path.exists() {
        return false;
    }
    let Some(file) = open_lock(name) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(_) => false,
    }
}

/// Held for the app's whole lifetime so a second copy can tell. `wait`
/// keeps trying for a while (right after an update, while the old copy
/// closes).
pub fn hold_instance_lock(wait: Duration) -> Option<File> {
    let file = open_lock(INSTANCE_LOCK)?;
    let deadline = Instant::now() + wait;
    loop {
        if file.try_lock().is_ok() {
            return Some(file);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Stop the separate background copy older versions ran, and tidy its
/// lock files.
pub fn stop_legacy_background() {
    if lock_held(LEGACY_WATCHER_LOCK) {
        stop_processes_with_arg(LEGACY_BACKGROUND_ARG);
        let deadline = Instant::now() + Duration::from_secs(5);
        while lock_held(LEGACY_WATCHER_LOCK) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    for name in [LEGACY_WATCHER_LOCK, LEGACY_GUI_LOCK] {
        if !lock_held(name) {
            let _ = std::fs::remove_file(state_dir().join(name));
        }
    }
}

/// End other Jenova processes started with `arg`.
fn stop_processes_with_arg(arg: &str) {
    let me = std::process::id();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = format!(
            "Get-CimInstance Win32_Process -Filter \"Name LIKE 'jenova%.exe'\" | \
             Where-Object {{ $_.CommandLine -like '*{arg}*' -and $_.ProcessId -ne {me} }} | \
             ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}"
        );
        let result = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if let Err(error) = result {
            warn!("Could not stop the old background copy: {error}");
        }
    }
    #[cfg(unix)]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if pid == me {
                continue;
            }
            let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            let args: Vec<String> = cmdline
                .split(|b| *b == 0)
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect();
            let is_jenova = args.first().is_some_and(|exe| {
                Path::new(exe)
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("jenova"))
            });
            if is_jenova && args.iter().any(|a| a == arg) {
                let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
            }
        }
    }
}

/// Start-with-login registration.
pub mod startup {
    #[cfg(windows)]
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    #[cfg(windows)]
    const VALUE: &str = "Jenova";

    /// Whether this platform supports starting with login.
    pub fn supported() -> bool {
        cfg!(any(windows, all(unix, not(target_os = "macos"))))
    }

    fn command_line() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Ok(format!("\"{}\" {}", exe.display(), super::TRAY_ARG))
    }

    /// Register (or refresh) the startup entry for the current exe path.
    #[cfg(windows)]
    pub fn register() -> Result<(), String> {
        use winreg::enums::HKEY_CURRENT_USER;
        let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(RUN_KEY)
            .map_err(|e| e.to_string())?;
        key.set_value(VALUE, &command_line()?)
            .map_err(|e| e.to_string())
    }

    /// Remove the startup entry completely.
    #[cfg(windows)]
    pub fn unregister() -> Result<(), String> {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
        let key = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
            .map_err(|e| e.to_string())?;
        match key.delete_value(VALUE) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }


    #[cfg(all(unix, not(target_os = "macos")))]
    fn autostart_file() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|dir| dir.join("autostart").join("jenova.desktop"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn register() -> Result<(), String> {
        let path = autostart_file().ok_or("No config directory")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Jenova\nExec={}\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
            command_line()?
        );
        std::fs::write(path, entry).map_err(|e| e.to_string())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn unregister() -> Result<(), String> {
        match autostart_file() {
            Some(path) if path.exists() => std::fs::remove_file(path).map_err(|e| e.to_string()),
            _ => Ok(()),
        }
    }


    #[cfg(target_os = "macos")]
    pub fn register() -> Result<(), String> {
        Err("Starting with login isn't supported on macOS yet".into())
    }

    #[cfg(target_os = "macos")]
    pub fn unregister() -> Result<(), String> {
        Ok(())
    }

}
