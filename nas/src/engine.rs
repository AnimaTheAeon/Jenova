//! The watcher service: folder events -> settle -> check -> Subliminal.
//!
//! One thread owns the state machine. It blocks while nothing is pending, so
//! an idle NAS sees no wake-ups (and disks can sleep). Downloads run on short
//! worker threads, a few at a time, at low CPU priority.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

use crate::config::{in_skipped_folder, is_extras_folder, is_system_folder, is_video_file, var_dir, Config};
use crate::python::{self, SubliminalState};
use crate::subs::SubtitleUtils;
use crate::{debug, info, warn};

const MAX_JOBS: usize = 300;
const TICK: Duration = Duration::from_secs(1);

// ---------- shared state ----------

#[derive(Clone, Serialize, Default, Debug)]
pub struct FolderInfo {
    /// "watching", "polling", "paused", "offline", "error"
    pub state: String,
    pub error: Option<String>,
    pub videos: Option<usize>,
    pub with_subs: Option<usize>,
    pub readable: bool,
    pub writable: bool,
}

#[derive(Clone, Serialize, Debug)]
pub struct Job {
    pub id: u64,
    pub path: String,
    /// "queued", "running", "found", "none", "failed"
    pub status: String,
    pub langs: Vec<String>,
    pub message: String,
    pub time: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct Record {
    size: u64,
    mtime: i64,
    found: bool,
    at: u64,
}

pub struct Shared {
    pub config: RwLock<Config>,
    pub config_path: PathBuf,
    pub folders: Mutex<HashMap<String, FolderInfo>>,
    pub jobs: Mutex<VecDeque<Job>>,
    pub subliminal: Mutex<SubliminalState>,
    pub python: Mutex<Option<String>>,
    pub settling: AtomicUsize,
    pub queued: AtomicUsize,
    pub running: AtomicUsize,
    pub scanning: AtomicUsize,
    pub started: Instant,
    history: Mutex<HashMap<String, Record>>,
    history_path: PathBuf,
    cancel: AtomicBool,
    tx: Mutex<Option<Sender<Msg>>>,
}

impl Shared {
    pub fn send(&self, msg: Msg) {
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.send(msg);
        }
    }

    pub fn subs_found_total(&self) -> usize {
        self.history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|r| r.found)
            .count()
    }

    pub fn update_config(&self, mut config: Config) -> std::io::Result<()> {
        config.normalize();
        config.save(&self.config_path)?;
        let _ = write_subliminal_config(&config);
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = config;
        self.send(Msg::Configure);
        Ok(())
    }

    pub fn clear_finished_jobs(&self) {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|j| j.status == "queued" || j.status == "running");
    }
}

pub enum Msg {
    Configure,
    CheckNow,
    InstallSubliminal,
    SubliminalReady,
    Fs(notify::Result<notify::Event>),
    Found {
        folder: String,
        videos: Vec<PathBuf>,
        with_subs: usize,
        queue: bool,
    },
    JobDone {
        id: u64,
        path: PathBuf,
        found_langs: Vec<String>,
        ok: bool,
        message: String,
    },
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fingerprint(path: &Path) -> Option<(u64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((meta.len(), mtime))
}

fn write_subliminal_config(config: &Config) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let path = var_dir().join("subliminal.toml");
    std::fs::write(&path, config.subliminal_toml())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
}

fn access_ok(path: &Path, mode: libc::c_int) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: valid NUL-terminated path.
    unsafe { libc::access(c.as_ptr(), mode) == 0 }
}

pub fn folder_access(path: &Path) -> (bool, bool) {
    (
        path.is_dir() && access_ok(path, libc::R_OK | libc::X_OK),
        access_ok(path, libc::W_OK),
    )
}

// ---------- startup ----------

pub fn start() -> Arc<Shared> {
    let var = var_dir();
    let _ = std::fs::create_dir_all(&var);
    let config_path = var.join("config.json");
    let config = Config::load(&config_path);
    let _ = config.save(&config_path);
    let _ = write_subliminal_config(&config);
    let history_path = var.join("history.json");
    let history: HashMap<String, Record> = std::fs::read(&history_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();

    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        config: RwLock::new(config),
        config_path,
        folders: Mutex::new(HashMap::new()),
        jobs: Mutex::new(VecDeque::new()),
        subliminal: Mutex::new(SubliminalState::Checking),
        python: Mutex::new(None),
        settling: AtomicUsize::new(0),
        queued: AtomicUsize::new(0),
        running: AtomicUsize::new(0),
        scanning: AtomicUsize::new(0),
        started: Instant::now(),
        history: Mutex::new(history),
        history_path,
        cancel: AtomicBool::new(false),
        tx: Mutex::new(Some(tx.clone())),
    });

    {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("engine".into())
            .spawn(move || Engine::new(shared, tx).run(rx))
            .expect("spawn engine");
    }
    shared.send(Msg::InstallSubliminal);
    shared.send(Msg::Configure);
    shared
}

// ---------- engine ----------

#[derive(Clone, Copy)]
struct Candidate {
    size: u64,
    mtime: i64,
    last_change: Instant,
    existing: bool,
}

struct Engine {
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    inotify: Option<notify::INotifyWatcher>,
    poller: Option<notify::PollWatcher>,
    active: Vec<String>,
    candidates: HashMap<PathBuf, Candidate>,
    queue: VecDeque<(u64, PathBuf)>,
    queued: HashSet<PathBuf>,
    known_videos: HashSet<PathBuf>,
    next_id: u64,
    installing: bool,
}

impl Engine {
    fn new(shared: Arc<Shared>, tx: Sender<Msg>) -> Self {
        Self {
            shared,
            tx,
            inotify: None,
            poller: None,
            active: Vec::new(),
            candidates: HashMap::new(),
            queue: VecDeque::new(),
            queued: HashSet::new(),
            known_videos: HashSet::new(),
            next_id: 1,
            installing: false,
        }
    }

    fn config(&self) -> Config {
        self.shared.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            // Sleep until something happens unless work is pending.
            let busy = !self.candidates.is_empty() || !self.queue.is_empty();
            let msg = if busy {
                match rx.recv_timeout(TICK) {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => break,
                }
            };
            if let Some(msg) = msg {
                self.handle(msg);
            }
            self.evaluate_candidates();
            self.dispatch();
            self.publish_counts();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Configure => self.configure(),
            Msg::CheckNow => self.scan_all(true),
            Msg::InstallSubliminal => self.ensure_subliminal(),
            Msg::SubliminalReady => {
                self.installing = false;
            }
            Msg::Fs(event) => self.on_event(event),
            Msg::Found {
                folder,
                videos,
                with_subs,
                queue,
            } => {
                if let Some(info) = self
                    .shared
                    .folders
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(&folder)
                {
                    info.videos = Some(videos.len());
                    info.with_subs = Some(with_subs);
                }
                self.known_videos.extend(videos.iter().cloned());
                if queue {
                    let mut added = 0;
                    for video in videos {
                        if self.add_candidate(video, true) {
                            added += 1;
                        }
                    }
                    if added > 0 {
                        info!("{folder}: {added} existing video(s) to check");
                    }
                }
                let _ = self.shared.scanning.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
            }
            Msg::JobDone {
                id,
                path,
                found_langs,
                ok,
                message,
            } => self.job_done(id, path, found_langs, ok, message),
        }
    }

    // ----- Subliminal -----

    fn ensure_subliminal(&mut self) {
        if self.installing {
            return;
        }
        self.installing = true;
        let shared = Arc::clone(&self.shared);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let set = |state: SubliminalState| {
                python::log_state(&state);
                *shared.subliminal.lock().unwrap_or_else(|e| e.into_inner()) = state;
            };
            set(SubliminalState::Checking);
            let Some(py) = python::find_python() else {
                set(SubliminalState::NoPython);
                let _ = tx.send(Msg::SubliminalReady);
                return;
            };
            *shared.python.lock().unwrap_or_else(|e| e.into_inner()) = Some(py.clone());
            let state = match python::subliminal_version(&py) {
                Some(version) => SubliminalState::Ready {
                    version,
                    python: py.clone(),
                },
                None => {
                    info!("Installing Subliminal");
                    let progress = |step: &str| {
                        *shared.subliminal.lock().unwrap_or_else(|e| e.into_inner()) =
                            SubliminalState::Installing { step: step.into() };
                    };
                    match python::install(&py, &progress) {
                        Ok(version) => SubliminalState::Ready {
                            version,
                            python: py.clone(),
                        },
                        Err(message) => SubliminalState::Error { message },
                    }
                }
            };
            set(state);
            let _ = tx.send(Msg::SubliminalReady);
        });
    }

    /// Reinstall/upgrade on request.
    pub fn reinstall(shared: &Arc<Shared>) {
        let shared = Arc::clone(shared);
        std::thread::spawn(move || {
            let py = shared.python.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let Some(py) = py.or_else(python::find_python) else {
                *shared.subliminal.lock().unwrap_or_else(|e| e.into_inner()) = SubliminalState::NoPython;
                return;
            };
            let progress = |step: &str| {
                *shared.subliminal.lock().unwrap_or_else(|e| e.into_inner()) =
                    SubliminalState::Installing { step: step.into() };
            };
            let state = match python::install(&py, &progress) {
                Ok(version) => SubliminalState::Ready {
                    version,
                    python: py.clone(),
                },
                Err(message) => SubliminalState::Error { message },
            };
            python::log_state(&state);
            *shared.subliminal.lock().unwrap_or_else(|e| e.into_inner()) = state;
            shared.send(Msg::SubliminalReady);
        });
    }

    fn subliminal_python(&self) -> Option<String> {
        match &*self.shared.subliminal.lock().unwrap_or_else(|e| e.into_inner()) {
            SubliminalState::Ready { python, .. } => Some(python.clone()),
            _ => None,
        }
    }

    // ----- watching -----

    fn configure(&mut self) {
        let config = self.config();
        let wanted: Vec<String> = if config.watch_enabled {
            config.folders.clone()
        } else {
            Vec::new()
        };

        // Rebuild watchers from scratch; cheap and keeps state simple.
        self.inotify = None;
        self.poller = None;
        self.candidates.clear();
        let first_start = self.active.is_empty();
        let newly_added: Vec<String> = wanted
            .iter()
            .filter(|f| !self.active.contains(f))
            .cloned()
            .collect();

        let mut folders = HashMap::new();
        for folder in &config.folders {
            let path = Path::new(folder);
            let (readable, writable) = folder_access(path);
            let mut info = FolderInfo {
                readable,
                writable,
                ..FolderInfo::default()
            };
            // Keep last known counts.
            if let Some(old) = self
                .shared
                .folders
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(folder)
            {
                info.videos = old.videos;
                info.with_subs = old.with_subs;
            }
            info.state = if !path.is_dir() {
                info.error = Some("Folder not found".into());
                "offline".into()
            } else if !readable {
                info.error = Some("Jenova doesn't have permission to read this folder".into());
                "error".into()
            } else if !config.watch_enabled {
                "paused".into()
            } else {
                String::new()
            };
            folders.insert(folder.clone(), info);
        }

        let tx = self.tx.clone();
        let handler = move |event: notify::Result<notify::Event>| {
            let _ = tx.send(Msg::Fs(event));
        };
        let mut use_polling: Vec<String> = Vec::new();
        if !config.polling {
            match notify::INotifyWatcher::new(handler.clone(), notify::Config::default()) {
                Ok(mut watcher) => {
                    for folder in &wanted {
                        if !folders.get(folder).is_some_and(|f| f.state.is_empty()) {
                            continue;
                        }
                        match watcher.watch(Path::new(folder), RecursiveMode::Recursive) {
                            Ok(()) => {
                                folders.get_mut(folder).map(|f| f.state = "watching".into());
                            }
                            Err(error) => {
                                // Typically DSM's inotify watch limit on huge trees.
                                let _ = watcher.unwatch(Path::new(folder));
                                warn!("{folder}: live watching unavailable ({error}); using polling");
                                use_polling.push(folder.clone());
                            }
                        }
                    }
                    self.inotify = Some(watcher);
                }
                Err(error) => {
                    warn!("inotify unavailable ({error}); using polling");
                    use_polling.extend(wanted.iter().cloned());
                }
            }
        } else {
            use_polling.extend(wanted.iter().cloned());
        }

        use_polling.retain(|f| folders.get(f).is_some_and(|i| i.state.is_empty()));
        if !use_polling.is_empty() {
            let poll_config = notify::Config::default()
                .with_poll_interval(Duration::from_secs(config.poll_interval_seconds));
            match notify::PollWatcher::new(handler, poll_config) {
                Ok(mut watcher) => {
                    for folder in &use_polling {
                        match watcher.watch(Path::new(folder), RecursiveMode::Recursive) {
                            Ok(()) => {
                                folders.get_mut(folder).map(|f| f.state = "polling".into());
                            }
                            Err(error) => {
                                if let Some(f) = folders.get_mut(folder) {
                                    f.state = "error".into();
                                    f.error = Some(error.to_string());
                                }
                            }
                        }
                    }
                    self.poller = Some(watcher);
                }
                Err(error) => warn!("Polling unavailable: {error}"),
            }
        }

        *self.shared.folders.lock().unwrap_or_else(|e| e.into_inner()) = folders;
        let watching: Vec<&String> = wanted.iter().collect();
        if config.watch_enabled {
            info!("Watching {} folder(s)", watching.len());
        } else {
            info!("Watching is paused");
        }

        // Count (and on first start, catch up on) folders.
        if first_start {
            self.scan_all(config.catch_up);
        } else {
            for folder in newly_added {
                self.scan_folder(folder, config.catch_up);
            }
        }
        self.active = wanted;
    }

    fn scan_all(&mut self, queue: bool) {
        let folders = self.config().folders;
        for folder in folders {
            self.scan_folder(folder, queue);
        }
    }

    /// Walk a folder in the background: counts for the UI, and optionally
    /// queue videos that are missing subtitles.
    fn scan_folder(&mut self, folder: String, queue: bool) {
        let config = self.config();
        let queue = queue && config.watch_enabled;
        let tx = self.tx.clone();
        self.shared.scanning.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let videos = walk_videos(Path::new(&folder), config.ignore_extras);
            let mut with_subs = 0;
            let mut listings: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
            for video in &videos {
                let Some(parent) = video.parent() else { continue };
                let listing = listings
                    .entry(parent.to_path_buf())
                    .or_insert_with(|| SubtitleUtils::list_subtitle_files_in_folder(parent));
                if !SubtitleUtils::video_missing_subtitle_in_listing(
                    video,
                    &config.languages,
                    config.exclude_hearing_impaired,
                    listing,
                ) {
                    with_subs += 1;
                }
            }
            let _ = tx.send(Msg::Found {
                folder,
                videos,
                with_subs,
                queue,
            });
        });
    }

    fn on_event(&mut self, event: notify::Result<notify::Event>) {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                debug!("Watcher error: {error}");
                return;
            }
        };
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any) {
            return;
        }
        let ignore_extras = self.config().ignore_extras;
        for path in event.paths {
            if in_skipped_folder(&path, ignore_extras) {
                continue;
            }
            if path.is_dir() {
                // A folder moved in produces no events for its contents.
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if is_system_folder(name) || (ignore_extras && is_extras_folder(name)) {
                    continue;
                }
                for video in walk_videos(&path, ignore_extras) {
                    self.add_candidate(video, false);
                }
            } else if is_video_file(&path) {
                self.add_candidate(path, false);
            }
        }
    }

    /// Returns true if newly added.
    fn add_candidate(&mut self, path: PathBuf, existing: bool) -> bool {
        if self.queued.contains(&path) {
            return false;
        }
        let Some((size, mtime)) = fingerprint(&path) else {
            return false;
        };
        let now = Instant::now();
        if let Some(c) = self.candidates.get_mut(&path) {
            if !existing {
                c.last_change = now;
                c.existing = false;
            }
            return false;
        }
        self.candidates.insert(
            path,
            Candidate {
                size,
                mtime,
                last_change: now,
                existing,
            },
        );
        true
    }

    fn evaluate_candidates(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        let config = self.config();
        let settle = Duration::from_secs(config.settle_seconds);
        let now = Instant::now();
        let mut ready = Vec::new();
        let mut gone = Vec::new();
        for (path, c) in self.candidates.iter_mut() {
            let Some((size, mtime)) = fingerprint(path) else {
                gone.push(path.clone());
                continue;
            };
            if size != c.size || mtime != c.mtime {
                c.size = size;
                c.mtime = mtime;
                c.last_change = now;
                continue;
            }
            let age_ok = if c.existing {
                // Files from a scan only need to be old enough.
                (now_secs() as i64 - mtime) >= settle.as_secs() as i64
                    || now.duration_since(c.last_change) >= settle
            } else {
                now.duration_since(c.last_change) >= settle
            };
            if age_ok && size > 0 {
                ready.push(path.clone());
            }
        }
        for path in gone {
            self.candidates.remove(&path);
        }
        for path in ready {
            if let Some(c) = self.candidates.remove(&path) {
                self.consider(path, c, &config);
            }
        }
    }

    fn consider(&mut self, path: PathBuf, c: Candidate, config: &Config) {
        if config.languages.is_empty() || in_skipped_folder(&path, config.ignore_extras) {
            return;
        }
        let new_video = self.known_videos.insert(path.clone());
        if new_video {
            self.bump_folder_counts(&path, 1, 0);
        }
        let missing = SubtitleUtils::video_missing_subtitle_in_listing(
            &path,
            &config.languages,
            config.exclude_hearing_impaired,
            &path
                .parent()
                .map(SubtitleUtils::list_subtitle_files_in_folder)
                .unwrap_or_default(),
        );
        if !missing {
            return;
        }
        // Skip what was already handled (unless the file changed).
        let key = path.to_string_lossy().into_owned();
        if let Some(record) = self
            .shared
            .history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            let same_file = record.size == c.size && record.mtime == c.mtime;
            let recent = now_secs().saturating_sub(record.at) < config.retry_hours * 3600;
            if same_file && (record.found || recent) {
                return;
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        if !c.existing {
            info!("New video: {}", display_name(&path));
        }
        self.push_job(Job {
            id,
            path: key,
            status: "queued".into(),
            langs: Vec::new(),
            message: String::new(),
            time: crate::log::timestamp(),
        });
        self.queued.insert(path.clone());
        self.queue.push_back((id, path));
    }

    fn push_job(&self, job: Job) {
        let mut jobs = self.shared.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.push_back(job);
        while jobs.len() > MAX_JOBS {
            // Drop the oldest finished job.
            match jobs.iter().position(|j| j.status != "queued" && j.status != "running") {
                Some(i) => {
                    jobs.remove(i);
                }
                None => break,
            }
        }
    }

    fn update_job(&self, id: u64, f: impl FnOnce(&mut Job)) {
        if let Some(job) = self
            .shared
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
            .find(|j| j.id == id)
        {
            f(job);
        }
    }

    fn bump_folder_counts(&self, path: &Path, videos: isize, with_subs: isize) {
        let mut folders = self.shared.folders.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, info)) = folders
            .iter_mut()
            .filter(|(f, _)| path.starts_with(f.as_str()))
            .max_by_key(|(f, _)| f.len())
        {
            if let Some(v) = info.videos.as_mut() {
                *v = (*v as isize + videos).max(0) as usize;
            }
            if let Some(w) = info.with_subs.as_mut() {
                *w = (*w as isize + with_subs).max(0) as usize;
            }
        }
    }

    // ----- downloads -----

    fn dispatch(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let Some(py) = self.subliminal_python() else {
            return;
        };
        let config = self.config();
        while self.shared.running.load(Ordering::SeqCst) < config.concurrent_downloads {
            let Some((id, path)) = self.queue.pop_front() else {
                break;
            };
            self.queued.remove(&path);
            if !path.exists() {
                self.update_job(id, |j| {
                    j.status = "failed".into();
                    j.message = "File was removed".into();
                });
                continue;
            }
            self.shared.running.fetch_add(1, Ordering::SeqCst);
            self.update_job(id, |j| j.status = "running".into());
            let tx = self.tx.clone();
            let shared = Arc::clone(&self.shared);
            let py = py.clone();
            let config = config.clone();
            std::thread::spawn(move || {
                let (found_langs, ok, message) = download(&py, &path, &config, &shared.cancel);
                let _ = tx.send(Msg::JobDone {
                    id,
                    path,
                    found_langs,
                    ok,
                    message,
                });
            });
        }
    }

    fn job_done(&mut self, id: u64, path: PathBuf, found_langs: Vec<String>, ok: bool, message: String) {
        self.shared.running.fetch_sub(1, Ordering::SeqCst);
        let found = !found_langs.is_empty();
        let status = if found {
            "found"
        } else if ok {
            "none"
        } else {
            "failed"
        };
        let name = display_name(&path);
        if found {
            info!("{name}: subtitles found ({})", found_langs.join(", "));
            self.bump_folder_counts(&path, 0, 1);
        } else if ok {
            info!("{name}: no subtitles found");
        } else {
            warn!("{name}: {message}");
        }
        self.update_job(id, |j| {
            j.status = status.into();
            j.langs = found_langs;
            j.message = message;
            j.time = crate::log::timestamp();
        });
        if let Some((size, mtime)) = fingerprint(&path) {
            let mut history = self.shared.history.lock().unwrap_or_else(|e| e.into_inner());
            history.insert(
                path.to_string_lossy().into_owned(),
                Record {
                    size,
                    mtime,
                    found,
                    at: now_secs(),
                },
            );
            if let Ok(json) = serde_json::to_vec(&*history) {
                let _ = crate::config::write_atomic(&self.shared.history_path, &json);
            }
        }
    }

    fn publish_counts(&self) {
        self.shared.settling.store(self.candidates.len(), Ordering::SeqCst);
        self.shared.queued.store(self.queue.len(), Ordering::SeqCst);
    }
}

/// Run Subliminal for one video. Returns (languages found, ran ok, message).
fn download(python: &str, video: &Path, config: &Config, cancel: &AtomicBool) -> (Vec<String>, bool, String) {
    let folder = video.parent().unwrap_or(Path::new("/"));
    let before: HashMap<PathBuf, Option<SystemTime>> =
        SubtitleUtils::subtitle_files_for_video_in_listing(video, &SubtitleUtils::list_subtitle_files_in_folder(folder))
            .into_iter()
            .map(|p| {
                let m = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
                (p, m)
            })
            .collect();

    let var = var_dir();
    let mut args: Vec<String> = vec![
        "--config".into(),
        var.join("subliminal.toml").to_string_lossy().into_owned(),
        "--cache-dir".into(),
        var.join("cache").to_string_lossy().into_owned(),
        "download".into(),
    ];
    if config.ignore_embedded {
        args.push("--force-embedded-subtitles".into());
    }
    if config.exclude_hearing_impaired {
        args.push("--no-hearing-impaired".into());
        args.push("--language-type-suffix".into());
    }
    for provider in &config.providers {
        args.push("--provider".into());
        args.push(provider.clone());
    }
    for refiner in &config.refiners {
        args.push("--refiner".into());
        args.push(refiner.clone());
    }
    if config.minimum_score > 0 {
        args.push("--min-score".into());
        args.push(config.minimum_score.to_string());
    }
    for lang in &config.languages {
        args.push("-l".into());
        args.push(lang.clone());
    }
    args.push(video.to_string_lossy().into_owned());

    let out = python::run_subliminal(python, &args, cancel);

    let after = SubtitleUtils::subtitle_files_for_video_in_listing(
        video,
        &SubtitleUtils::list_subtitle_files_in_folder(folder),
    );
    let changed: Vec<PathBuf> = after
        .into_iter()
        .filter(|p| {
            let m = std::fs::metadata(p).and_then(|m| m.modified()).ok();
            before.get(p).is_none_or(|old| *old != m)
        })
        .collect();
    let mut langs: Vec<String> = config
        .languages
        .iter()
        .filter(|lang| changed.iter().any(|p| SubtitleUtils::matches_language(video, p, lang)))
        .cloned()
        .collect();
    if langs.is_empty() && !changed.is_empty() {
        langs = config.languages.clone();
    }
    let message = if out.timed_out {
        "Subliminal took too long and was stopped".to_string()
    } else {
        out.summary
    };
    (langs, out.success || !changed.is_empty(), message)
}

/// Recursively list videos, skipping DSM system folders (and extras).
pub fn walk_videos(root: &Path, ignore_extras: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !is_system_folder(&name) && !(ignore_extras && is_extras_folder(&name)) {
                    stack.push(path);
                }
            } else if kind.is_file() && is_video_file(&path) {
                out.push(path);
            }
        }
    }
    out
}

pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn request_reinstall(shared: &Arc<Shared>) {
    Engine::reinstall(shared);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_skips_synology_and_extras_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for p in [
            "Movie (2020)/Movie.mkv",
            "Movie (2020)/@eaDir/Movie.mkv/SYNOVIDEO_VIDEO_SCREENSHOT.jpg",
            "Movie (2020)/@eaDir/fake.mkv",
            "#recycle/Old.mkv",
            "Show/Season 01/Show.S01E01.mkv",
            "Show/Featurettes/Bonus.mkv",
        ] {
            let path = root.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
        }
        let mut found: Vec<String> = walk_videos(root, true)
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect();
        found.sort();
        assert_eq!(found, vec!["Movie (2020)/Movie.mkv", "Show/Season 01/Show.S01E01.mkv"]);
        assert_eq!(walk_videos(root, false).len(), 3);
    }
}
