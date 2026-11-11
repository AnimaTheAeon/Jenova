//! Downloading subtitles for one video with Subliminal.
//!
//! [`process_video`] runs Subliminal for a single file and works out what
//! happened: subtitles found, nothing available yet, already there, or an
//! error. The watcher's queue (see `watch.rs`) decides when and how many run.

use std::collections::HashSet;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::data_structures::JobStatus;
use crate::helper_functions::Utils;
use crate::python_manager::{PythonManager, SubliminalCommand};
use crate::scan_history::ScanHistory;
use crate::settings::Settings;
use crate::subtitle_utils::SubtitleUtils;
use crate::{error, info, warn};

const MAX_STATUS_BUFFER_BYTES: usize = 64 * 1024;

fn latest_non_empty_line(output: &str) -> Option<&str> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

fn next_char_at(s: &str, idx: usize) -> Option<char> {
    s[idx..].chars().next()
}

fn redact_sensitive(text: &str) -> String {
    let mut out = text.to_string();
    for key in ["apikey", "api_key", "password", "token"] {
        let mut search_from = 0;
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(pos) = lower[search_from..].find(key) else {
                break;
            };
            let key_start = search_from + pos;
            let after_key = key_start + key.len();
            if let Some(eq_rel) = out[after_key..].find(['=', ':']) {
                let eq_pos = after_key + eq_rel;
                let mut value_start = eq_pos + 1;
                while value_start < out.len() {
                    let Some(character) = next_char_at(&out, value_start) else {
                        break;
                    };
                    if !character.is_whitespace() {
                        break;
                    }
                    value_start += character.len_utf8();
                }

                if value_start >= out.len() {
                    search_from = after_key;
                    continue;
                }

                let delimiter = match next_char_at(&out, value_start) {
                    Some(character) => character,
                    None => {
                        search_from = after_key;
                        continue;
                    }
                };
                let (start, end) = if matches!(delimiter, '"' | '\'') {
                    let start = value_start + delimiter.len_utf8();
                    let mut end = start;
                    let mut escaped = false;
                    let mut closed = false;
                    while end < out.len() {
                        let Some(character) = next_char_at(&out, end) else {
                            break;
                        };
                        end += character.len_utf8();
                        if escaped {
                            escaped = false;
                        } else if character == '\\' {
                            escaped = true;
                        } else if character == delimiter {
                            closed = true;
                            break;
                        }
                    }
                    let end = if closed {
                        end - delimiter.len_utf8()
                    } else {
                        out.len()
                    };
                    (start, end)
                } else {
                    let start = value_start;
                    let mut end = start;
                    while end < out.len() {
                        let Some(character) = next_char_at(&out, end) else {
                            break;
                        };
                        if character.is_whitespace() || matches!(character, '"' | '\'' | ',') {
                            break;
                        }
                        end += character.len_utf8();
                    }
                    (start, end)
                };
                if end > start {
                    out.replace_range(start..end, "***");
                    search_from = start + 3;
                } else {
                    search_from = end;
                }
            } else {
                search_from = after_key;
            }
        }
    }
    out
}

fn subtitle_file_signature(path: &Path) -> Option<(u64, u128)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some((metadata.len(), modified.as_nanos()))
}

fn subliminal_config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        dirs::data_local_dir().map(|p| {
            p.join("subliminal")
                .join("subliminal")
                .join("subliminal.toml")
        })
    }
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|p| {
            p.join("Library")
                .join("Application Support")
                .join("subliminal")
                .join("subliminal.toml")
        })
    }
    #[cfg(target_os = "linux")]
    {
        xdg::BaseDirectories::new()
            .get_config_home()
            .map(|p| p.join("subliminal").join("subliminal.toml"))
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Write the OpenSubtitles.com username and API key to Subliminal's config.
/// The password is never stored there; it only goes into each run's
/// temporary config (see [`session_config`]).
fn sync_credentials_at(path: &Path, username: &str, apikey: &str) -> Result<String, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("Could not read Subliminal configuration: {error}")),
    };
    let mut document = content.parse::<toml_edit::DocumentMut>().map_err(|_| {
        "Invalid Subliminal configuration; the existing file was not changed".to_string()
    })?;
    if document.get("provider").is_none() {
        document["provider"] = toml_edit::table();
    }
    let providers = document
        .get_mut("provider")
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or("Subliminal provider configuration must be a table")?;
    if !providers.contains_key("opensubtitlescom") {
        providers.insert("opensubtitlescom", toml_edit::table());
    }
    let provider = providers
        .get_mut("opensubtitlescom")
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or("OpenSubtitles.com configuration must be a table")?;
    provider.remove("password");
    for (name, value) in [("username", username), ("apikey", apikey)] {
        if value.is_empty() {
            provider.remove(name);
        } else {
            provider.insert(name, toml_edit::value(value));
        }
    }
    let rewritten = document.to_string();
    let parent = path
        .parent()
        .ok_or("Subliminal configuration has no parent directory")?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Could not create config directory: {e}"))?;
    Utils::write_atomic(path, rewritten.as_bytes())
        .map_err(|e| format!("Could not save Subliminal configuration: {e}"))?;
    Ok(rewritten)
}

pub(crate) fn sync_subliminal_credentials(username: &str, apikey: &str) -> Result<String, String> {
    let path =
        subliminal_config_path().ok_or("Could not resolve the Subliminal configuration path")?;
    sync_credentials_at(&path, username, apikey)
}

fn session_config(content: &str, password: &str) -> Result<String, String> {
    let mut document = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| "Invalid Subliminal configuration".to_string())?;
    if document.get("download").is_none() {
        document["download"] = toml_edit::table();
    }
    let download = document
        .get_mut("download")
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or("Subliminal download configuration must be a table")?;
    // These defaults must not override the independent GUI controls or sidecar location.
    for name in [
        "force",
        "force_external_subtitles",
        "force_embedded_subtitles",
        "single",
    ] {
        download.insert(name, toml_edit::value(false));
    }
    download.remove("directory");
    download.insert("language_format", toml_edit::value("alpha2"));
    if !password.is_empty() {
        if document.get("provider").is_none() {
            document["provider"] = toml_edit::table();
        }
        let provider = document["provider"]
            .as_table_like_mut()
            .ok_or("Subliminal provider configuration must be a table")?;
        if !provider.contains_key("opensubtitlescom") {
            provider.insert("opensubtitlescom", toml_edit::table());
        }
        provider
            .get_mut("opensubtitlescom")
            .and_then(toml_edit::Item::as_table_like_mut)
            .ok_or("OpenSubtitles.com configuration must be a table")?
            .insert("password", toml_edit::value(password));
    }
    Ok(document.to_string())
}

fn subtitle_policy_args(ignore_embedded: bool, overwrite: bool, exclude_sdh: bool) -> Vec<String> {
    let mut args = Vec::new();
    if ignore_embedded {
        args.push("--force-embedded-subtitles".into());
    }
    if overwrite {
        args.push("--force-external-subtitles".into());
    }
    if exclude_sdh {
        args.extend([
            "--no-hearing-impaired".into(),
            "--language-type-suffix".into(),
        ]);
    }
    args
}


/// Everything a download needs, built once from the current settings.
#[derive(Clone)]
pub struct JobSetup {
    pub langs: Vec<String>,
    pub providers: Vec<String>,
    pub refiners: Vec<String>,
    pub opensubtitles_max_pages: Option<u8>,
    pub command: SubliminalCommand,
    pub config_content: String,
    pub force_download: bool,
    pub overwrite_existing: bool,
    pub minimum_score: u8,
    pub exclude_hearing_impaired: bool,
    pub scan_history: Arc<Mutex<ScanHistory>>,
    pub cancel: Arc<AtomicBool>,
}

impl JobSetup {
    pub fn new(
        settings: &Settings,
        command: SubliminalCommand,
        scan_history: Arc<Mutex<ScanHistory>>,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let password = crate::credentials::load_password();
        let config_content = sync_subliminal_credentials(
            &settings.opensubtitlescom_username,
            &settings.opensubtitlescom_apikey,
        )
        .and_then(|content| session_config(&content, &password))?;
        Ok(Self {
            langs: settings.selected_languages.clone(),
            providers: settings.providers.clone(),
            refiners: settings.refiners.clone(),
            opensubtitles_max_pages: settings.opensubtitles_max_pages,
            command,
            config_content,
            force_download: settings.force_download,
            overwrite_existing: settings.overwrite_existing,
            minimum_score: settings.minimum_score,
            exclude_hearing_impaired: settings.exclude_hearing_impaired,
            scan_history,
            cancel,
        })
    }
}

/// What happened to one video.
pub struct JobResult {
    pub status: JobStatus,
    pub subtitle_paths: Vec<PathBuf>,
}

impl JobResult {
    fn status(status: JobStatus) -> Self {
        Self {
            status,
            subtitle_paths: Vec::new(),
        }
    }
}

/// Download subtitles for `job_path`. `set_output` receives Subliminal's
/// latest status line while it runs.
pub fn process_video(
    setup: &JobSetup,
    job_path: &Path,
    set_output: &mut dyn FnMut(String),
) -> JobResult {
    if setup.cancel.load(Ordering::SeqCst) {
        return JobResult::status(JobStatus::Cancelled);
    }
    let langs = &setup.langs;
    let existing_subtitles = {
        let listing = job_path
            .parent()
            .map(SubtitleUtils::list_subtitle_files_in_folder)
            .unwrap_or_default();
        let mut v = SubtitleUtils::subtitle_files_for_video_in_listing(job_path, &listing);
        v.sort();
        v
    };
    let existing_subtitle_signatures: HashMap<PathBuf, (u64, u128)> = existing_subtitles
        .iter()
        .filter_map(|path| subtitle_file_signature(path).map(|signature| (path.clone(), signature)))
        .collect();
    // Never download over subtitles that are already there.
    if !SubtitleUtils::video_missing_subtitle_in_listing(
        job_path,
        langs,
        setup.exclude_hearing_impaired,
        &existing_subtitles,
    ) {
        info!("Skipped (already has subtitles): {}", job_path.display());
        return JobResult::status(JobStatus::Skipped);
    }

    // A private cache and config for each Subliminal process.
    let cache_dir = match PythonManager::ensure_cache_dir() {
        Ok(directory) => directory,
        Err(error) => {
            return JobResult::status(JobStatus::Failed(format!(
                "Could not create job cache: {error}"
            )))
        }
    };
    let config_path = cache_dir.path().join("subliminal.toml");
    if let Err(error) = Utils::write_atomic(&config_path, setup.config_content.as_bytes()) {
        return JobResult::status(JobStatus::Failed(format!(
            "Could not prepare job configuration: {error}"
        )));
    }
    let mut env_vars = HashMap::<String, String>::new();
    env_vars.insert("PYTHONIOENCODING".to_string(), "utf-8".to_string());
    env_vars.insert("PYTHONHASHSEED".to_string(), "0".to_string());

    // Do not add --debug; it can expose provider tokens.
    let mut all_args = vec![
        "--config".to_string(),
        config_path.to_string_lossy().into_owned(),
        "--cache-dir".to_string(),
        cache_dir.path().to_string_lossy().into_owned(),
    ];
    if let Some(pages) = setup.opensubtitles_max_pages {
        all_args.push("--provider.opensubtitlescom.max_result_pages".to_string());
        all_args.push(pages.to_string());
    }
    all_args.push("download".to_string());
    all_args.extend(subtitle_policy_args(
        setup.force_download,
        setup.overwrite_existing,
        setup.exclude_hearing_impaired,
    ));
    for provider in &setup.providers {
        all_args.push("--provider".to_string());
        all_args.push(provider.clone());
    }
    for refiner in &setup.refiners {
        let is_network_path = job_path.to_string_lossy().starts_with(r"\\")
            || (cfg!(target_os = "macos") && job_path.starts_with("/Volumes"));
        if refiner == "metadata" && is_network_path {
            continue;
        }
        all_args.push("--refiner".to_string());
        all_args.push(refiner.clone());
    }
    if setup.minimum_score > 0 {
        all_args.push("--min-score".to_string());
        all_args.push(setup.minimum_score.to_string());
    }
    for lang in langs {
        all_args.push("-l".to_string());
        all_args.push(lang.clone());
    }
    let Some(job_path_arg) = job_path.to_str() else {
        return JobResult::status(JobStatus::Failed("Video path is not valid UTF-8".to_string()));
    };
    all_args.push(job_path_arg.to_string());

    let mut stdout_buffer = String::new();
    let mut stderr_buffer = String::new();
    let mut suppress_stdout = false;
    let mut suppress_stderr = false;
    let mut on_output = |stream: &str, bytes: &[u8], _elapsed: std::time::Duration| {
        let (buffer, suppress) = match stream {
            "stdout" => (&mut stdout_buffer, &mut suppress_stdout),
            _ => (&mut stderr_buffer, &mut suppress_stderr),
        };
        if *suppress {
            return;
        }
        buffer.push_str(&String::from_utf8_lossy(bytes));
        if buffer.len() > MAX_STATUS_BUFFER_BYTES {
            // Drop oversized output so secrets stay together.
            buffer.clear();
            *suppress = true;
        }
    };
    let output = PythonManager::run_subliminal(
        &all_args,
        &env_vars,
        &setup.cancel,
        &mut on_output,
        &setup.command,
    );
    for (buffer, suppressed) in [(&stdout_buffer, suppress_stdout), (&stderr_buffer, suppress_stderr)] {
        if suppressed {
            continue;
        }
        if let Some(status) = latest_non_empty_line(&redact_sensitive(buffer)) {
            set_output(status.to_string());
        }
    }
    drop(cache_dir);

    if setup.cancel.load(Ordering::SeqCst) {
        return JobResult::status(JobStatus::Cancelled);
    }

    let mut subtitle_paths = Vec::new();
    let mut changed_subtitle_paths = Vec::new();
    let status = match output {
        Ok(out) => {
            let combined_output = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout).to_lowercase(),
                String::from_utf8_lossy(&out.stderr).to_lowercase()
            )
            .trim()
            .to_string();
            let folder_listing = job_path
                .parent()
                .map(SubtitleUtils::list_subtitle_files_in_folder)
                .unwrap_or_default();
            subtitle_paths =
                SubtitleUtils::find_all_subtitle_files_in_listing(job_path, langs, &folder_listing);
            let all_subtitle_paths =
                SubtitleUtils::subtitle_files_for_video_in_listing(job_path, &folder_listing);
            let mut excluded_sdh_count = 0;
            if setup.exclude_hearing_impaired {
                let rejected_paths: Vec<PathBuf> = all_subtitle_paths
                    .iter()
                    .filter(|path| SubtitleUtils::is_hearing_impaired_path(job_path, path))
                    .cloned()
                    .collect();
                let existing_set: HashSet<PathBuf> = existing_subtitles.iter().cloned().collect();
                let rejected_set: HashSet<PathBuf> = rejected_paths.iter().cloned().collect();
                excluded_sdh_count = rejected_paths.len();
                for path in rejected_paths.iter().filter(|path| !existing_set.contains(*path)) {
                    if let Err(error) = std::fs::remove_file(path) {
                        warn!("Failed to remove excluded SDH subtitle {}: {}", path.display(), error);
                    }
                }
                subtitle_paths.retain(|path| !rejected_set.contains(path));
            }
            changed_subtitle_paths = subtitle_paths
                .iter()
                .filter(|path| {
                    match (existing_subtitle_signatures.get(*path), subtitle_file_signature(path)) {
                        (Some(before), Some(after)) => before != &after,
                        (None, Some(_)) => true,
                        _ => false,
                    }
                })
                .cloned()
                .collect();
            classify(setup, job_path, &combined_output, &changed_subtitle_paths, &subtitle_paths, excluded_sdh_count)
        }
        Err(error) => {
            error!("Failed to run subliminal for {}: {}", job_path.display(), error);
            JobStatus::Failed(format!("Couldn't run Subliminal: {error}"))
        }
    };

    if matches!(status, JobStatus::Success) && !changed_subtitle_paths.is_empty() {
        let mut found_langs = ScanHistory::covered_langs(job_path, &changed_subtitle_paths, langs);
        if found_langs.is_empty() {
            found_langs = langs.clone();
        }
        crate::notifications::subtitle_found(job_path, &found_langs);
        if let Ok(mut history) = setup.scan_history.lock() {
            let hearing_impaired_langs =
                ScanHistory::hearing_impaired_langs(job_path, &changed_subtitle_paths, &found_langs);
            history.record_success(job_path, &found_langs, &hearing_impaired_langs);
            if let Err(error) = history.save_if_dirty() {
                warn!("Failed to save scan history: {}", error);
            }
        }
    }
    JobResult {
        status,
        subtitle_paths,
    }
}

/// Turn Subliminal's output into a job status.
fn classify(
    setup: &JobSetup,
    job_path: &Path,
    output: &str,
    changed: &[PathBuf],
    found: &[PathBuf],
    excluded_sdh_count: usize,
) -> JobStatus {
    if !changed.is_empty() {
        return JobStatus::Success;
    }
    if output.contains("downloaded 0 subtitle") {
        if !setup.force_download {
            let embedded = SubtitleUtils::embedded_subtitle_languages(job_path, &setup.langs);
            if !embedded.is_empty() {
                let names = |codes: Vec<&String>| {
                    codes
                        .into_iter()
                        .map(|code| SubtitleUtils::language_code_to_name(code))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let missing: Vec<&String> =
                    setup.langs.iter().filter(|code| !embedded.contains(code)).collect();
                let embedded_names = names(embedded.iter().collect());
                return if missing.is_empty() {
                    JobStatus::EmbeddedExists(format!("Has embedded {embedded_names} subtitles"))
                } else {
                    JobStatus::NotFound(format!(
                        "Has embedded {embedded_names}; none online yet for {}",
                        names(missing)
                    ))
                };
            }
        }
        return JobStatus::NotFound("No subtitles online yet".to_string());
    }
    if output.contains("dbm.error") || output.contains("db type could not be determined") {
        warn!("Subliminal cache error for {}", job_path.display());
        return JobStatus::Failed("Subliminal cache error".to_string());
    }
    if output.contains("error") || output.contains("failed") {
        return JobStatus::Failed("Subliminal error (see log)".to_string());
    }
    if excluded_sdh_count > 0 && found.is_empty() {
        return JobStatus::NotFound("Only SDH/caption subtitles found".to_string());
    }
    JobStatus::NotFound("No subtitles online yet".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_non_empty_line_keeps_partial_output() {
        assert_eq!(latest_non_empty_line("first\nfinal status"), Some("final status"));
    }

    #[test]
    fn redaction_handles_unicode_without_panicking() {
        assert_eq!(
            redact_sensitive("préface password=éclair token=abc apikey = \"secret value\""),
            "préface password=*** token=*** apikey = \"***\""
        );
    }

    #[test]
    fn redaction_accepts_empty_and_truncated_values() {
        for text in ["token=\"\"", "password=''", "apikey=", "token=,", "token=\""] {
            assert_eq!(redact_sensitive(text), text);
        }
        assert_eq!(redact_sensitive("token=\"\" password=secret"), "token=\"\" password=***");
        assert_eq!(redact_sensitive("token=\"unterminated"), "token=\"***");
    }

    #[test]
    fn subtitle_policies_are_independent() {
        for ignore_embedded in [false, true] {
            for overwrite in [false, true] {
                let args = subtitle_policy_args(ignore_embedded, overwrite, true);
                assert_eq!(args.iter().any(|a| a == "--force-embedded-subtitles"), ignore_embedded);
                assert_eq!(args.iter().any(|a| a == "--force-external-subtitles"), overwrite);
                assert!(!args.iter().any(|a| a == "--force"));
                assert!(args.iter().any(|a| a == "--language-type-suffix"));
                assert!(args.iter().any(|a| a == "--no-hearing-impaired"));
            }
        }
    }

    #[test]
    fn password_never_lands_in_subliminals_config_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("subliminal.toml");
        std::fs::write(
            &path,
            "# keep\n[download]\nforce = true\n[provider.other]\nsetting = 'preserved'\n[provider.opensubtitlescom]\npassword = 'old'\n",
        )
        .unwrap();
        let written = sync_credentials_at(&path, "me", "key").unwrap();
        let doc = written.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(doc["provider"]["opensubtitlescom"].get("password").is_none());
        assert_eq!(doc["provider"]["opensubtitlescom"]["username"].as_str(), Some("me"));
        assert_eq!(doc["provider"]["other"]["setting"].as_str(), Some("preserved"));
        assert!(written.starts_with("# keep"));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("old"));

        let value = "fixture\\\"\n\t";
        let session = session_config(&written, value).unwrap();
        let doc = session.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(doc["provider"]["opensubtitlescom"]["password"].as_str(), Some(value));
        assert_eq!(doc["download"]["force"].as_bool(), Some(false));
        let no_password = session_config(&written, "").unwrap();
        assert!(!no_password.contains("password"));

        std::fs::write(&path, "[invalid").unwrap();
        assert!(sync_credentials_at(&path, "", "").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[invalid");
    }
}
