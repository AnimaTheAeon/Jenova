//! Folder watching: detect new videos and download subtitles automatically.
//!
//! A single background thread owns the filesystem watcher. New or changed
//! video files become *candidates*; once a candidate has stopped changing for
//! the configured settle time (and, on Windows, no other process holds it open
//! for writing) it is checked for missing subtitles and queued. Queued videos
//! run through the same Subliminal batch runner the manual scan uses.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::app::{batch_config_for, run_download_batch, BatchParams};
use crate::data_structures::{DownloadJob, DownloadJobs, JobStatus};
use crate::helper_functions::Utils;
use crate::python_manager::SubliminalCommand;
use crate::scan_history::ScanHistory;
use crate::settings::Settings;
use crate::subtitle_utils::SubtitleUtils;
use crate::{debug, info, warn};

/// Max lines kept in the activity log.
const MAX_LOG_LINES: usize = 300;
/// Max finished watch jobs kept in the table.
const MAX_WATCH_JOBS: usize = 500;
/// How often candidates are re-checked.
const TICK: Duration = Duration::from_secs(1);

/// The part of the configuration that requires rebuilding the watcher.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct WatchTarget {
    pub folders: Vec<PathBuf>,
    pub polling: bool,
    pub poll_interval_secs: u64,
}

impl WatchTarget {
    pub fn is_active(&self) -> bool {
        !self.folders.is_empty()
    }
}

enum Msg {
    Configure(WatchTarget, bool),
    CatchUp,
    Fs(notify::Result<notify::Event>),
    Found(Vec<PathBuf>, bool),
    Stop,
}

/// Per-folder library counts shown in the UI.
#[derive(Clone, Copy, Default, Debug)]
pub struct FolderStats {
    pub videos: usize,
    pub with_subs: usize,
}

/// Values the UI keeps up to date for the watcher.
#[derive(Default)]
struct Runtime {
    settings: Option<Settings>,
    subliminal_command: Option<SubliminalCommand>,
}

/// State shared between the UI and the watch thread.
pub struct WatchShared {
    runtime: Mutex<Runtime>,
    pub jobs: DownloadJobs,
    log: Mutex<VecDeque<String>>,
    pub watched_folders: AtomicUsize,
    pub settling: AtomicUsize,
    pub queued: AtomicUsize,
    pub batch_running: AtomicBool,
    pub catching_up: AtomicBool,
    pub errors: Mutex<Vec<String>>,
    pub folder_stats: Mutex<HashMap<PathBuf, FolderStats>>,
    stats_running: AtomicBool,
    batch_cancel: Mutex<Arc<AtomicBool>>,
    repaint: Mutex<Option<eframe::egui::Context>>,
    scan_history: Arc<Mutex<ScanHistory>>,
}

impl WatchShared {
    fn push_log(&self, line: impl Into<String>) {
        let line = line.into();
        info!("[watch] {}", line);
        let stamp = chrono::Local::now().format("%H:%M:%S");
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        log.push_back(format!("{stamp}  {line}"));
        while log.len() > MAX_LOG_LINES {
            log.pop_front();
        }
        drop(log);
        self.repaint();
    }

    fn repaint(&self) {
        if let Some(ctx) = self
            .repaint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            ctx.request_repaint();
        }
    }

    pub fn log_lines(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    pub fn stats_for(&self, folder: &Path) -> Option<FolderStats> {
        self.folder_stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(folder)
            .copied()
    }

    pub fn jobs_snapshot(&self) -> Vec<DownloadJob> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Handle owned by the UI.
pub struct WatchService {
    pub shared: Arc<WatchShared>,
    tx: Sender<Msg>,
    handle: Option<thread::JoinHandle<()>>,
    applied: Option<WatchTarget>,
}

impl WatchService {
    pub fn new(scan_history: Arc<Mutex<ScanHistory>>) -> Self {
        let shared = Arc::new(WatchShared {
            runtime: Mutex::new(Runtime::default()),
            jobs: Arc::new(Mutex::new(Vec::new())),
            log: Mutex::new(VecDeque::new()),
            watched_folders: AtomicUsize::new(0),
            settling: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            batch_running: AtomicBool::new(false),
            catching_up: AtomicBool::new(false),
            errors: Mutex::new(Vec::new()),
            folder_stats: Mutex::new(HashMap::new()),
            stats_running: AtomicBool::new(false),
            batch_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
            repaint: Mutex::new(None),
            scan_history,
        });
        let (tx, rx) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let thread_tx = tx.clone();
        let handle = thread::Builder::new()
            .name("watch".into())
            .spawn(move || WatchLoop::new(thread_shared, thread_tx).run(rx))
            .ok();
        Self {
            shared,
            tx,
            handle,
            applied: None,
        }
    }

    /// Give the watcher a context so it can wake the UI.
    pub fn set_repaint_context(&self, ctx: &eframe::egui::Context) {
        let mut slot = self
            .shared
            .repaint
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(ctx.clone());
        }
    }

    /// Push the latest settings and Subliminal command to the watcher.
    pub fn update_runtime(&self, settings: Settings, command: Option<SubliminalCommand>) {
        let mut runtime = self
            .shared
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        runtime.settings = Some(settings);
        runtime.subliminal_command = command;
    }

    /// Apply a watch target if it changed. `catch_up` scans existing files.
    pub fn apply(&mut self, target: WatchTarget, catch_up: bool) {
        if self.applied.as_ref() == Some(&target) {
            return;
        }
        self.applied = Some(target.clone());
        let _ = self.tx.send(Msg::Configure(target, catch_up));
    }

    /// Scan watched folders now for videos missing subtitles.
    pub fn catch_up_now(&self) {
        let _ = self.tx.send(Msg::CatchUp);
    }

    /// Cancel the running watch batch (queued files are dropped too).
    pub fn cancel_batch(&self) {
        self.shared
            .batch_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .store(true, Ordering::SeqCst);
    }

    pub fn clear_finished(&self) {
        if self.shared.batch_running.load(Ordering::SeqCst) {
            return;
        }
        self.shared
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|job| matches!(job.status, JobStatus::Pending | JobStatus::Running));
    }

    pub fn shutdown(&mut self) {
        self.cancel_batch();
        let _ = self.tx.send(Msg::Stop);
        if let Some(handle) = self.handle.take() {
            let (done_tx, done_rx) = mpsc::channel();
            thread::spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
            let _ = done_rx.recv_timeout(Duration::from_secs(3));
        }
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    size: u64,
    modified: Option<SystemTime>,
    last_change: Instant,
    /// From a catch-up scan rather than a live event.
    catch_up: bool,
}

/// File identity used to avoid processing the same file twice per session.
type Fingerprint = (u64, Option<SystemTime>);

struct WatchLoop {
    shared: Arc<WatchShared>,
    tx: Sender<Msg>,
    watcher: Option<Box<dyn Watcher + Send>>,
    target: WatchTarget,
    candidates: HashMap<PathBuf, Candidate>,
    queue: VecDeque<PathBuf>,
    queued_set: HashSet<PathBuf>,
    processed: HashMap<PathBuf, Fingerprint>,
    batch_handle: Option<thread::JoinHandle<()>>,
    batch_size: usize,
    waiting_reason_logged: Option<&'static str>,
}

impl WatchLoop {
    fn new(shared: Arc<WatchShared>, tx: Sender<Msg>) -> Self {
        Self {
            shared,
            tx,
            watcher: None,
            target: WatchTarget::default(),
            candidates: HashMap::new(),
            queue: VecDeque::new(),
            queued_set: HashSet::new(),
            processed: HashMap::new(),
            batch_handle: None,
            batch_size: 0,
            waiting_reason_logged: None,
        }
    }

    fn settings(&self) -> Option<Settings> {
        self.shared
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .settings
            .clone()
    }

    fn run(mut self, rx: Receiver<Msg>) {
        let mut last_tick = Instant::now();
        loop {
            match rx.recv_timeout(TICK) {
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Msg::Configure(target, catch_up)) => self.configure(target, catch_up),
                Ok(Msg::CatchUp) => self.start_catch_up(),
                Ok(Msg::Fs(event)) => self.on_fs_event(event),
                Ok(Msg::Found(paths, catch_up)) => {
                    if catch_up {
                        self.shared.catching_up.store(false, Ordering::SeqCst);
                    }
                    let count = paths.len();
                    for path in paths {
                        self.add_candidate(path, catch_up);
                    }
                    if catch_up {
                        self.shared
                            .push_log(format!("Catch-up scan found {count} video(s) to check"));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if last_tick.elapsed() >= TICK {
                last_tick = Instant::now();
                self.tick();
            }
        }
        self.watcher = None;
        if let Some(handle) = self.batch_handle.take() {
            let _ = handle.join();
        }
        debug!("Watch thread stopped");
    }

    fn configure(&mut self, target: WatchTarget, catch_up: bool) {
        self.watcher = None;
        self.candidates.clear();
        self.shared
            .errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.shared.watched_folders.store(0, Ordering::SeqCst);
        self.target = target.clone();

        if !target.is_active() {
            self.shared.push_log("Watching stopped");
            return;
        }

        let tx = self.tx.clone();
        let handler = move |event: notify::Result<notify::Event>| {
            let _ = tx.send(Msg::Fs(event));
        };
        let watcher: notify::Result<Box<dyn Watcher + Send>> = if target.polling {
            let config = notify::Config::default()
                .with_poll_interval(Duration::from_secs(target.poll_interval_secs.max(5)));
            notify::PollWatcher::new(handler, config)
                .map(|w| Box::new(w) as Box<dyn Watcher + Send>)
        } else {
            notify::recommended_watcher(handler).map(|w| Box::new(w) as Box<dyn Watcher + Send>)
        };
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                let message = format!("Could not start the folder watcher: {error}");
                self.shared.push_log(&message);
                self.shared
                    .errors
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(message);
                return;
            }
        };

        let mut ok = 0;
        for folder in &target.folders {
            match watcher.watch(folder, RecursiveMode::Recursive) {
                Ok(()) => {
                    ok += 1;
                    self.shared
                        .push_log(format!("Watching {}", folder.display()));
                }
                Err(error) => {
                    let message = format!("Cannot watch {}: {error}", folder.display());
                    self.shared.push_log(&message);
                    self.shared
                        .errors
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(message);
                }
            }
        }
        self.shared.watched_folders.store(ok, Ordering::SeqCst);
        if target.polling {
            self.shared.push_log(format!(
                "Polling mode: checking every {}s",
                target.poll_interval_secs.max(5)
            ));
        }
        self.watcher = Some(watcher);
        self.refresh_stats();

        if catch_up {
            self.start_catch_up();
        }
    }

    fn start_catch_up(&mut self) {
        if !self.target.is_active() || self.shared.catching_up.load(Ordering::SeqCst) {
            return;
        }
        let ignore_extras = self
            .settings()
            .map(|s| s.ignore_local_extras)
            .unwrap_or(true);
        let folders = self.target.folders.clone();
        let tx = self.tx.clone();
        self.shared.catching_up.store(true, Ordering::SeqCst);
        self.shared.push_log("Catch-up scan started");
        self.refresh_stats();
        thread::spawn(move || {
            let mut found = Vec::new();
            for folder in &folders {
                found.extend(walk_videos(folder, ignore_extras));
            }
            let _ = tx.send(Msg::Found(found, true));
        });
    }

    /// Recount videos and subtitle coverage for every watched folder.
    fn refresh_stats(&mut self) {
        if !self.target.is_active() || self.shared.stats_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(settings) = self.settings() else {
            self.shared.stats_running.store(false, Ordering::SeqCst);
            return;
        };
        let folders = self.target.folders.clone();
        let shared = Arc::clone(&self.shared);
        thread::spawn(move || {
            for folder in folders {
                let videos = walk_videos(&folder, settings.ignore_local_extras);
                let mut listings: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
                let mut with_subs = 0;
                for video in &videos {
                    let Some(parent) = video.parent() else {
                        continue;
                    };
                    let listing = listings
                        .entry(parent.to_path_buf())
                        .or_insert_with(|| SubtitleUtils::list_subtitle_files_in_folder(parent));
                    if !SubtitleUtils::video_missing_subtitle_in_listing(
                        video,
                        &settings.selected_languages,
                        settings.exclude_hearing_impaired,
                        listing,
                    ) {
                        with_subs += 1;
                    }
                }
                shared
                    .folder_stats
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        folder,
                        FolderStats {
                            videos: videos.len(),
                            with_subs,
                        },
                    );
                shared.repaint();
            }
            shared.stats_running.store(false, Ordering::SeqCst);
        });
    }

    fn on_fs_event(&mut self, event: notify::Result<notify::Event>) {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                warn!("Watcher error: {error}");
                return;
            }
        };
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
        ) {
            return;
        }
        let ignore_extras = self
            .settings()
            .map(|s| s.ignore_local_extras)
            .unwrap_or(true);
        for path in event.paths {
            if path.is_dir() {
                // A folder moved or copied in: its files may produce no events.
                if ignore_extras && in_extras_folder(&path) {
                    continue;
                }
                let tx = self.tx.clone();
                thread::spawn(move || {
                    let found = walk_videos(&path, ignore_extras);
                    if !found.is_empty() {
                        let _ = tx.send(Msg::Found(found, false));
                    }
                });
            } else if Utils::is_video_file(&path) {
                self.add_candidate(path, false);
            }
        }
    }

    fn add_candidate(&mut self, path: PathBuf, catch_up: bool) {
        if self.queued_set.contains(&path) {
            return;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            return;
        };
        let size = meta.len();
        let modified = meta.modified().ok();
        if self.processed.get(&path) == Some(&(size, modified)) {
            return;
        }
        let now = Instant::now();
        if let Some(existing) = self.candidates.get_mut(&path) {
            if !catch_up {
                existing.last_change = now;
                existing.catch_up = false;
            }
            return;
        }
        if self.candidates.is_empty() && !catch_up {
            debug!("New video detected: {}", path.display());
        }
        self.candidates.insert(
            path,
            Candidate {
                size,
                modified,
                last_change: now,
                catch_up,
            },
        );
    }

    fn tick(&mut self) {
        let settings = self.settings();
        if let Some(settings) = settings.as_ref() {
            self.evaluate_candidates(settings);
        }
        self.reap_batch();
        if let Some(settings) = settings {
            self.maybe_start_batch(settings);
        }
        self.shared
            .settling
            .store(self.candidates.len(), Ordering::SeqCst);
        self.shared.queued.store(self.queue.len(), Ordering::SeqCst);
    }

    fn evaluate_candidates(&mut self, settings: &Settings) {
        if self.candidates.is_empty() {
            return;
        }
        let settle = Duration::from_secs(settings.watch_settle_seconds);
        let now = Instant::now();
        let mut ready = Vec::new();
        let mut gone = Vec::new();

        for (path, cand) in self.candidates.iter_mut() {
            let Ok(meta) = std::fs::metadata(path) else {
                gone.push(path.clone());
                continue;
            };
            let size = meta.len();
            let modified = meta.modified().ok();
            if size != cand.size || modified != cand.modified {
                cand.size = size;
                cand.modified = modified;
                cand.last_change = now;
                continue;
            }
            // Existing files from a catch-up scan only need to be old enough.
            let settled = if cand.catch_up {
                modified
                    .and_then(|m| SystemTime::now().duration_since(m).ok())
                    .map(|age| age >= settle)
                    .unwrap_or(true)
                    || now.duration_since(cand.last_change) >= settle
            } else {
                now.duration_since(cand.last_change) >= settle
            };
            if !settled || size == 0 {
                continue;
            }
            if !file_is_free(path) {
                // Still held open for writing (download or copy in progress).
                cand.last_change = now;
                continue;
            }
            ready.push(path.clone());
        }

        for path in gone {
            self.candidates.remove(&path);
        }

        for path in ready {
            let Some(cand) = self.candidates.remove(&path) else {
                continue;
            };
            self.processed
                .insert(path.clone(), (cand.size, cand.modified));
            self.consider(path, settings, cand.catch_up);
        }
    }

    /// Decide whether a settled video needs subtitles, and queue it if so.
    fn consider(&mut self, path: PathBuf, settings: &Settings, catch_up: bool) {
        let name = Utils::get_file_name(&path);
        if settings.ignore_local_extras && in_extras_folder(&path) {
            return;
        }
        if settings.selected_languages.is_empty() {
            return;
        }
        let missing = SubtitleUtils::video_missing_subtitle(
            &path,
            &settings.selected_languages,
            settings.exclude_hearing_impaired,
        );
        if !missing {
            if !catch_up {
                self.shared
                    .push_log(format!("{name}: already has subtitles, skipping"));
            }
            return;
        }
        if settings.skip_scanned_media {
            let skip = self
                .shared
                .scan_history
                .lock()
                .map(|history| {
                    history.should_skip(
                        &path,
                        &settings.selected_languages,
                        settings.exclude_hearing_impaired,
                    )
                })
                .unwrap_or(false);
            if skip {
                return;
            }
        }
        if !catch_up {
            self.shared.push_log(format!("{name}: queued"));
        }
        self.queued_set.insert(path.clone());
        self.queue.push_back(path);
    }

    fn reap_batch(&mut self) {
        if self
            .batch_handle
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            if let Some(handle) = self.batch_handle.take() {
                let _ = handle.join();
            }
            self.shared.batch_running.store(false, Ordering::SeqCst);
            let jobs = self.shared.jobs_snapshot();
            let mut ok = 0;
            let mut failed = 0;
            for job in jobs.iter().rev().take(self.batch_size) {
                match job.status {
                    JobStatus::Success | JobStatus::EmbeddedExists(_) => ok += 1,
                    JobStatus::Failed(_) => failed += 1,
                    _ => {}
                }
            }
            let cancelled = self
                .shared
                .batch_cancel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .load(Ordering::SeqCst);
            if cancelled {
                self.queue.clear();
                self.queued_set.clear();
                self.shared.push_log("Watch batch cancelled");
            } else {
                self.shared
                    .push_log(format!("Batch finished: {ok} succeeded, {failed} failed"));
            }
            self.refresh_stats();
        }
    }

    fn maybe_start_batch(&mut self, settings: Settings) {
        if self.batch_handle.is_some() || self.queue.is_empty() {
            return;
        }
        let command = self
            .shared
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subliminal_command
            .clone();
        let Some(command) = command else {
            self.log_waiting("Waiting for Subliminal to be ready before downloading");
            return;
        };
        let config_content = match batch_config_for(&settings) {
            Ok(content) => content,
            Err(error) => {
                self.log_waiting("Subliminal config error; see log");
                warn!("{error}");
                return;
            }
        };
        self.waiting_reason_logged = None;

        let paths: Vec<PathBuf> = self.queue.drain(..).collect();
        self.batch_size = paths.len();
        self.queued_set.clear();
        {
            let mut jobs = self.shared.jobs.lock().unwrap_or_else(|e| e.into_inner());
            // Trim old finished jobs (no batch is running here).
            if jobs.len() + paths.len() > MAX_WATCH_JOBS {
                let excess = jobs.len() + paths.len() - MAX_WATCH_JOBS;
                let cut = excess.min(jobs.len());
                jobs.drain(..cut);
            }
            for video_path in &paths {
                jobs.push(DownloadJob {
                    video_path: video_path.clone(),
                    status: JobStatus::Pending,
                    output: "Queued".to_string(),
                    subtitle_paths: Vec::new(),
                });
            }
        }
        self.shared.push_log(format!(
            "Downloading subtitles for {} video(s)",
            paths.len()
        ));

        let cancel = Arc::new(AtomicBool::new(false));
        *self
            .shared
            .batch_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Arc::clone(&cancel);
        let params = BatchParams {
            jobs: Arc::clone(&self.shared.jobs),
            cancel_flag: cancel,
            max_concurrent: settings.concurrent_downloads.max(1),
            langs: settings.selected_languages.clone(),
            providers: settings.providers.clone(),
            refiners: settings.refiners.clone(),
            opensubtitles_max_pages: settings.opensubtitles_max_pages,
            subliminal_command: command,
            config_content,
            force_download: settings.force_download,
            overwrite_existing: settings.overwrite_existing,
            minimum_score: settings.minimum_score,
            exclude_hearing_impaired: settings.exclude_hearing_impaired,
            scan_history: Arc::clone(&self.shared.scan_history),
        };
        self.shared.batch_running.store(true, Ordering::SeqCst);
        let shared = Arc::clone(&self.shared);
        self.batch_handle = Some(thread::spawn(move || {
            run_download_batch(params);
            shared.repaint();
        }));
    }

    fn log_waiting(&mut self, reason: &'static str) {
        if self.waiting_reason_logged != Some(reason) {
            self.waiting_reason_logged = Some(reason);
            self.shared.push_log(reason);
        }
    }
}

/// Recursively list video files under `root`.
fn walk_videos(root: &Path, ignore_extras: bool) -> Vec<PathBuf> {
    let walker = jwalk::WalkDir::new(root)
        .skip_hidden(false)
        .process_read_dir(move |_depth, _path, _state, children| {
            if !ignore_extras {
                return;
            }
            for child in children.iter_mut() {
                let Ok(entry) = child else { continue };
                if entry.file_type.is_dir()
                    && entry
                        .file_name()
                        .to_str()
                        .is_some_and(Utils::is_plex_extras_folder)
                {
                    entry.read_children_path = None;
                }
            }
        });
    walker
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.path())
        .filter(|path| Utils::is_video_file(path))
        .collect()
}

/// Whether any parent folder is a Plex local-extras folder.
fn in_extras_folder(path: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .any(Utils::is_plex_extras_folder)
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| path.is_dir() && Utils::is_plex_extras_folder(n))
}

/// Whether no other process has the file open for writing.
#[cfg(windows)]
fn file_is_free(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .is_ok()
}

#[cfg(not(windows))]
fn file_is_free(path: &Path) -> bool {
    std::fs::File::open(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extras_folders_are_detected_in_ancestors() {
        assert!(in_extras_folder(Path::new(
            "/media/Movie (2020)/Featurettes/clip.mkv"
        )));
        assert!(!in_extras_folder(Path::new(
            "/media/Movie (2020)/Movie.mkv"
        )));
    }

    #[test]
    fn watch_target_inactive_without_folders() {
        assert!(!WatchTarget::default().is_active());
        let target = WatchTarget {
            folders: vec![PathBuf::from("/tmp")],
            ..Default::default()
        };
        assert!(target.is_active());
    }

    #[test]
    fn walk_finds_nested_videos_and_skips_extras() {
        let dir = tempfile::tempdir().unwrap();
        let season = dir.path().join("Show/Season 01");
        let extras = dir.path().join("Show/Featurettes");
        std::fs::create_dir_all(&season).unwrap();
        std::fs::create_dir_all(&extras).unwrap();
        std::fs::write(season.join("Show.S01E01.mkv"), b"x").unwrap();
        std::fs::write(season.join("notes.txt"), b"x").unwrap();
        std::fs::write(extras.join("bonus.mkv"), b"x").unwrap();

        let found = walk_videos(dir.path(), true);
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("Show.S01E01.mkv"));
        assert_eq!(walk_videos(dir.path(), false).len(), 2);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// End-to-end: needs `subliminal` on PATH. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn new_video_is_detected_settled_and_processed() {
        let dir = tempfile::tempdir().unwrap();
        let history = Arc::new(Mutex::new(ScanHistory::default()));
        let mut service = WatchService::new(history);
        let settings = Settings {
            watch_settle_seconds: 3,
            providers: vec!["podnapisi".into()],
            refiners: vec![],
            concurrent_downloads: 2,
            skip_scanned_media: false,
            ..Settings::default()
        };
        service.update_runtime(
            settings,
            Some(SubliminalCommand {
                program: PathBuf::from("subliminal"),
                prefix: vec![],
            }),
        );
        // A pre-existing video exercises catch-up.
        std::fs::write(dir.path().join("Old.Movie.2001.mkv"), vec![0u8; 4096]).unwrap();
        service.apply(
            WatchTarget {
                folders: vec![dir.path().to_path_buf()],
                polling: false,
                poll_interval_secs: 60,
            },
            true,
        );
        thread::sleep(Duration::from_millis(500));

        // A new season folder appears, and a file is written slowly into it.
        let season = dir.path().join("Some Show/Season 01");
        std::fs::create_dir_all(&season).unwrap();
        let video = season.join("Some.Show.S01E01.1080p.mkv");
        let started = Instant::now();
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&video).unwrap();
            for _ in 0..6 {
                f.write_all(&[1u8; 1024 * 64]).unwrap();
                f.flush().unwrap();
                thread::sleep(Duration::from_secs(1));
                assert_eq!(
                    service
                        .shared
                        .jobs_snapshot()
                        .iter()
                        .filter(|j| j.video_path == video)
                        .count(),
                    0,
                    "queued while still being written"
                );
            }
        }
        let written = started.elapsed();

        let deadline = Instant::now() + Duration::from_secs(150);
        loop {
            let jobs = service.shared.jobs_snapshot();
            let done: Vec<_> = jobs
                .iter()
                .filter(|j| !matches!(j.status, JobStatus::Pending | JobStatus::Running))
                .collect();
            if done.len() >= 2 {
                for j in &jobs {
                    let status = match &j.status {
                        JobStatus::Failed(m) => format!("Failed({m})"),
                        JobStatus::Success => "Success".into(),
                        _ => "other".into(),
                    };
                    println!("JOB {} -> {}", j.video_path.display(), status);
                }
                break;
            }
            assert!(Instant::now() < deadline, "timed out; jobs={}", jobs.len());
            thread::sleep(Duration::from_millis(500));
        }
        for line in service.shared.log_lines() {
            println!("LOG {line}");
        }
        println!("file written over {:?}", written);
        service.shutdown();
    }
}
