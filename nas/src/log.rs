//! Small logger: a capped file in the var folder plus an in-memory tail for
//! the Log page.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_LINES: usize = 400;

struct Logger {
    path: PathBuf,
    lines: VecDeque<String>,
}

static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();

pub fn init(path: PathBuf) {
    let _ = LOGGER.set(Mutex::new(Logger {
        path,
        lines: VecDeque::new(),
    }));
}

/// Local time as "YYYY-MM-DD HH:MM:SS" using the system's zone offset.
pub fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = now + local_offset_seconds();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

/// Offset from /etc/localtime via the TZ-aware `date` output, cached.
fn local_offset_seconds() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let s = s.trim();
                let sign = if s.starts_with('-') { -1 } else { 1 };
                let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
                if digits.len() != 4 {
                    return None;
                }
                let h: i64 = digits[..2].parse().ok()?;
                let m: i64 = digits[2..].parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            })
            .unwrap_or(0)
    })
}

/// Days since 1970-01-01 to (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn write(level: &str, message: &str) {
    let line = format!("{} [{level}] {message}", timestamp());
    let Some(logger) = LOGGER.get() else {
        eprintln!("{line}");
        return;
    };
    let mut logger = logger.lock().unwrap_or_else(|e| e.into_inner());
    if level != "DEBUG" {
        logger.lines.push_back(line.clone());
        while logger.lines.len() > MAX_LINES {
            logger.lines.pop_front();
        }
    }
    if std::fs::metadata(&logger.path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
        let _ = std::fs::rename(&logger.path, logger.path.with_extension("log.1"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&logger.path)
    {
        let _ = writeln!(file, "{line}");
    }
}

pub fn recent() -> Vec<String> {
    LOGGER
        .get()
        .map(|l| {
            l.lock()
                .unwrap_or_else(|e| e.into_inner())
                .lines
                .iter()
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::log::write("INFO", &format!($($arg)*)) };
}
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::log::write("WARN", &format!($($arg)*)) };
}
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => { $crate::log::write("DEBUG", &format!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn civil_dates() {
        assert_eq!(super::civil_from_days(0), (1970, 1, 1));
        assert_eq!(super::civil_from_days(20_765), (2026, 11, 8));
    }
}
