//! Folder watching and the download queue.
//!
//! One thread owns the filesystem watcher and the queue. New or changed
//! videos become *candidates*; once a candidate has stopped changing for the
//! settle time (and, on Windows, nothing else holds it open for writing) it
//! is checked for missing subtitles and queued. Up to "concurrent downloads"
//! Subliminal runs go at once, and new arrivals jump ahead of older work.
//!
//! Videos with no subtitles online yet are tried again on a schedule (see
//! `retry.rs`), and the full check of existing files runs at most once a day,
//! so restarts and drives reconnecting don't re-search the whole library.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::data_structures::{DownloadJob, DownloadJobs, JobStatus};
use crate::downloader::{process_video, JobResult, JobSetup};
use crate::helper_functions::Utils;
use crate::python_manager::SubliminalCommand;
use crate::retry::{self, RetryBook};
use crate::scan_history::ScanHistory;
use crate::settings::Settings;
use crate::subtitle_utils::SubtitleUtils;
use crate::{debug, info, warn};

/// Max lines kept in the activity log.
const MAX_LOG_LINES: usize = 300;
/// Max jobs kept in the Activity list (finished ones are trimmed first).
const MAX_JOBS: usize = 500;
/// How often candidates and workers are checked.
const TICK: Duration = Duration::from_millis(500);
/// How often folders are checked for being reachable.
const FOLDER_CHECK_EVERY: Duration = Duration::from_secs(30);
/// How often the retry list is checked for videos that are due.
const RETRY_CHECK_EVERY: Duration = Duration::from_secs(10 * 60);
/// Minimum time between subtitle-coverage recounts.
const STATS_EVERY: Duration = Duration::from_secs(10 * 60);

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

    /// The target for these settings (all folders, reachable or not).
    pub fn from_settings(settings: &Settings) -> Self {
        if !settings.watch_enabled {
            return Self::default();
        }
        Self {
            folders: settings.watch_folders.iter().map(PathBuf::from).collect(),
            polling: settings.watch_polling,
            poll_interval_secs: settings.watch_poll_interval_seconds,
        }
    }
}

/// Where a video came from; decides its place in the queue.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Origin {
    /// Just arrived in a watched folder: goes to the front.
    New,
    /// Found by checking existing files (daily, "Check existing files", or
    /// "Scan a folder once").
    Existing,
    /// Tried before with nothing found; its retry time has come.
    Retry,
}

enum Msg {
    Configure(WatchTarget),
    CheckExisting { folders: Option<Vec<PathBuf>> },
    Fs(notify::Result<notify::Event>),
    Found(Vec<PathBuf>, Origin),
    FolderStatus(Vec<(PathBuf, bool)>),
    Cancel,
    Stop,
}

/// Per-folder library counts shown in the UI.
#[derive(Clone, Copy, Default, Debug)]
pub struct FolderStats {
    pub videos: usize,
    pub with_subs: usize,
}

#[derive(Default)]
struct Runtime {
    settings: Option<Settings>,
    command: Option<SubliminalCommand>,
    /// Bumped whenever either changes, so the job setup is rebuilt.
    generation: u64,
}

/// State shared between the UI and the watch thread.
pub struct WatchShared {
    runtime: Mutex<Runtime>,
    pub jobs: DownloadJobs,
    log: Mutex<VecDeque<String>>,
    pub watched_folders: AtomicUsize,
    pub settling: AtomicUsize,
    pub queued: AtomicUsize,
    pub running: AtomicUsize,
    pub retry_waiting: AtomicUsize,
    pub catching_up: AtomicBool,
    folder_online: Mutex<HashMap<PathBuf, bool>>,
    folder_stats: Mutex<HashMap<PathBuf, FolderStats>>,
    stats_running: AtomicBool,
    found_today: Mutex<(chrono::NaiveDate, usize)>,
    next_id: AtomicU64,
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
        if let Some(ctx) = self.repaint.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            ctx.request_repaint();
        }
    }

    pub fn log_lines(&self) -> Vec<String> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
    }

    pub fn stats_for(&self, folder: &Path) -> Option<FolderStats> {
        self.folder_stats.lock().unwrap_or_else(|e| e.into_inner()).get(folder).copied()
    }

    /// Last known reachability of a watched folder (`None` = not checked yet).
    pub fn folder_online(&self, folder: &Path) -> Option<bool> {
        self.folder_online.lock().unwrap_or_else(|e| e.into_inner()).get(folder).copied()
    }

    pub fn jobs_snapshot(&self) -> Vec<DownloadJob> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Downloads waiting or running.
    pub fn busy(&self) -> bool {
        self.running.load(Ordering::SeqCst) > 0 || self.queued.load(Ordering::SeqCst) > 0
    }

    /// Videos that got subtitles today.
    pub fn found_today(&self) -> usize {
        let today = chrono::Local::now().date_naive();
        let slot = self.found_today.lock().unwrap_or_else(|e| e.into_inner());
        if slot.0 == today {
            slot.1
        } else {
            0
        }
    }

    fn count_found(&self) {
        let today = chrono::Local::now().date_naive();
        let mut slot = self.found_today.lock().unwrap_or_else(|e| e.into_inner());
        if slot.0 != today {
            *slot = (today, 0);
        }
        slot.1 += 1;
    }

    fn update_job(&self, id: u64, f: impl FnOnce(&mut DownloadJob)) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = jobs.iter_mut().rev().find(|job| job.id == id) {
            f(job);
        }
    }
}

/// Handle used by the UI and the engine. Cheap to clone.
#[derive(Clone)]
pub struct WatchHandle {
    pub shared: Arc<WatchShared>,
    tx: Sender<Msg>,
}

impl WatchHandle {
    /// Start the watch thread.
    pub fn start(scan_history: Arc<Mutex<ScanHistory>>) -> (Self, thread::JoinHandle<()>) {
        let shared = Arc::new(WatchShared {
            runtime: Mutex::new(Runtime::default()),
            jobs: Arc::new(Mutex::new(Vec::new())),
            log: Mutex::new(VecDeque::new()),
            watched_folders: AtomicUsize::new(0),
            settling: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            retry_waiting: AtomicUsize::new(0),
            catching_up: AtomicBool::new(false),
            folder_online: Mutex::new(HashMap::new()),
            folder_stats: Mutex::new(HashMap::new()),
            stats_running: AtomicBool::new(false),
            found_today: Mutex::new((chrono::Local::now().date_naive(), 0)),
            next_id: AtomicU64::new(1),
            repaint: Mutex::new(None),
            scan_history,
        });
        let (tx, rx) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let thread_tx = tx.clone();
        let handle = thread::Builder::new()
            .name("watch".into())
            .spawn(move || WatchLoop::new(thread_shared, thread_tx).run(rx))
            .expect("spawn watch thread");
        (Self { shared, tx }, handle)
    }

    /// Give the watcher a context so it can wake the UI.
    pub fn set_repaint_context(&self, ctx: &eframe::egui::Context) {
        let mut slot = self.shared.repaint.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(ctx.clone());
        }
    }

    /// Push the latest settings and Subliminal command to the watcher.
    pub fn update_runtime(&self, settings: Settings, command: Option<SubliminalCommand>) {
        let mut runtime = self.shared.runtime.lock().unwrap_or_else(|e| e.into_inner());
        let changed = runtime.command != command
            || runtime
                .settings
                .as_ref()
                .is_none_or(|old| serde_json::to_string(old).ok() != serde_json::to_string(&settings).ok());
        if changed {
            runtime.settings = Some(settings);
            runtime.command = command;
            runtime.generation += 1;
        }
    }

    /// Watch these folders (no-op if nothing changed).
    pub fn configure(&self, target: WatchTarget) {
        let _ = self.tx.send(Msg::Configure(target));
    }

    /// Check every watched folder for videos missing subtitles now.
    pub fn check_existing_now(&self) {
        let _ = self.tx.send(Msg::CheckExisting { folders: None });
    }

    /// One-off check of any folder (it doesn't have to be watched).
    pub fn scan_folder(&self, folder: PathBuf) {
        let _ = self.tx.send(Msg::CheckExisting { folders: Some(vec![folder]) });
    }

    /// Stop running downloads and drop everything queued.
    pub fn cancel_all(&self) {
        let _ = self.tx.send(Msg::Cancel);
    }

    /// Remove finished entries from the Activity list.
    pub fn clear_finished(&self) {
        self.shared
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|job| job.status.is_active());
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Msg::Cancel);
        let _ = self.tx.send(Msg::Stop);
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    size: u64,
    modified: Option<SystemTime>,
    last_change: Instant,
    origin: Origin,
}

/// File identity used to avoid processing the same file twice per session.
type Fingerprint = (u64, Option<SystemTime>);

struct Worker {
    id: u64,
    path: PathBuf,
    handle: thread::JoinHandle<JobResult>,
}

struct WatchLoop {
    shared: Arc<WatchShared>,
    tx: Sender<Msg>,
    watcher: Option<Box<dyn Watcher + Send>>,
    target: WatchTarget,
    /// Target folders currently being watched.
    watching: HashSet<PathBuf>,
    candidates: HashMap<PathBuf, Candidate>,
    queue: VecDeque<(u64, PathBuf)>,
    queued_set: HashSet<PathBuf>,
    processed: HashMap<PathBuf, Fingerprint>,
    workers: Vec<Worker>,
    cancel: Arc<AtomicBool>,
    setup: Option<(u64, JobSetup)>,
    retry: RetryBook,
    finished_since_stats: bool,
    last_stats: Option<Instant>,
    last_folder_check: Option<Instant>,
    folder_check_running: Arc<AtomicBool>,
    last_retry_check: Instant,
    waiting_reason_logged: Option<&'static str>,
}

impl WatchLoop {
    fn new(shared: Arc<WatchShared>, tx: Sender<Msg>) -> Self {
        let retry = RetryBook::load();
        shared.retry_waiting.store(retry.waiting(), Ordering::SeqCst);
        Self {
            shared,
            tx,
            watcher: None,
            target: WatchTarget::default(),
            watching: HashSet::new(),
            candidates: HashMap::new(),
            queue: VecDeque::new(),
            queued_set: HashSet::new(),
            processed: HashMap::new(),
            workers: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            setup: None,
            retry,
            finished_since_stats: false,
            last_stats: None,
            last_folder_check: None,
            folder_check_running: Arc::new(AtomicBool::new(false)),
            // First retry check a minute after start.
            last_retry_check: Instant::now() - RETRY_CHECK_EVERY + Duration::from_secs(60),
            waiting_reason_logged: None,
        }
    }

    fn settings(&self) -> Option<Settings> {
        self.shared.runtime.lock().unwrap_or_else(|e| e.into_inner()).settings.clone()
    }

    fn run(mut self, rx: Receiver<Msg>) {
        let mut last_tick = Instant::now();
        loop {
            match rx.recv_timeout(TICK) {
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Msg::Configure(target)) => self.configure(target),
                Ok(Msg::CheckExisting { folders }) => self.check_existing(folders),
                Ok(Msg::Fs(event)) => self.on_fs_event(event),
                Ok(Msg::Found(paths, origin)) => self.on_found(paths, origin),
                Ok(Msg::FolderStatus(status)) => self.on_folder_status(status),
                Ok(Msg::Cancel) => self.cancel_all(),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if last_tick.elapsed() >= TICK {
                last_tick = Instant::now();
                self.tick();
            }
        }
        self.cancel_all();
        self.watcher = None;
        for worker in self.workers.drain(..) {
            let _ = worker.handle.join();
        }
        self.retry.save_if_dirty();
        debug!("Watch thread stopped");
    }

    // ---------- watching ----------

    fn configure(&mut self, target: WatchTarget) {
        if target == self.target {
            return;
        }
        self.watcher = None;
        self.watching.clear();
        self.candidates.clear();
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
            notify::PollWatcher::new(handler, config).map(|w| Box::new(w) as Box<dyn Watcher + Send>)
        } else {
            notify::recommended_watcher(handler).map(|w| Box::new(w) as Box<dyn Watcher + Send>)
        };
        match watcher {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(error) => {
                self.shared.push_log(format!("Could not start the folder watcher: {error}"));
                return;
            }
        }
        for folder in target.folders.clone() {
            // Folders known to be offline are picked up when they're back.
            if self.shared.folder_online(&folder) != Some(false) {
                self.watch_folder(&folder);
            }
        }
        if target.polling {
            self.shared
                .push_log(format!("Polling mode: checking every {}s", target.poll_interval_secs.max(5)));
        }
        self.last_folder_check = None;
        self.refresh_stats(true);
    }

    fn watch_folder(&mut self, folder: &Path) -> bool {
        let Some(watcher) = self.watcher.as_mut() else {
            return false;
        };
        match watcher.watch(folder, RecursiveMode::Recursive) {
            Ok(()) => {
                self.watching.insert(folder.to_path_buf());
                self.shared.push_log(format!("Watching {}", folder.display()));
                self.shared.watched_folders.store(self.watching.len(), Ordering::SeqCst);
                true
            }
            Err(error) => {
                debug!("Cannot watch {}: {error}", folder.display());
                false
            }
        }
    }

    /// Check (off this thread) which watched folders are reachable.
    fn check_folders(&mut self) {
        if !self.target.is_active() || self.folder_check_running.swap(true, Ordering::SeqCst) {
            return;
        }
        self.last_folder_check = Some(Instant::now());
        let folders = self.target.folders.clone();
        let tx = self.tx.clone();
        let running = Arc::clone(&self.folder_check_running);
        thread::spawn(move || {
            let status = folders.into_iter().map(|f| {
                let online = f.is_dir();
                (f, online)
            });
            let _ = tx.send(Msg::FolderStatus(status.collect()));
            running.store(false, Ordering::SeqCst);
        });
    }

    fn on_folder_status(&mut self, status: Vec<(PathBuf, bool)>) {
        {
            let mut map = self.shared.folder_online.lock().unwrap_or_else(|e| e.into_inner());
            for (folder, online) in &status {
                map.insert(folder.clone(), *online);
            }
        }
        for (folder, online) in status {
            if !self.target.folders.contains(&folder) {
                continue;
            }
            let watched = self.watching.contains(&folder);
            if online && !watched {
                // Back online (or reachable for the first time).
                self.watch_folder(&folder);
                self.refresh_stats(true);
            } else if !online && watched {
                if let Some(watcher) = self.watcher.as_mut() {
                    let _ = watcher.unwatch(&folder);
                }
                self.watching.remove(&folder);
                self.shared.watched_folders.store(self.watching.len(), Ordering::SeqCst);
                self.shared.push_log(format!("{} is offline", folder.display()));
            }
        }
        self.shared.repaint();
    }

    fn on_fs_event(&mut self, event: notify::Result<notify::Event>) {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                warn!("Watcher error: {error}");
                return;
            }
        };
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any) {
            return;
        }
        let ignore_extras = self.settings().map(|s| s.ignore_local_extras).unwrap_or(true);
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
                        let _ = tx.send(Msg::Found(found, Origin::New));
                    }
                });
            } else if Utils::is_video_file(&path) {
                self.add_candidate(path, Origin::New);
            }
        }
    }

    // ---------- finding videos ----------

    /// Walk folders (all watched ones when `None`) for existing videos.
    fn check_existing(&mut self, folders: Option<Vec<PathBuf>>) {
        let one_off = folders.is_some();
        let folders = match folders {
            Some(folders) => folders,
            None => {
                if !self.target.is_active() {
                    return;
                }
                self.watching.iter().cloned().collect()
            }
        };
        if folders.is_empty() || self.shared.catching_up.swap(true, Ordering::SeqCst) {
            return;
        }
        if !one_off {
            self.retry.mark_full_check();
            self.retry.save_if_dirty();
        }
        let ignore_extras = self.settings().map(|s| s.ignore_local_extras).unwrap_or(true);
        let tx = self.tx.clone();
        self.shared.push_log(if one_off {
            format!("Checking {}", folders[0].display())
        } else {
            "Checking existing files".into()
        });
        thread::spawn(move || {
            let mut found = Vec::new();
            for folder in &folders {
                found.extend(walk_videos(folder, ignore_extras));
            }
            let _ = tx.send(Msg::Found(found, Origin::Existing));
        });
    }

    fn on_found(&mut self, paths: Vec<PathBuf>, origin: Origin) {
        if origin == Origin::Existing {
            self.shared.catching_up.store(false, Ordering::SeqCst);
        }
        let before = self.queue.len();
        let count = paths.len();
        for path in paths {
            if origin == Origin::New {
                self.add_candidate(path, origin);
            } else {
                // Existing files don't need to settle.
                self.consider(path, origin);
            }
        }
        if origin == Origin::Existing {
            let queued = self.queue.len().saturating_sub(before);
            self.shared
                .push_log(format!("Checked {count} video(s): {queued} need subtitles now"));
        }
    }

    fn add_candidate(&mut self, path: PathBuf, origin: Origin) {
        if self.queued_set.contains(&path) || self.workers.iter().any(|w| w.path == path) {
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
            existing.last_change = now;
            return;
        }
        debug!("New video detected: {}", path.display());
        self.candidates.insert(path, Candidate { size, modified, last_change: now, origin });
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
            if now.duration_since(cand.last_change) < settle || size == 0 {
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
            if let Some(cand) = self.candidates.remove(&path) {
                self.processed.insert(path.clone(), (cand.size, cand.modified));
                self.consider(path, cand.origin);
            }
        }
    }

    /// Decide whether a video needs subtitles now, and queue it if so.
    fn consider(&mut self, path: PathBuf, origin: Origin) {
        let Some(settings) = self.settings() else {
            return;
        };
        if self.queued_set.contains(&path) || self.workers.iter().any(|w| w.path == path) {
            return;
        }
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
            self.retry.forget(&path);
            if origin == Origin::New {
                self.shared.push_log(format!("{name}: already has subtitles"));
            }
            return;
        }
        if origin != Origin::Retry && !self.retry.is_due(&path) {
            // Nothing was online last time; it'll be tried on schedule.
            if origin == Origin::New {
                self.shared.push_log(format!("{name}: waiting to retry"));
            }
            return;
        }
        if settings.skip_scanned_media {
            let skip = self
                .shared
                .scan_history
                .lock()
                .map(|h| {
                    h.should_skip(&path, &settings.selected_languages, settings.exclude_hearing_impaired)
                })
                .unwrap_or(false);
            if skip {
                return;
            }
        }
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        {
            let mut jobs = self.shared.jobs.lock().unwrap_or_else(|e| e.into_inner());
            trim_jobs(&mut jobs);
            jobs.push(DownloadJob {
                id,
                video_path: path.clone(),
                status: JobStatus::Pending,
                output: match origin {
                    Origin::New => "New arrival".into(),
                    Origin::Existing => "Missing subtitles".into(),
                    Origin::Retry => "Trying again".into(),
                },
                subtitle_paths: Vec::new(),
                at: chrono::Local::now(),
            });
        }
        if origin == Origin::New {
            self.shared.push_log(format!("{name}: queued"));
            self.queue.push_front((id, path.clone()));
        } else {
            self.queue.push_back((id, path.clone()));
        }
        self.queued_set.insert(path);
        self.shared.repaint();
    }

    // ---------- downloading ----------

    fn tick(&mut self) {
        let settings = self.settings();
        if let Some(settings) = settings.as_ref() {
            self.evaluate_candidates(settings);
        }
        self.reap_workers();
        if let Some(settings) = settings.as_ref() {
            self.start_workers(settings);
            if self.target.is_active() {
                if self.last_folder_check.is_none_or(|t| t.elapsed() >= FOLDER_CHECK_EVERY) {
                    self.check_folders();
                }
                if settings.watch_catch_up
                    && !self.watching.is_empty()
                    && self.retry.full_check_due()
                    && !self.shared.catching_up.load(Ordering::SeqCst)
                {
                    self.check_existing(None);
                }
                if self.last_retry_check.elapsed() >= RETRY_CHECK_EVERY {
                    self.last_retry_check = Instant::now();
                    let due = self.retry.take_due();
                    if !due.is_empty() {
                        self.shared.push_log(format!("Trying {} video(s) again", due.len()));
                        self.on_found(due, Origin::Retry);
                    }
                }
            }
        }
        if self.workers.is_empty() && self.queue.is_empty() && self.finished_since_stats {
            self.refresh_stats(false);
        }
        self.retry.save_if_dirty();
        self.shared.settling.store(self.candidates.len(), Ordering::SeqCst);
        self.shared.queued.store(self.queue.len(), Ordering::SeqCst);
        self.shared.running.store(self.workers.len(), Ordering::SeqCst);
        self.shared.retry_waiting.store(self.retry.waiting(), Ordering::SeqCst);
    }

    fn job_setup(&mut self) -> Option<JobSetup> {
        let (generation, settings, command) = {
            let runtime = self.shared.runtime.lock().unwrap_or_else(|e| e.into_inner());
            (runtime.generation, runtime.settings.clone(), runtime.command.clone())
        };
        if let Some((built, setup)) = &self.setup {
            if *built == generation {
                return Some(setup.clone());
            }
        }
        let settings = settings?;
        let Some(command) = command else {
            self.log_waiting("Waiting for Subliminal to be ready before downloading");
            return None;
        };
        match JobSetup::new(&settings, command, Arc::clone(&self.shared.scan_history), Arc::clone(&self.cancel)) {
            Ok(setup) => {
                self.waiting_reason_logged = None;
                self.setup = Some((generation, setup.clone()));
                Some(setup)
            }
            Err(error) => {
                warn!("{error}");
                self.log_waiting("Subliminal config error; see log");
                None
            }
        }
    }

    fn start_workers(&mut self, settings: &Settings) {
        let max = settings.concurrent_downloads.max(1);
        while self.workers.len() < max && !self.queue.is_empty() {
            let Some(mut setup) = self.job_setup() else {
                return;
            };
            setup.cancel = Arc::clone(&self.cancel);
            let Some((id, path)) = self.queue.pop_front() else {
                return;
            };
            self.queued_set.remove(&path);
            self.shared.update_job(id, |job| {
                job.status = JobStatus::Running;
                job.output = "Searching…".into();
            });
            let shared = Arc::clone(&self.shared);
            let job_path = path.clone();
            let handle = thread::spawn(move || {
                let mut set_output = |line: String| {
                    shared.update_job(id, |job| job.output = line);
                };
                process_video(&setup, &job_path, &mut set_output)
            });
            self.workers.push(Worker { id, path, handle });
        }
    }

    fn reap_workers(&mut self) {
        let mut i = 0;
        while i < self.workers.len() {
            if !self.workers[i].handle.is_finished() {
                i += 1;
                continue;
            }
            let worker = self.workers.swap_remove(i);
            let result = worker.handle.join().unwrap_or_else(|_| JobResult {
                status: JobStatus::Failed("Download worker crashed".into()),
                subtitle_paths: Vec::new(),
            });
            self.finish_job(worker.id, &worker.path, result);
        }
    }

    fn finish_job(&mut self, id: u64, path: &Path, result: JobResult) {
        let name = Utils::get_file_name(path);
        let mut status = result.status;
        match &mut status {
            JobStatus::Success => {
                self.retry.forget(path);
                self.shared.count_found();
                self.shared.push_log(format!("{name}: subtitles found"));
            }
            JobStatus::Skipped | JobStatus::EmbeddedExists(_) => self.retry.forget(path),
            JobStatus::NotFound(message) | JobStatus::Failed(message) => {
                let wait = self.retry.record_miss(path);
                message.push_str(&format!(" · retrying in {}", retry::describe(wait)));
                self.shared.push_log(format!("{name}: {message}"));
            }
            JobStatus::Cancelled | JobStatus::Pending | JobStatus::Running => {}
        }
        self.shared.update_job(id, |job| {
            job.status = status;
            job.subtitle_paths = result.subtitle_paths;
            job.output.clear();
            job.at = chrono::Local::now();
        });
        self.finished_since_stats = true;
        self.shared.repaint();
    }

    fn cancel_all(&mut self) {
        if self.queue.is_empty() && self.workers.is_empty() {
            return;
        }
        self.cancel.store(true, Ordering::SeqCst);
        let dropped: Vec<u64> = self.queue.drain(..).map(|(id, _)| id).collect();
        self.queued_set.clear();
        for id in dropped {
            self.shared.update_job(id, |job| {
                job.status = JobStatus::Cancelled;
                job.output.clear();
            });
        }
        for worker in self.workers.drain(..) {
            let result = worker.handle.join().unwrap_or_else(|_| JobResult {
                status: JobStatus::Cancelled,
                subtitle_paths: Vec::new(),
            });
            self.shared.update_job(worker.id, |job| {
                job.status = if result.status == JobStatus::Success {
                    JobStatus::Success
                } else {
                    JobStatus::Cancelled
                };
                job.output.clear();
            });
        }
        // Fresh flag for whatever comes next.
        self.cancel = Arc::new(AtomicBool::new(false));
        self.setup = None;
        self.shared.push_log("Downloads cancelled");
        self.shared.repaint();
    }

    fn log_waiting(&mut self, reason: &'static str) {
        if self.waiting_reason_logged != Some(reason) {
            self.waiting_reason_logged = Some(reason);
            self.shared.push_log(reason);
        }
    }

    /// Recount videos and subtitle coverage for every watched folder.
    fn refresh_stats(&mut self, force: bool) {
        if !self.target.is_active() {
            return;
        }
        if !force && self.last_stats.is_some_and(|t| t.elapsed() < STATS_EVERY) {
            return;
        }
        if self.shared.stats_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(settings) = self.settings() else {
            self.shared.stats_running.store(false, Ordering::SeqCst);
            return;
        };
        self.finished_since_stats = false;
        self.last_stats = Some(Instant::now());
        let folders = self.watching.iter().cloned().collect::<Vec<_>>();
        let shared = Arc::clone(&self.shared);
        thread::spawn(move || {
            for folder in folders {
                let videos = walk_videos(&folder, settings.ignore_local_extras);
                let mut listings: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
                let mut with_subs = 0;
                for video in &videos {
                    let Some(parent) = video.parent() else { continue };
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
                    .insert(folder, FolderStats { videos: videos.len(), with_subs });
                shared.repaint();
            }
            shared.stats_running.store(false, Ordering::SeqCst);
        });
    }
}

/// Keep the list bounded, dropping the oldest finished jobs first.
fn trim_jobs(jobs: &mut Vec<DownloadJob>) {
    while jobs.len() >= MAX_JOBS {
        match jobs.iter().position(|job| !job.status.is_active()) {
            Some(index) => {
                jobs.remove(index);
            }
            None => break,
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
                    && entry.file_name().to_str().is_some_and(Utils::is_plex_extras_folder)
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
        assert!(in_extras_folder(Path::new("/media/Movie (2020)/Featurettes/clip.mkv")));
        assert!(!in_extras_folder(Path::new("/media/Movie (2020)/Movie.mkv")));
    }

    #[test]
    fn watch_target_inactive_without_folders() {
        assert!(!WatchTarget::default().is_active());
        let target = WatchTarget { folders: vec![PathBuf::from("/tmp")], ..Default::default() };
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

    fn job(id: u64, status: JobStatus) -> DownloadJob {
        DownloadJob {
            id,
            video_path: PathBuf::from(format!("/v/{id}.mkv")),
            status,
            output: String::new(),
            subtitle_paths: Vec::new(),
            at: chrono::Local::now(),
        }
    }

    #[test]
    fn trimming_keeps_queued_and_running_jobs() {
        let mut jobs: Vec<DownloadJob> = (0..MAX_JOBS as u64)
            .map(|id| job(id, if id < 3 { JobStatus::Pending } else { JobStatus::Success }))
            .collect();
        trim_jobs(&mut jobs);
        assert_eq!(jobs.len(), MAX_JOBS - 1);
        assert!(jobs.iter().take(3).all(|j| j.status == JobStatus::Pending));
    }
}

/// The queue end to end, with a stand-in for Subliminal: videos named
/// "*found*" get an .en.srt, anything else finds nothing.
#[cfg(all(test, unix))]
mod queue_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_subliminal(dir: &Path) -> SubliminalCommand {
        let script = dir.join("fake-subliminal");
        std::fs::write(
            &script,
            r#"#!/bin/sh
for last; do :; done
echo "$last" >> "$(dirname "$0")/calls.log"
sleep 1
case "$(basename "$last")" in
  *found*) touch "${last%.*}.en.srt"; echo "Downloaded 1 subtitle" ;;
  *) echo "Downloaded 0 subtitle" ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        SubliminalCommand { program: script, prefix: vec![] }
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(40);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(200));
        }
    }

    fn status_of(handle: &WatchHandle, name: &str) -> Option<JobStatus> {
        handle
            .shared
            .jobs_snapshot()
            .iter()
            .rev()
            .find(|j| j.video_path.file_name().is_some_and(|n| n == name))
            .map(|j| j.status.clone())
    }

    #[test]
    fn existing_new_and_retry_flow() {
        let home = tempfile::tempdir().unwrap();
        // Keep settings, history and the retry list out of the real profile.
        std::env::set_var("XDG_CONFIG_HOME", home.path().join("config"));
        let tools = tempfile::tempdir().unwrap();
        let media = tempfile::tempdir().unwrap();
        std::fs::write(media.path().join("Old.found.mkv"), vec![0u8; 2048]).unwrap();
        std::fs::write(media.path().join("Old.missing.mkv"), vec![0u8; 2048]).unwrap();
        std::fs::write(media.path().join("Has.subs.mkv"), vec![0u8; 2048]).unwrap();
        std::fs::write(media.path().join("Has.subs.en.srt"), b"1").unwrap();

        let history = Arc::new(Mutex::new(ScanHistory::default()));
        let (handle, thread) = WatchHandle::start(history);
        let settings = Settings {
            watch_enabled: true,
            watch_settle_seconds: 1,
            concurrent_downloads: 2,
            providers: vec![],
            refiners: vec![],
            skip_scanned_media: false,
            watch_folders: vec![media.path().display().to_string()],
            ..Settings::default()
        };
        handle.update_runtime(settings.clone(), Some(fake_subliminal(tools.path())));
        handle.configure(WatchTarget::from_settings(&settings));

        // The first full check queues the two without subtitles.
        wait_for("the full check", || {
            matches!(status_of(&handle, "Old.found.mkv"), Some(JobStatus::Success))
                && matches!(status_of(&handle, "Old.missing.mkv"), Some(JobStatus::NotFound(_)))
        });
        assert!(status_of(&handle, "Has.subs.mkv").is_none(), "already had subtitles");
        assert!(media.path().join("Old.found.en.srt").exists());
        match status_of(&handle, "Old.missing.mkv") {
            Some(JobStatus::NotFound(message)) => assert!(message.contains("retrying in 6 h"), "{message}"),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(handle.shared.retry_waiting.load(Ordering::SeqCst), 1);
        assert_eq!(handle.shared.found_today(), 1);

        // A new arrival is picked up once it settles.
        std::fs::write(media.path().join("New.found.mkv"), vec![1u8; 4096]).unwrap();
        wait_for("the new arrival", || {
            matches!(status_of(&handle, "New.found.mkv"), Some(JobStatus::Success))
        });

        // "Restart": stop and start watching. Nothing is searched again: the
        // daily check already ran and the miss waits for its retry time.
        let calls = || std::fs::read_to_string(tools.path().join("calls.log")).unwrap().lines().count();
        let before = calls();
        handle.configure(WatchTarget::default());
        handle.configure(WatchTarget::from_settings(&settings));
        handle.check_existing_now();
        thread::sleep(Duration::from_secs(4));
        assert_eq!(calls(), before, "re-searched videos that should have waited");

        handle.stop();
        thread.join().unwrap();
        let retry = std::fs::read_to_string(home.path().join("config/jenova/jenova_retry.json")).unwrap();
        assert!(retry.contains("Old.missing.mkv"));
    }
}
