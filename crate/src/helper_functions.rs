//! Utility functions and validation helpers.

use crate::config::{EXTRAS_FOLDER_NAMES, MAX_CONCURRENT_DOWNLOADS, VIDEO_EXTENSIONS};
use std::path::Path;

/// Common application utilities.
pub struct Utils;

impl Utils {
    pub fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("File has no parent directory"))?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(content)?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    /// Get a file name, or `Unknown` when it is unavailable.
    pub fn get_file_name(path: &Path) -> String {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Unknown")
            .to_string()
    }

    /// Truncate a string to a maximum length.
    pub fn truncate_string(s: &str, max_len: usize) -> String {
        if s.chars().count() <= max_len {
            s.to_string()
        } else if max_len <= 3 {
            s.chars().take(max_len).collect()
        } else {
            let prefix: String = s.chars().take(max_len - 3).collect();
            format!("{prefix}...")
        }
    }

    /// Check whether a path is a video file.
    pub fn is_video_file(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                VIDEO_EXTENSIONS
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(ext))
            })
    }

    /// Check whether a directory is a Plex local-extras folder.
    pub fn is_plex_extras_folder(name: &str) -> bool {
        EXTRAS_FOLDER_NAMES
            .iter()
            .any(|folder| folder.eq_ignore_ascii_case(name))
    }

    /// Format download progress as a percentage.
    pub fn format_progress(current: usize, total: usize) -> String {
        if total == 0 {
            "0%".to_string()
        } else {
            let percentage = (current as f32 / total as f32 * 100.0) as usize;
            format!("{}%", percentage)
        }
    }

    /// %APPDATA%\\Jenova: every file Jenova writes lives here, whichever
    /// folder the exe runs from. Created if missing.
    #[cfg(windows)]
    pub fn app_data_dir() -> std::io::Result<std::path::PathBuf> {
        let base = dirs::config_dir().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "No AppData folder")
        })?;
        let dir = base.join("Jenova");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    #[cfg(windows)]
    pub fn shared_data_file(name: &str) -> std::io::Result<std::path::PathBuf> {
        Ok(Self::app_data_dir()?.join(name))
    }

    /// Move files older versions left next to the exe into %APPDATA%\\Jenova.
    #[cfg(windows)]
    pub fn migrate_legacy_files() {
        if let (Ok(data_dir), Some(exe_dir)) = (
            Self::app_data_dir(),
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf)),
        ) {
            Self::move_legacy_files(&exe_dir, &data_dir);
        }
    }

    /// Move Jenova's data files from `from` into `to`. If `to` already has a
    /// file, it wins and the leftover in `from` is deleted.
    pub fn move_legacy_files(from: &std::path::Path, to: &std::path::Path) {
        const FILES: &[&str] = &[
            "jenova_settings.json",
            "jenova_settings.json.bak",
            "jenova_scan_history.json",
            "jenova_log.txt",
            "jenova_background_log.txt",
            "jenova_gui.lock",
            "jenova_watcher.lock",
        ];
        let same = from
            .canonicalize()
            .ok()
            .zip(to.canonicalize().ok())
            .is_some_and(|(a, b)| a == b);
        if same {
            return;
        }
        for name in FILES {
            let legacy = from.join(name);
            if !legacy.is_file() {
                continue;
            }
            let target = to.join(name);
            if target.exists() {
                let _ = std::fs::remove_file(&legacy);
            } else if std::fs::rename(&legacy, &target).is_err()
                && std::fs::copy(&legacy, &target).is_ok()
            {
                let _ = std::fs::remove_file(&legacy);
            }
        }
    }

    /// Get the log file path.
    pub fn get_log_path() -> Result<std::path::PathBuf, String> {
        #[cfg(windows)]
        {
            Ok(Self::app_data_dir()
                .map_err(|e| e.to_string())?
                .join("jenova_log.txt"))
        }
        #[cfg(not(windows))]
        {
            let xdg_dirs = xdg::BaseDirectories::new();
            if let Some(cache_dir) = xdg_dirs.get_cache_home() {
                Ok(cache_dir.join("jenova").join("jenova.log"))
            } else {
                let home_dir = dirs::home_dir().ok_or("Failed to get home directory")?;
                Ok(home_dir.join(".jenova").join("jenova.log"))
            }
        }
    }

    /// Open the log file in the file browser.
    pub fn open_log_file() -> Result<(), String> {
        let log_path = Self::get_log_path()?;
        if !log_path.exists() {
            return Err("Log file does not exist yet".to_string());
        }
        Self::open_containing_folder(&log_path)
    }

    /// Open a folder in the system file manager.
    pub fn open_folder(folder: &str) -> Result<(), String> {
        #[cfg(windows)]
        let program = "explorer.exe";
        #[cfg(target_os = "macos")]
        let program = "open";
        #[cfg(all(unix, not(target_os = "macos")))]
        let program = "xdg-open";
        std::process::Command::new(program)
            .arg(folder)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Open a file's containing folder.
    pub fn open_containing_folder(path: &Path) -> Result<(), String> {
        let _folder = path.parent().ok_or("No parent folder")?;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let canonical = path.canonicalize().map_err(|e| e.to_string())?;
            let path_str = canonical.to_string_lossy().replace("/", "\\");
            let mut cmd = std::process::Command::new("explorer.exe");
            cmd.arg("/select,").arg(&path_str);
            cmd.creation_flags(0x08000000); // Hide the console.
            cmd.spawn().map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "linux")]
        {
            let canonical = path
                .parent()
                .ok_or("No parent folder")?
                .canonicalize()
                .map_err(|e| e.to_string())?;
            let status = std::process::Command::new("xdg-open")
                .arg(canonical)
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("xdg-open failed: {:?}", status));
            }
        }
        #[cfg(target_os = "macos")]
        {
            let canonical = path.canonicalize().map_err(|e| e.to_string())?;
            let status = std::process::Command::new("open")
                .arg("-R")
                .arg(&canonical)
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("open -R failed: {:?}", status));
            }
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            return Err("Open folder not supported on this OS".to_string());
        }
        Ok(())
    }
}

/// Input validation utilities.
pub struct Validation;

impl Validation {
    /// Check whether a path is an existing folder.
    pub fn is_valid_folder(path: &str) -> bool {
        if path.is_empty() {
            return false;
        }

        let path = Path::new(path);
        path.exists() && path.is_dir()
    }

    /// Check whether a concurrent download count is valid.
    pub fn is_valid_concurrent_downloads(value: usize) -> bool {
        value > 0 && value <= MAX_CONCURRENT_DOWNLOADS
    }
}

#[cfg(test)]
mod tests {
    use super::Utils;
    use std::path::Path;

    #[test]
    fn atomic_write_replaces_complete_files_and_uses_private_permissions() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("settings.json");
        Utils::write_atomic(&path, b"first").unwrap();
        Utils::write_atomic(&path, b"replacement").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn truncate_string_respects_character_boundaries() {
        assert_eq!(Utils::truncate_string("字幕文件名称", 5), "字幕...");
        assert_eq!(Utils::truncate_string("abcdef", 3), "abc");
        assert_eq!(Utils::truncate_string("abcdef", 0), "");
    }

    #[test]
    fn is_video_file_accepts_mkv_and_rejects_m4a() {
        assert!(Utils::is_video_file(Path::new("show.mkv")));
        assert!(Utils::is_video_file(Path::new("SHOW.MKV")));
        assert!(!Utils::is_video_file(Path::new("track.m4a")));
        assert!(!Utils::is_video_file(Path::new("audiobook.m4b")));
        assert!(!Utils::is_video_file(Path::new("ringtone.m4r")));
    }

    #[test]
    fn plex_extras_folder_match_is_case_insensitive() {
        assert!(Utils::is_plex_extras_folder("Behind The Scenes"));
        assert!(Utils::is_plex_extras_folder("behind the scenes"));
        assert!(Utils::is_plex_extras_folder("FEATURETTES"));
        assert!(!Utils::is_plex_extras_folder("Season 01"));
    }
}

#[cfg(test)]
mod legacy_file_tests {
    use super::Utils;

    #[test]
    fn leftovers_move_into_data_folder_and_existing_data_wins() {
        let exe_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        std::fs::write(exe_dir.path().join("jenova_log.txt"), "old log").unwrap();
        std::fs::write(exe_dir.path().join("jenova_settings.json"), "stale").unwrap();
        std::fs::write(data_dir.path().join("jenova_settings.json"), "current").unwrap();
        std::fs::write(exe_dir.path().join("jenova.exe"), "program").unwrap();

        Utils::move_legacy_files(exe_dir.path(), data_dir.path());

        assert_eq!(
            std::fs::read_to_string(data_dir.path().join("jenova_log.txt")).unwrap(),
            "old log"
        );
        assert_eq!(
            std::fs::read_to_string(data_dir.path().join("jenova_settings.json")).unwrap(),
            "current"
        );
        assert!(!exe_dir.path().join("jenova_log.txt").exists());
        assert!(!exe_dir.path().join("jenova_settings.json").exists());
        // Only Jenova's data files are touched.
        assert!(exe_dir.path().join("jenova.exe").exists());
    }
}
