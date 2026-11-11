//! The part of Jenova that keeps working whether the window is open, hidden
//! in the tray, or never shown (started with Windows).
//!
//! It owns the watch thread, follows the settings file, and decides what to
//! watch. The window only edits settings and shows what's going on.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::python_manager::SubliminalCommand;
use crate::scan_history::ScanHistory;
use crate::settings::Settings;
use crate::watch::{WatchHandle, WatchTarget};
use crate::{info, warn};

/// How often the settings file is checked for outside changes.
const POLL: Duration = Duration::from_secs(2);
/// How often to look for Subliminal again while it's missing.
const PROBE_EVERY: Duration = Duration::from_secs(60);

enum EngineMsg {
    SettingsChanged,
    Command(Option<SubliminalCommand>),
    Stop,
}

pub struct Engine {
    pub watch: WatchHandle,
    tx: Sender<EngineMsg>,
    threads: Mutex<Vec<thread::JoinHandle<()>>>,
}

impl Engine {
    pub fn start(scan_history: Arc<Mutex<ScanHistory>>) -> Arc<Self> {
        let (watch, watch_thread) = WatchHandle::start(scan_history);
        let (tx, rx) = mpsc::channel();
        let loop_watch = watch.clone();
        let engine_thread = thread::Builder::new()
            .name("engine".into())
            .spawn(move || run(loop_watch, rx))
            .expect("spawn engine thread");
        Arc::new(Self {
            watch,
            tx,
            threads: Mutex::new(vec![engine_thread, watch_thread]),
        })
    }

    /// The window saved new settings.
    pub fn settings_changed(&self) {
        let _ = self.tx.send(EngineMsg::SettingsChanged);
    }

    /// Subliminal was found (or installed).
    pub fn set_command(&self, command: Option<SubliminalCommand>) {
        let _ = self.tx.send(EngineMsg::Command(command));
    }

    /// Stop downloads and threads (waits a few seconds at most).
    pub fn shutdown(&self) {
        let _ = self.tx.send(EngineMsg::Stop);
        self.watch.stop();
        let threads: Vec<_> = self.threads.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            for handle in threads {
                let _ = handle.join();
            }
            let _ = done_tx.send(());
        });
        let _ = done_rx.recv_timeout(Duration::from_secs(5));
    }
}

fn settings_modified() -> Option<SystemTime> {
    Settings::get_path()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
}

fn run(watch: WatchHandle, rx: Receiver<EngineMsg>) {
    let mut settings = Settings::load();
    let mut mtime = settings_modified();
    let mut command: Option<SubliminalCommand> = None;
    // The window's startup check usually supplies the command; probe
    // ourselves only if it hasn't after a while.
    let mut last_probe = Instant::now();
    let probing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (probe_tx, probe_rx) = mpsc::channel::<Option<SubliminalCommand>>();

    loop {
        let mut reload = false;
        match rx.recv_timeout(POLL) {
            Ok(EngineMsg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(EngineMsg::SettingsChanged) => reload = true,
            Ok(EngineMsg::Command(found)) => {
                if found.is_some() != command.is_some() {
                    info!("Subliminal {}", if found.is_some() { "ready" } else { "missing" });
                }
                command = found;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if let Ok(found) = probe_rx.try_recv() {
            if command.is_none() && found.is_some() {
                info!("Subliminal ready");
                command = found;
            }
        }
        let now_mtime = settings_modified();
        if reload || now_mtime != mtime {
            mtime = now_mtime;
            settings = Settings::load();
        }
        if command.is_none()
            && last_probe.elapsed() >= PROBE_EVERY
            && !probing.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            last_probe = Instant::now();
            let probing = Arc::clone(&probing);
            let probe_tx = probe_tx.clone();
            thread::spawn(move || {
                let found = crate::app::probe_startup_dependencies().subliminal_command;
                if found.is_none() {
                    warn!("Subliminal not found; open Jenova to install it");
                }
                let _ = probe_tx.send(found);
                probing.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }

        crate::notifications::ENABLED.store(settings.notify_desktop, std::sync::atomic::Ordering::Relaxed);
        watch.update_runtime(settings.clone(), command.clone());
        let target = if command.is_some() {
            WatchTarget::from_settings(&settings)
        } else {
            WatchTarget::default()
        };
        watch.configure(target);
    }
}
