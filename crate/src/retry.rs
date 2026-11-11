//! Remembers videos whose subtitles weren't available yet, so they're tried
//! again later on a schedule instead of on every start.
//!
//! Saved as `jenova_retry.json` next to the settings. Also records when the
//! last full check of the watched folders ran, so it happens at most once a
//! day.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{debug, warn};

/// Waits after the 1st, 2nd, 3rd… miss. After the last one, weekly.
const SCHEDULE_HOURS: &[u64] = &[6, 24, 72, 168];

/// How often the full check of watched folders runs.
pub const FULL_CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub attempts: u32,
    /// Unix seconds.
    pub next_try: u64,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct RetryBook {
    #[serde(default)]
    entries: HashMap<String, Entry>,
    /// Unix seconds of the last full check.
    #[serde(default)]
    last_full_check: u64,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    dirty: bool,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// Wait before the next try after `attempts` misses.
pub fn wait_after(attempts: u32) -> Duration {
    let index = (attempts.max(1) as usize - 1).min(SCHEDULE_HOURS.len() - 1);
    Duration::from_secs(SCHEDULE_HOURS[index] * 3600)
}

/// "6 h", "1 day", "3 days", "1 week".
pub fn describe(wait: Duration) -> String {
    let hours = wait.as_secs() / 3600;
    match hours {
        h if h < 24 => format!("{h} h"),
        24 => "1 day".into(),
        168 => "1 week".into(),
        h => format!("{} days", h / 24),
    }
}

impl RetryBook {
    pub fn load() -> Self {
        let path = crate::settings::Settings::get_path()
            .ok()
            .map(|settings| settings.with_file_name("jenova_retry.json"));
        let mut book: RetryBook = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        book.path = path;
        book
    }

    #[cfg(test)]
    fn in_memory() -> Self {
        Self::default()
    }

    pub fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = &self.path else {
            self.dirty = false;
            return;
        };
        match serde_json::to_vec(self) {
            Ok(bytes) => {
                if let Err(error) = crate::helper_functions::Utils::write_atomic(path, &bytes) {
                    warn!("Couldn't save the retry list: {error}");
                } else {
                    self.dirty = false;
                }
            }
            Err(error) => {
                warn!("Couldn't save the retry list: {error}");
            }
        }
    }

    /// Whether `path` should be tried now. A video that changed (new size)
    /// since it was last tried is always due.
    pub fn is_due(&self, path: &Path) -> bool {
        let Some(entry) = self.entries.get(&key(path)) else {
            return true;
        };
        if file_size(path).is_some_and(|size| size != entry.size) {
            return true;
        }
        now() >= entry.next_try
    }

    /// Nothing was found for `path`; returns the wait before the next try.
    pub fn record_miss(&mut self, path: &Path) -> Duration {
        let size = file_size(path).unwrap_or(0);
        let entry = self.entries.entry(key(path)).or_insert(Entry {
            attempts: 0,
            next_try: 0,
            size,
        });
        if entry.size != size {
            entry.attempts = 0;
            entry.size = size;
        }
        entry.attempts += 1;
        let wait = wait_after(entry.attempts);
        entry.next_try = now() + wait.as_secs();
        self.dirty = true;
        debug!("Retry {} in {}", path.display(), describe(wait));
        wait
    }

    /// Subtitles are there now (or the video is gone): stop tracking it.
    pub fn forget(&mut self, path: &Path) {
        if self.entries.remove(&key(path)).is_some() {
            self.dirty = true;
        }
    }

    /// Videos whose next try has come, dropping ones that no longer exist.
    pub fn take_due(&mut self) -> Vec<PathBuf> {
        let now = now();
        let mut due = Vec::new();
        let mut gone = Vec::new();
        for (name, entry) in &mut self.entries {
            if entry.next_try > now {
                continue;
            }
            let path = PathBuf::from(name);
            if !path.is_file() {
                // Deleted, unless its whole folder is unreachable (a network
                // drive that's offline): then just wait.
                if path.parent().is_some_and(Path::is_dir) {
                    gone.push(name.clone());
                }
                continue;
            }
            // Not due again until this try reports back.
            entry.next_try = now + 3600;
            due.push(path);
        }
        for name in gone {
            self.entries.remove(&name);
        }
        if !due.is_empty() {
            self.dirty = true;
        }
        due
    }

    pub fn waiting(&self) -> usize {
        self.entries.len()
    }

    pub fn full_check_due(&self) -> bool {
        now().saturating_sub(self.last_full_check) >= FULL_CHECK_EVERY.as_secs()
    }

    pub fn mark_full_check(&mut self) {
        self.last_full_check = now();
        self.dirty = true;
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_backs_off_then_stays_weekly() {
        assert_eq!(wait_after(1), Duration::from_secs(6 * 3600));
        assert_eq!(wait_after(2), Duration::from_secs(24 * 3600));
        assert_eq!(wait_after(3), Duration::from_secs(72 * 3600));
        assert_eq!(wait_after(4), Duration::from_secs(168 * 3600));
        assert_eq!(wait_after(40), Duration::from_secs(168 * 3600));
        assert_eq!(describe(wait_after(1)), "6 h");
        assert_eq!(describe(wait_after(2)), "1 day");
        assert_eq!(describe(wait_after(3)), "3 days");
        assert_eq!(describe(wait_after(4)), "1 week");
    }

    #[test]
    fn misses_are_not_due_until_their_time() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("a.mkv");
        std::fs::write(&video, b"1234").unwrap();
        let mut book = RetryBook::in_memory();
        assert!(book.is_due(&video));
        book.record_miss(&video);
        assert!(!book.is_due(&video));
        assert!(book.take_due().is_empty());
        // A changed file is due straight away.
        std::fs::write(&video, b"123456").unwrap();
        assert!(book.is_due(&video));
        book.forget(&video);
        assert!(book.is_due(&video));
        assert_eq!(book.waiting(), 0);
    }

    #[test]
    fn due_entries_are_handed_out_once_and_gone_files_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("a.mkv");
        std::fs::write(&video, b"1234").unwrap();
        let mut book = RetryBook::in_memory();
        book.record_miss(&video);
        book.entries.get_mut(&key(&video)).unwrap().next_try = 0;
        book.entries.insert(
            key(&dir.path().join("gone.mkv")),
            Entry { attempts: 1, next_try: 0, size: 1 },
        );
        assert_eq!(book.take_due(), vec![video.clone()]);
        assert!(book.take_due().is_empty());
        assert_eq!(book.waiting(), 1);
    }

    #[test]
    fn full_check_runs_once_a_day() {
        let mut book = RetryBook::in_memory();
        assert!(book.full_check_due());
        book.mark_full_check();
        assert!(!book.full_check_due());
    }
}
