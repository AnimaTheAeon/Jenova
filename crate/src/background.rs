//! Background mode: run the folder watcher without a window, started with
//! the user's login.
//!
//! `jenova --background` runs headless. Only one background copy runs at a
//! time (it holds `jenova_watcher.lock`). While the Jenova window is open
//! (it holds `jenova_gui.lock`) the background copy pauses so folders aren't
//! watched twice, and it resumes when the window closes. It re-reads the
//! settings file, so changes made in the window apply to it, and it exits
//! on its own once "Run in background" is switched off.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::scan_history::ScanHistory;
use crate::settings::Settings;
use crate::watch::{WatchService, WatchTarget};
use crate::{info, warn};

pub const BACKGROUND_ARG: &str = "--background";
const GUI_LOCK: &str = "jenova_gui.lock";
const WATCHER_LOCK: &str = "jenova_watcher.lock";
const POLL: Duration = Duration::from_secs(2);
const DEPENDENCY_RETRY: Duration = Duration::from_secs(60);

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

/// Held by the window for its whole lifetime. `wait` keeps trying for a
/// while (used right after an update, while the old window closes).
pub fn hold_gui_lock(wait: Duration) -> Option<File> {
    let file = open_lock(GUI_LOCK)?;
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

/// Whether a background watcher is running.
pub fn background_running() -> bool {
    lock_held(WATCHER_LOCK)
}

fn gui_running() -> bool {
    lock_held(GUI_LOCK)
}

/// Start a detached background watcher (it exits at once if one is running).
pub fn spawn_background() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(exe);
    command
        .arg(BACKGROUND_ARG)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Stop every background Jenova process, whichever folder it runs from.
/// Blocks until they're gone (or a few seconds pass).
pub fn stop_all_background() {
    let me = std::process::id();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = format!(
            "Get-CimInstance Win32_Process -Filter \"Name LIKE 'jenova%.exe'\" | \
             Where-Object {{ $_.CommandLine -like '*{BACKGROUND_ARG}*' -and $_.ProcessId -ne {me} }} | \
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
            warn!("Could not stop background watchers: {error}");
        }
    }
    #[cfg(unix)]
    {
        if let Ok(entries) = std::fs::read_dir("/proc") {
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
                if is_jenova && args.iter().any(|a| a == BACKGROUND_ARG) {
                    let _ = std::process::Command::new("kill")
                        .arg(pid.to_string())
                        .status();
                }
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while background_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// After switching off: wait for the background copy to exit, then delete
/// its lock file so nothing is left behind.
pub fn cleanup_after_stop() {
    std::thread::spawn(|| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while background_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
        }
        if !background_running() {
            let _ = std::fs::remove_file(state_dir().join(WATCHER_LOCK));
        }
    });
}

fn settings_modified() -> Option<SystemTime> {
    Settings::get_path()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
}

fn watch_target(settings: &Settings) -> WatchTarget {
    if !settings.watch_enabled {
        return WatchTarget::default();
    }
    WatchTarget {
        folders: settings
            .watch_folders
            .iter()
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .collect(),
        polling: settings.watch_polling,
        poll_interval_secs: settings.watch_poll_interval_seconds,
    }
}

/// Entry point for `jenova --background`. Returns when the mode is switched off.
pub fn run() {
    let Some(lock) = open_lock(WATCHER_LOCK) else {
        warn!("Background: cannot open lock file; exiting");
        return;
    };
    if lock.try_lock().is_err() {
        info!("Background: another background watcher is already running; exiting");
        return;
    }
    info!("Background watcher started");

    let history = Arc::new(Mutex::new(ScanHistory::load()));
    let mut service = WatchService::new(Arc::clone(&history));
    let mut settings = Settings::load();
    let mut settings_mtime = settings_modified();
    let mut command = None;
    let mut last_probe: Option<Instant> = None;
    let mut paused_for_gui = false;
    crate::notifications::IN_APP_ENABLED.store(false, std::sync::atomic::Ordering::Relaxed);

    loop {
        // Pick up changes made in the window.
        let mtime = settings_modified();
        if mtime != settings_mtime {
            settings_mtime = mtime;
            settings = Settings::load();
            info!("Background: settings reloaded");
        }
        if !settings.background_service {
            info!("Background mode switched off; exiting");
            break;
        }
        crate::notifications::DESKTOP_ENABLED.store(
            settings.notify_desktop,
            std::sync::atomic::Ordering::Relaxed,
        );

        if command.is_none() && last_probe.is_none_or(|t| t.elapsed() >= DEPENDENCY_RETRY) {
            last_probe = Some(Instant::now());
            command = crate::app::probe_startup_dependencies().subliminal_command;
            if command.is_none() {
                warn!(
                    "Background: Subliminal not found; open Jenova to install it. Retrying in 60s"
                );
            }
        }

        let gui = gui_running();
        if gui != paused_for_gui {
            paused_for_gui = gui;
            info!(
                "Background: {}",
                if gui {
                    "Jenova window opened, pausing"
                } else {
                    "Jenova window closed, resuming"
                }
            );
        }
        let target = if gui || command.is_none() {
            WatchTarget::default()
        } else {
            watch_target(&settings)
        };
        service.update_runtime(settings.clone(), command.clone());
        service.apply(target, settings.watch_catch_up);

        std::thread::sleep(POLL);
    }

    service.shutdown();
    let _ = lock.unlock();
    drop(lock);
    let _ = std::fs::remove_file(state_dir().join(WATCHER_LOCK));
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
        Ok(format!("\"{}\" {}", exe.display(), super::BACKGROUND_ARG))
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

    #[cfg(windows)]
    pub fn is_registered() -> bool {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
        winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_READ)
            .and_then(|key| key.get_value::<String, _>(VALUE))
            .is_ok()
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
            "[Desktop Entry]\nType=Application\nName=Jenova (background)\nExec={}\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
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

    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn is_registered() -> bool {
        autostart_file().is_some_and(|path| path.exists())
    }

    #[cfg(target_os = "macos")]
    pub fn register() -> Result<(), String> {
        Err("Starting with login isn't supported on macOS yet".into())
    }

    #[cfg(target_os = "macos")]
    pub fn unregister() -> Result<(), String> {
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn is_registered() -> bool {
        false
    }
}
