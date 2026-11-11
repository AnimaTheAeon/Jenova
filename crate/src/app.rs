//! Application logic.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::data_structures::{
    InitCheckResult, MenuPage, StartupPhase,
    SubliminalInstallResult, SubtitleDownloader,
};
use crate::python_manager::PythonManager;
use crate::scan_history::ScanHistory;
use crate::settings::Settings;

// Logging macros
use crate::{debug, error, info, warn};

// Version check
use once_cell::sync::Lazy;
type VersionCheckState = (Option<String>, Option<String>, bool);

static VERSION_PTR: Lazy<std::sync::Arc<std::sync::Mutex<VersionCheckState>>> =
    Lazy::new(|| std::sync::Arc::new(std::sync::Mutex::new((None, None, false))));


pub(crate) fn probe_startup_dependencies() -> InitCheckResult {
    let python_info = PythonManager::get_python_info();
    let (python_command, python_version) = python_info
        .map(|(command, version)| (Some(command), Some(version)))
        .unwrap_or((None, None));
    let python_installed = python_command.is_some();

    if python_installed {
        if let Err(error) = PythonManager::add_scripts_to_path() {
            warn!("Failed to add Python Scripts to PATH: {}", error);
        }
        if let Err(error) = PythonManager::refresh_environment() {
            warn!("Failed to refresh environment: {}", error);
        }
    }

    // Probe after Python setup.
    let ffmpeg_handle = thread::spawn(PythonManager::is_ffmpeg_installed);
    let homebrew_handle = thread::spawn(PythonManager::is_homebrew_installed);
    #[cfg(target_os = "macos")]
    let pipx_handle = thread::spawn(PythonManager::_pipx_available);

    let ffmpeg_installed = ffmpeg_handle.join().unwrap_or(false);
    let homebrew_installed = homebrew_handle.join().unwrap_or(false);

    // Keep Linux pipx setup ordered.
    #[cfg(windows)]
    let (pipx_installed, pipx_version) = (true, None::<String>);
    #[cfg(target_os = "macos")]
    let (pipx_installed, pipx_version) = (pipx_handle.join().unwrap_or(false), None::<String>);
    #[cfg(target_os = "linux")]
    let (pipx_installed, pipx_version) = if python_installed {
        let available = PythonManager::_pipx_available();
        let installed = if !available {
            info!("pipx not found, attempting to install pipx");
            if PythonManager::try_install_pipx() {
                if let Err(error) = PythonManager::add_scripts_to_path() {
                    warn!("Failed to add pipx Scripts directory to PATH: {}", error);
                }
                if let Err(error) = PythonManager::refresh_environment() {
                    warn!(
                        "Failed to refresh environment after pipx installation: {}",
                        error
                    );
                }
                PythonManager::_pipx_available()
            } else {
                false
            }
        } else {
            available
        };
        let version = if installed {
            PythonManager::get_pipx_version()
        } else {
            None
        };
        (installed, version)
    } else {
        (false, None)
    };

    let subliminal = if python_installed && (pipx_installed || cfg!(target_os = "macos")) {
        probe_subliminal(python_command.as_deref())
    } else {
        SubliminalInstallResult::default()
    };
    let SubliminalInstallResult {
        installed: subliminal_installed,
        version: subliminal_version,
        command: subliminal_command,
    } = subliminal;

    info!(
        "Python installed: {}, version: {:?}",
        python_installed, python_version
    );
    info!(
        "pipx installed: {}, version: {:?}",
        pipx_installed, pipx_version
    );
    info!(
        "Subliminal installed: {}, version: {:?}",
        subliminal_installed, subliminal_version
    );
    info!("FFmpeg installed: {}", ffmpeg_installed);
    info!("Homebrew installed: {}", homebrew_installed);

    InitCheckResult {
        python_command,
        python_version,
        pipx_installed,
        pipx_version,
        subliminal_installed,
        subliminal_version,
        subliminal_command,
        ffmpeg_installed,
        homebrew_installed,
    }
}

fn probe_subliminal(python: Option<&str>) -> SubliminalInstallResult {
    let (command, version) = PythonManager::resolve_subliminal(python);
    SubliminalInstallResult {
        installed: command.is_some(),
        version,
        command,
    }
}

fn install_subliminal_and_verify(
    python_command: Option<String>,
) -> Result<SubliminalInstallResult, String> {
    PythonManager::install_subliminal()?;
    PythonManager::add_scripts_to_path()
        .map_err(|e| format!("Subliminal installed, but failed to update PATH: {}", e))?;
    PythonManager::refresh_environment().map_err(|e| {
        format!(
            "Subliminal installed, but failed to refresh environment: {}",
            e
        )
    })?;

    let result = probe_subliminal(python_command.as_deref());
    if result.installed {
        Ok(result)
    } else {
        Err("Subliminal installation could not be verified".to_string())
    }
}

fn spawn_subliminal_install(
    result_ptr: Arc<Mutex<Option<Result<SubliminalInstallResult, String>>>>,
    python_command: Option<String>,
) {
    thread::spawn(move || {
        let result = install_subliminal_and_verify(python_command);
        *result_ptr.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
    });
}

fn verified_subliminal_state(
    result: Result<SubliminalInstallResult, String>,
) -> Result<
    (
        bool,
        Option<String>,
        Option<crate::python_manager::SubliminalCommand>,
    ),
    String,
> {
    result.map(|result| (result.installed, result.version, result.command))
}

impl SubtitleDownloader {
    pub fn new(ctx: &eframe::egui::Context) -> Self {
        info!("Initializing SubtitleDownloader");
        let mut settings = Settings::load();
        if !crate::credentials::stored_in_settings() {
            settings.opensubtitlescom_password = crate::credentials::load_password();
        }
        crate::notifications::ENABLED.store(settings.notify_desktop, Ordering::Relaxed);
        // Keep the startup entry pointing at this exe.
        if settings.background_service {
            if let Err(error) = crate::background::startup::register() {
                warn!("Could not refresh startup entry: {}", error);
            }
        }

        let scan_history = Arc::new(Mutex::new(ScanHistory::load()));
        let engine = crate::engine::Engine::start(Arc::clone(&scan_history));
        engine.watch.set_repaint_context(ctx);
        crate::notifications::set_context(ctx);

        // Check dependencies in the background; the engine gets Subliminal
        // straight away even if the window stays hidden.
        let (init_tx, init_rx) = mpsc::channel();
        let init_engine = Arc::clone(&engine);
        thread::spawn(move || {
            let result = probe_startup_dependencies();
            if result.subliminal_command.is_some() {
                init_engine.set_command(result.subliminal_command.clone());
            }
            let _ = init_tx.send(result);
        });

        Self {
            python_command: None,
            python_installed: false,
            python_version: None,
            pipx_installed: false,
            pipx_version: None,
            subliminal_installed: false,
            subliminal_version: None,
            subliminal_command: None,
            pushed_command: None,
            ffmpeg_installed: false,
            homebrew_installed: false,
            installing_python: false,
            installing_subliminal: false,
            python_install_result: Arc::new(Mutex::new(None)),
            subliminal_install_result: Arc::new(Mutex::new(None)),
            selected_languages: settings.selected_languages,
            skip_scanned_media: settings.skip_scanned_media,
            force_download: settings.force_download,
            overwrite_existing: settings.overwrite_existing,
            ignore_local_extras: settings.ignore_local_extras,
            concurrent_downloads: settings.concurrent_downloads,
            providers: settings.providers,
            refiners: settings.refiners,
            minimum_score: settings.minimum_score,
            exclude_hearing_impaired: settings.exclude_hearing_impaired,
            matching_options_open: settings.matching_options_open,
            opensubtitles_max_pages: settings.opensubtitles_max_pages,
            opensubtitlescom_username: settings.opensubtitlescom_username,
            opensubtitlescom_password: settings.opensubtitlescom_password,
            opensubtitlescom_apikey: settings.opensubtitlescom_apikey,
            show_opensubtitles_apikey: false,
            keep_dropdown_open: false,
            language_search: String::new(),
            status: "Checking dependencies...".to_string(),
            background_check_handle: None,
            background_check_receiver: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            latest_version: None,
            version_check_error: None,
            version_checked: false,
            startup_phase: StartupPhase::Checking,
            init_check_receiver: Some(init_rx),
            splash_started: std::time::Instant::now(),
            splash_dismiss_started: None,
            watch: engine.watch.clone(),
            engine,
            scan_history,
            watch_enabled: settings.watch_enabled,
            watch_folders: settings.watch_folders,
            watch_settle_seconds: settings.watch_settle_seconds,
            watch_catch_up: settings.watch_catch_up,
            watch_polling: settings.watch_polling,
            watch_poll_interval_seconds: settings.watch_poll_interval_seconds,
            watch_options_open: settings.watch_options_open,
            notify_desktop: settings.notify_desktop,
            background_service: settings.background_service,
            background_checked: false,
            updater: Arc::new(crate::updater::Updater::default()),
            check_updates: settings.check_updates,
            theme: crate::ff_theme::Theme::from_key(&settings.theme),
            green_in_page: false,
            green_cursor: 0,
            green_zoom: None,
            green_reveal: None,
            green_leave: None,
            green_unzoom: None,
            menu_page: MenuPage::Watch,
            menu_touched: false,
            advanced_open: false,
            bak_cleanup: Arc::new(Mutex::new(crate::data_structures::BakCleanup::Idle)),
        }
    }

    fn settings_snapshot(&self) -> Settings {
        Settings {
            selected_languages: self.selected_languages.clone(),
            skip_scanned_media: self.skip_scanned_media,
            force_download: self.force_download,
            overwrite_existing: self.overwrite_existing,
            ignore_local_extras: self.ignore_local_extras,
            concurrent_downloads: self.concurrent_downloads,
            providers: self.providers.clone(),
            refiners: self.refiners.clone(),
            minimum_score: self.minimum_score,
            exclude_hearing_impaired: self.exclude_hearing_impaired,
            matching_options_open: self.matching_options_open,
            opensubtitles_max_pages: self.opensubtitles_max_pages,
            opensubtitlescom_username: self.opensubtitlescom_username.clone(),
            opensubtitlescom_password: self.opensubtitlescom_password.clone(),
            opensubtitlescom_apikey: self.opensubtitlescom_apikey.clone(),
            watch_enabled: self.watch_enabled,
            watch_folders: self.watch_folders.clone(),
            watch_settle_seconds: self.watch_settle_seconds,
            watch_catch_up: self.watch_catch_up,
            watch_polling: self.watch_polling,
            watch_poll_interval_seconds: self.watch_poll_interval_seconds,
            watch_options_open: self.watch_options_open,
            notify_desktop: self.notify_desktop,
            background_service: self.background_service,
            check_updates: self.check_updates,
            theme: self.theme.key().to_string(),
        }
    }

    /// Save the current settings; they take effect straight away.
    pub fn save_current_settings(&self) {
        let settings = self.settings_snapshot();
        if let Err(e) = settings.save() {
            warn!("Failed to save settings: {}", e);
        } else {
            debug!("Settings saved successfully");
        }
        crate::notifications::ENABLED.store(self.notify_desktop, Ordering::Relaxed);
        self.engine.settings_changed();
    }

    /// Save the OpenSubtitles.com password (Credential Manager on Windows).
    pub fn save_password(&self) {
        if let Err(error) = crate::credentials::save_password(&self.opensubtitlescom_password) {
            warn!("{error}");
        }
        self.save_current_settings();
    }

    /// Turn "Start with Windows" on or off.
    pub fn set_start_with_windows(&mut self, enabled: bool) {
        let result = if enabled {
            crate::background::startup::register()
        } else {
            crate::background::startup::unregister()
        };
        match result {
            Ok(()) => {
                self.background_service = enabled;
                self.save_current_settings();
                info!("Start with Windows {}", if enabled { "on" } else { "off" });
            }
            Err(error) => {
                warn!("Couldn't change the startup entry: {error}");
            }
        }
    }

    /// Per-frame housekeeping: first-run tasks, and handing Subliminal to
    /// the engine once it's found or installed.
    pub fn sync_with_engine(&mut self, ctx: &eframe::egui::Context) {
        if !self.background_checked {
            self.background_checked = true;
            self.watch.set_repaint_context(ctx);
            crate::notifications::set_context(ctx);
            std::thread::spawn(crate::updater::cleanup_old_exe);
            if self.check_updates {
                crate::updater::check_in_background(Arc::clone(&self.updater), false, Some(ctx.clone()));
            }
        }
        if self.subliminal_command != self.pushed_command {
            self.pushed_command = self.subliminal_command.clone();
            self.engine.set_command(self.subliminal_command.clone());
        }
    }

    /// Handle the startup dependency check.
    pub fn poll_init_check(&mut self) {
        if self.startup_phase != StartupPhase::Checking {
            return;
        }

        let result = match self
            .init_check_receiver
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            Some(r) => r,
            None => return,
        };
        self.init_check_receiver = None;

        self.python_command = result.python_command;
        self.python_version = result.python_version;
        self.python_installed = self.python_version.is_some();
        self.pipx_installed = result.pipx_installed;
        self.pipx_version = result.pipx_version;
        self.subliminal_installed = result.subliminal_installed;
        self.subliminal_version = result.subliminal_version;
        self.subliminal_command = result.subliminal_command;
        self.ffmpeg_installed = result.ffmpeg_installed;
        self.homebrew_installed = result.homebrew_installed;

        if self.python_installed && self.pipx_installed && !self.subliminal_installed {
            info!("Starting automatic Subliminal installation");
            self.startup_phase = StartupPhase::Installing;
            self.installing_subliminal = true;
            self.status = "Python and pipx detected. Installing Subliminal...".to_string();
            let result_ptr = Arc::clone(&self.subliminal_install_result);
            spawn_subliminal_install(result_ptr, self.python_command.clone());
        } else {
            self.status = "Scanning will start automatically when a folder is selected".to_string();
            self.finish_initial_startup();
        }
    }

    fn finish_initial_startup(&mut self) {
        self.startup_phase = StartupPhase::Ready;
        self.start_background_check();
    }

    fn start_background_check(&mut self) {
        if self.background_check_handle.is_some() {
            return;
        }

        let (tx, rx) = mpsc::channel();
        let bg_shutdown = Arc::clone(&self.shutdown_flag);
        let bg = thread::spawn(move || {
            loop {
                if bg_shutdown.load(Ordering::Relaxed) {
                    return;
                }

                #[cfg(windows)]
                {
                    let result = probe_subliminal(None);
                    let sub = result.installed;
                    if tx.send((true, result, true, false)).is_err() {
                        break;
                    }
                    if sub {
                        break;
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    let pipx = PythonManager::_pipx_available();
                    let result = probe_subliminal(None);
                    let sub = result.installed;
                    let ffmpeg = PythonManager::is_ffmpeg_installed();
                    let homebrew = PythonManager::is_homebrew_installed();
                    if tx.send((pipx, result, ffmpeg, homebrew)).is_err() {
                        break;
                    }
                    if pipx && sub && ffmpeg {
                        break;
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    let pipx = PythonManager::_pipx_available();
                    let result = if pipx {
                        probe_subliminal(None)
                    } else {
                        SubliminalInstallResult::default()
                    };
                    let sub = result.installed;
                    let ffmpeg = PythonManager::is_ffmpeg_installed();
                    if tx.send((pipx, result, ffmpeg, false)).is_err() {
                        break;
                    }
                    if pipx && sub && ffmpeg {
                        break;
                    }
                }

                // Check shutdown while waiting.
                for _ in 0..50 {
                    if bg_shutdown.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        });
        self.background_check_handle = Some(bg);
        self.background_check_receiver = Some(rx);
    }

    /// Refresh installation status.
    pub fn refresh_installation_status(&mut self) {
        if !self.startup_phase.is_terminal() {
            return;
        }

        let mut last_status = None;
        if let Some(receiver) = &self.background_check_receiver {
            while let Ok(status) = receiver.try_recv() {
                last_status = Some(status);
            }
        }
        if let Some((_pipx_available, subliminal, ffmpeg_installed, homebrew_installed)) =
            last_status
        {
            let SubliminalInstallResult {
                installed: subliminal_installed,
                version,
                command,
            } = subliminal;
            self.subliminal_version = version;
            self.subliminal_command = command;
            let _old_pipx = self.pipx_installed;
            let old_subliminal = self.subliminal_installed;
            let old_ffmpeg = self.ffmpeg_installed;
            self.ffmpeg_installed = ffmpeg_installed;
            self.homebrew_installed = homebrew_installed;

            #[cfg(target_os = "linux")]
            {
                self.pipx_installed = _pipx_available;
            }
            #[cfg(windows)]
            {
                self.pipx_installed = true;
            }
            #[cfg(target_os = "macos")]
            {
                self.pipx_installed = _pipx_available;
            }

            #[cfg(any(windows, target_os = "macos"))]
            {
                if self.python_installed {
                    self.subliminal_installed = subliminal_installed;
                }
            }

            #[cfg(target_os = "linux")]
            {
                if self.python_installed && self.pipx_installed {
                    self.subliminal_installed = subliminal_installed;
                }
            }

            #[cfg(target_os = "linux")]
            {
                if !_old_pipx && self.pipx_installed && !self.subliminal_installed {
                    info!("pipx became available, starting automatic Subliminal installation");
                    self.status = "pipx detected! Installing Subliminal...".to_string();
                    self.installing_subliminal = true;
                    let result_ptr = Arc::clone(&self.subliminal_install_result);
                    spawn_subliminal_install(result_ptr, self.python_command.clone());
                }
            }

            if (!old_subliminal || !old_ffmpeg) && self.dependencies_ready() {
                info!("Subliminal became available");
                self.status =
                    "All dependencies installed. Ready to download subtitles.".to_string();
            }

            #[cfg(any(windows, target_os = "macos"))]
            {
                if self.dependencies_ready() {
                    self.shutdown_flag.store(true, Ordering::Relaxed);
                    self.background_check_handle = None;
                    self.background_check_receiver = None;
                }
            }

            #[cfg(target_os = "linux")]
            {
                if self.dependencies_ready() {
                    self.shutdown_flag.store(true, Ordering::Relaxed);
                    self.background_check_handle = None;
                    self.background_check_receiver = None;
                }
            }
        }
    }

    /// Handle installation states.
    pub fn handle_installation_states(&mut self) {
        if self.installing_python {
            if let Some(result) = self
                .python_install_result
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                self.installing_python = false;
                match result {
                    Ok(_) => {
                        info!("Python installation completed successfully");
                        if let Err(e) = PythonManager::refresh_environment() {
                            error!("Failed to refresh environment: {}", e);
                        }
                        if let Err(e) = PythonManager::add_scripts_to_path() {
                            error!("Failed to add Python Scripts to PATH: {}", e);
                        }
                        let python_info = PythonManager::get_python_info();
                        self.python_command =
                            python_info.as_ref().map(|(command, _)| command.clone());
                        self.python_version = python_info.map(|(_, version)| version);
                        self.python_installed = self.python_version.is_some();
                        self.status =
                            "Python installed successfully. Installing Subliminal...".to_string();
                        self.subliminal_installed = false;

                        self.installing_subliminal = true;
                        let result_ptr = Arc::clone(&self.subliminal_install_result);
                        spawn_subliminal_install(result_ptr, self.python_command.clone());
                    }
                    Err(e) => {
                        error!("Python installation failed: {}", e);
                        self.status = format!("Python install failed: {}", e);
                    }
                }
            }
        }

        if self.installing_subliminal {
            let result = self
                .subliminal_install_result
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(result) = result {
                let initial_startup_install = self.startup_phase == StartupPhase::Installing;
                self.installing_subliminal = false;
                match verified_subliminal_state(result) {
                    Ok((installed, version, command)) => {
                        info!("Subliminal installation completed successfully");
                        self.subliminal_installed = installed;
                        self.subliminal_version = version;
                        self.subliminal_command = command;
                        self.status = "Subliminal installed.".to_string();
                    }
                    Err(e) => {
                        error!("Subliminal installation failed: {}", e);
                        self.status = format!("Subliminal install failed: {}", e);
                    }
                }
                if initial_startup_install {
                    self.finish_initial_startup();
                }
            }
        }
    }

    /// Poll for update results.
    pub fn poll_version_check(&mut self) {
        if self.version_checked {
            return;
        }
        let lock = VERSION_PTR.lock().unwrap_or_else(|e| e.into_inner());
        if lock.2 {
            self.latest_version = lock.0.clone();
            self.version_check_error = lock.1.clone();
            self.version_checked = true;
        }
    }

    /// Check whether the current version is older.
    pub fn is_outdated(current: &str, latest: &str) -> bool {
        let parse = |s: &str| {
            s.trim_start_matches('v')
                .split('.')
                .map(|x| x.parse::<u32>().unwrap_or(0))
                .collect::<Vec<_>>()
        };
        let c = parse(current);
        let l = parse(latest);
        for (a, b) in c.iter().zip(l.iter()) {
            if a < b {
                return true;
            }
            if a > b {
                return false;
            }
        }
        c.len() < l.len() // e.g. 1.0 < 1.0.1
    }

    pub fn is_installing_python(&self) -> bool {
        self.installing_python
    }
    pub fn is_installing_subliminal(&self) -> bool {
        self.installing_subliminal
    }
    pub fn is_subliminal_installed(&self) -> bool {
        self.subliminal_installed
    }
    pub fn is_ffmpeg_installed(&self) -> bool {
        self.ffmpeg_installed
    }
    pub fn is_python_installed(&self) -> bool {
        self.python_installed
    }
    pub fn is_pipx_installed(&self) -> bool {
        self.pipx_installed
    }
    pub fn get_python_version(&self) -> Option<&String> {
        self.python_version.as_ref()
    }
    pub fn get_pipx_version(&self) -> Option<&String> {
        self.pipx_version.as_ref()
    }
    pub fn get_subliminal_version(&self) -> Option<&String> {
        self.subliminal_version.as_ref()
    }
    pub fn get_status(&self) -> &str {
        &self.status
    }
    pub fn get_latest_version(&self) -> Option<&String> {
        self.latest_version.as_ref()
    }
    pub fn get_version_check_error(&self) -> Option<&String> {
        self.version_check_error.as_ref()
    }
    pub fn is_version_checked(&self) -> bool {
        self.version_checked
    }
    pub fn is_startup_ready(&self) -> bool {
        self.startup_phase.is_terminal()
    }
    pub fn splash_min_elapsed(&self) -> bool {
        self.splash_started.elapsed() >= crate::config::MIN_SPLASH_DURATION
    }
    pub fn begin_splash_dismiss_if_ready(&mut self) {
        if self.splash_dismiss_started.is_none()
            && self.is_startup_ready()
            && self.splash_min_elapsed()
        {
            self.splash_dismiss_started = Some(std::time::Instant::now());
        }
    }
    pub fn splash_out_alpha(&self) -> f32 {
        let Some(started) = self.splash_dismiss_started else {
            return 1.0;
        };
        let elapsed = started.elapsed().as_secs_f32();
        let wait = crate::config::SPLASH_FADE_WAIT.as_secs_f32();
        let fade = crate::config::SPLASH_FADE_DURATION.as_secs_f32();
        if elapsed <= wait {
            1.0
        } else {
            (1.0 - (elapsed - wait) / fade).clamp(0.0, 1.0)
        }
    }
    pub fn can_show_main_ui(&self) -> bool {
        let Some(started) = self.splash_dismiss_started else {
            return false;
        };
        started.elapsed()
            >= crate::config::SPLASH_FADE_WAIT
                + crate::config::SPLASH_FADE_DURATION
                + crate::config::SPLASH_POST_FADE_WAIT
    }
    pub fn dependencies_ready(&self) -> bool {
        #[cfg(windows)]
        {
            self.python_installed && self.subliminal_installed
        }
        #[cfg(not(windows))]
        {
            self.python_installed && self.subliminal_installed && self.ffmpeg_installed
        }
    }

    pub fn set_installing_python(&mut self, installing: bool) {
        self.installing_python = installing;
    }
    pub fn set_python_install_result(&mut self, result: Arc<Mutex<Option<Result<(), String>>>>) {
        self.python_install_result = result;
    }
    pub fn set_keep_dropdown_open(&mut self, open: bool) {
        self.keep_dropdown_open = open;
    }
    pub fn get_keep_dropdown_open(&self) -> bool {
        self.keep_dropdown_open
    }
    pub fn set_matching_options_open(&mut self, open: bool) {
        self.matching_options_open = open;
    }
    pub fn get_matching_options_open(&self) -> bool {
        self.matching_options_open
    }

    pub fn get_selected_languages_mut(&mut self) -> &mut Vec<String> {
        &mut self.selected_languages
    }
    pub fn get_skip_scanned_media_mut(&mut self) -> &mut bool {
        &mut self.skip_scanned_media
    }
    pub fn get_force_download_mut(&mut self) -> &mut bool {
        &mut self.force_download
    }
    pub fn get_overwrite_existing(&self) -> bool {
        self.overwrite_existing
    }
    pub fn get_overwrite_existing_mut(&mut self) -> &mut bool {
        &mut self.overwrite_existing
    }
    pub fn get_ignore_local_extras(&self) -> bool {
        self.ignore_local_extras
    }
    pub fn get_ignore_local_extras_mut(&mut self) -> &mut bool {
        &mut self.ignore_local_extras
    }
    pub fn get_concurrent_downloads_mut(&mut self) -> &mut usize {
        &mut self.concurrent_downloads
    }
    pub fn get_shutdown_flag(&self) -> &Arc<AtomicBool> {
        &self.shutdown_flag
    }

    /// Start Subliminal installation.
    pub fn start_subliminal_install(&mut self) {
        if self.installing_subliminal || !self.python_installed {
            return;
        }

        self.installing_subliminal = true;
        self.status = "Installing Subliminal...".to_string();
        *self
            .subliminal_install_result
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        let result_ptr = Arc::clone(&self.subliminal_install_result);
        spawn_subliminal_install(result_ptr, self.python_command.clone());
    }

    /// Start Python installation on Windows.
    #[cfg(windows)]
    pub fn start_python_install(&mut self) {
        if self.installing_python {
            return;
        }
        self.installing_python = true;
        self.status =
            "Installing Python... Check your taskbar for a UAC prompt (shield icon)".to_string();
        let result_ptr = self.python_install_result.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let installer = crate::python_manager::PythonManager::download_installer()
                    .map_err(|e| format!("Failed to download installer: {}", e))?;
                let ok = crate::python_manager::PythonManager::install_silent(&installer)
                    .map_err(|e| format!("Failed to run installer: {}", e))?;
                if ok {
                    Ok(())
                } else {
                    Err("Installer did not complete successfully".to_string())
                }
            })();
            *result_ptr.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::verified_subliminal_state;
    use crate::data_structures::SubliminalInstallResult;

    #[test]
    fn verified_install_result_preserves_version() {
        let state = verified_subliminal_state(Ok(SubliminalInstallResult {
            installed: true,
            version: Some("subliminal 2.4.0".to_string()),
            command: None,
        }))
        .expect("verified installation should produce dependency state");

        assert_eq!(state, (true, Some("subliminal 2.4.0".to_string()), None));
    }
}
