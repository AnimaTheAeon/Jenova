//! Settings, stored as JSON in the package's var folder.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "mpeg", "mpg", "webm", "m4v", "3gp", "3g2", "asf",
    "mts", "m2ts", "ts", "vob", "ogv", "rm", "rmvb", "divx", "f4v", "mxf", "mp2", "mpv", "dat",
    "tod", "vro", "drc", "mng", "qt", "yuv", "viv", "amv", "nsv", "svi", "mpe", "mpv2", "m2v",
    "m1v", "m2p", "trp", "tp", "ps", "evo", "ogm", "ogx", "mod", "rec", "dvr-ms", "pva", "wtv",
    "m4p", "3gpp", "3gpp2",
];

/// Plex local-extras folder names.
pub const EXTRAS_FOLDER_NAMES: &[&str] = &[
    "Behind The Scenes",
    "Deleted Scenes",
    "Featurettes",
    "Interviews",
    "Scenes",
    "Shorts",
    "Trailers",
    "Other",
];

/// Folders DSM and file-sharing tools create that never hold real media.
pub const SYSTEM_FOLDER_NAMES: &[&str] = &[
    "@eaDir",
    "#recycle",
    "#snapshot",
    "@tmp",
    "@sharesnap",
    "@Recycle",
    ".@__thumb",
    "@SynoResource",
    "@SynoEAStream",
    ".AppleDouble",
    ".Trashes",
];

pub const PROVIDERS: &[&str] = &[
    "addic7ed",
    "gestdown",
    "napiprojekt",
    "opensubtitles",
    "opensubtitlescom",
    "podnapisi",
    "tvsubtitles",
];
pub const REFINERS: &[&str] = &["hash", "metadata", "tmdb", "tvdb"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    pub watch_enabled: bool,
    pub folders: Vec<String>,
    pub languages: Vec<String>,
    pub exclude_hearing_impaired: bool,
    /// Download even when the video has embedded subtitles.
    pub ignore_embedded: bool,
    pub ignore_extras: bool,
    pub providers: Vec<String>,
    pub refiners: Vec<String>,
    pub minimum_score: u8,
    pub opensubtitles_username: String,
    pub opensubtitles_password: String,
    pub opensubtitles_apikey: String,
    /// A new file must stop changing for this long before it's processed.
    pub settle_seconds: u64,
    /// Check existing files when watching starts.
    pub catch_up: bool,
    /// Poll instead of using inotify (always used as a fallback if needed).
    pub polling: bool,
    pub poll_interval_seconds: u64,
    pub concurrent_downloads: usize,
    /// Retry videos where nothing was found after this many hours.
    pub retry_hours: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watch_enabled: true,
            folders: Vec::new(),
            languages: vec!["en".into()],
            exclude_hearing_impaired: false,
            ignore_embedded: true,
            ignore_extras: true,
            providers: PROVIDERS.iter().map(|p| p.to_string()).collect(),
            refiners: REFINERS.iter().map(|r| r.to_string()).collect(),
            minimum_score: 0,
            opensubtitles_username: String::new(),
            opensubtitles_password: String::new(),
            opensubtitles_apikey: String::new(),
            settle_seconds: 60,
            catch_up: true,
            polling: false,
            poll_interval_seconds: 300,
            concurrent_downloads: 2,
            retry_hours: 24,
        }
    }
}

impl Config {
    /// Clamp values and drop unknown entries.
    pub fn normalize(&mut self) {
        self.settle_seconds = self.settle_seconds.clamp(5, 3600);
        self.poll_interval_seconds = self.poll_interval_seconds.clamp(30, 86_400);
        self.concurrent_downloads = self.concurrent_downloads.clamp(1, 8);
        self.minimum_score = self.minimum_score.min(100);
        self.retry_hours = self.retry_hours.clamp(1, 24 * 30);
        self.providers.retain(|p| PROVIDERS.contains(&p.as_str()));
        self.refiners.retain(|r| REFINERS.contains(&r.as_str()));
        self.languages.retain(|l| {
            !l.is_empty() && l.len() <= 8 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
        let mut seen = std::collections::HashSet::new();
        self.folders.retain(|f| f.starts_with('/') && seen.insert(f.trim_end_matches('/').to_string()));
        for folder in &mut self.folders {
            while folder.len() > 1 && folder.ends_with('/') {
                folder.pop();
            }
        }
    }

    pub fn load(path: &Path) -> Self {
        let mut config: Config = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        config.normalize();
        config
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(path, &json)
    }

    /// Subliminal TOML config with the OpenSubtitles.com credentials.
    pub fn subliminal_toml(&self) -> String {
        let mut out = String::from("[download]\nlanguage_format = \"alpha2\"\n");
        let creds = [
            ("username", &self.opensubtitles_username),
            ("password", &self.opensubtitles_password),
            ("apikey", &self.opensubtitles_apikey),
        ];
        if creds.iter().any(|(_, v)| !v.is_empty()) {
            out.push_str("\n[provider.opensubtitlescom]\n");
            for (key, value) in creds {
                if !value.is_empty() {
                    out.push_str(&format!("{key} = {}\n", toml_string(value)));
                }
            }
        }
        out
    }
}

fn toml_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

pub fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(ext)))
}

pub fn is_extras_folder(name: &str) -> bool {
    EXTRAS_FOLDER_NAMES.iter().any(|f| f.eq_ignore_ascii_case(name))
}

pub fn is_system_folder(name: &str) -> bool {
    SYSTEM_FOLDER_NAMES.iter().any(|f| f.eq_ignore_ascii_case(name))
        || (name.starts_with('.') && name.len() > 1)
}

/// Whether any ancestor directory of `path` should be skipped.
pub fn in_skipped_folder(path: &Path, ignore_extras: bool) -> bool {
    path.ancestors()
        .skip(1)
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .any(|n| is_system_folder(n) || (ignore_extras && is_extras_folder(n)))
}

/// Package data folder (/var/packages/Jenova/var on DSM).
pub fn var_dir() -> PathBuf {
    std::env::var_os("SYNOPKG_PKGVAR")
        .or_else(|| std::env::var_os("JENOVA_VAR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/packages/Jenova/var"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_cleans_input() {
        let mut c = Config {
            folders: vec!["/volume1/video/".into(), "/volume1/video".into(), "relative".into()],
            providers: vec!["podnapisi".into(), "bogus".into()],
            languages: vec!["en".into(), "bad lang!".into()],
            settle_seconds: 1,
            concurrent_downloads: 99,
            ..Config::default()
        };
        c.normalize();
        assert_eq!(c.folders, vec!["/volume1/video"]);
        assert_eq!(c.providers, vec!["podnapisi"]);
        assert_eq!(c.languages, vec!["en"]);
        assert_eq!(c.settle_seconds, 5);
        assert_eq!(c.concurrent_downloads, 8);
    }

    #[test]
    fn synology_and_extras_folders_are_skipped() {
        assert!(in_skipped_folder(Path::new("/volume1/video/Movie/@eaDir/Movie.mkv"), true));
        assert!(in_skipped_folder(Path::new("/volume1/video/#recycle/x.mkv"), false));
        assert!(in_skipped_folder(Path::new("/volume1/video/Film/Featurettes/a.mkv"), true));
        assert!(!in_skipped_folder(Path::new("/volume1/video/Film/Featurettes/a.mkv"), false));
        assert!(!in_skipped_folder(Path::new("/volume1/video/Show/Season 01/e.mkv"), true));
    }

    #[test]
    fn credentials_are_escaped_in_toml() {
        let c = Config {
            opensubtitles_username: "me".into(),
            opensubtitles_password: "p\"a\\ss".into(),
            ..Config::default()
        };
        let toml = c.subliminal_toml();
        assert!(toml.contains("username = \"me\""));
        assert!(toml.contains("password = \"p\\\"a\\\\ss\""));
        assert!(!toml.contains("apikey"));
    }
}
