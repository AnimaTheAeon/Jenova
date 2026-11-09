//! Subtitle sidecar detection (shared logic with the Windows app).

use std::path::{Path, PathBuf};

pub struct SubtitleUtils;

impl SubtitleUtils {
    /// List subtitle files in a folder.
    pub fn list_subtitle_files_in_folder(folder: &Path) -> Vec<PathBuf> {
        const EXTENSIONS: &[&str] = &["srt", "sub", "ssa", "ass", "vtt"];
        let entries = match std::fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };
        entries
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry.path()),
                Err(_) => None,
            })
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|ext| {
                            EXTENSIONS
                                .iter()
                                .any(|known| known.eq_ignore_ascii_case(ext))
                        })
            })
            .collect()
    }

    /// Filter a folder listing to one video's sidecars.
    pub fn subtitle_files_for_video_in_listing(
        video_path: &Path,
        folder_subtitles: &[PathBuf],
    ) -> Vec<PathBuf> {
        let stem = match video_path.file_stem().and_then(|value| value.to_str()) {
            Some(stem) => stem,
            None => return Vec::new(),
        };

        folder_subtitles
            .iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| {
                        name.eq_ignore_ascii_case(stem) || {
                            let prefix_len = stem.len();
                            name.len() > prefix_len + 1
                                && name.is_char_boundary(prefix_len)
                                && name.as_bytes().get(prefix_len) == Some(&b'.')
                                && name[..prefix_len].eq_ignore_ascii_case(stem)
                        }
                    })
            })
            .cloned()
            .collect()
    }

    fn sidecar_suffix<'a>(video_path: &Path, path: &'a Path) -> Option<&'a str> {
        let video = video_path.file_stem()?.to_str()?;
        let subtitle = path.file_stem()?.to_str()?;
        if !subtitle.get(..video.len())?.eq_ignore_ascii_case(video) {
            return None;
        }
        let suffix = &subtitle[video.len()..];
        if suffix.is_empty() {
            Some("")
        } else {
            suffix.strip_prefix('.')
        }
    }

    fn is_caption_marker(part: &str) -> bool {
        matches!(
            part.trim_matches(['[', ']', '(', ')'])
                .to_ascii_lowercase()
                .as_str(),
            "hi" | "sdh" | "cc" | "caption" | "captions"
        )
    }

    pub fn matches_language(video_path: &Path, path: &Path, language: &str) -> bool {
        let Some(suffix) = Self::sidecar_suffix(video_path, path) else {
            return false;
        };
        let parts: Vec<_> = suffix.split('.').collect();
        parts.iter().enumerate().any(|(index, part)| {
            part.eq_ignore_ascii_case(language)
                && parts
                    .iter()
                    .enumerate()
                    .all(|(other, part)| other == index || Self::is_caption_marker(part))
        })
    }

    /// Inspect the sidecar suffix, not words in the video title.
    pub fn is_hearing_impaired_path(video_path: &Path, path: &Path) -> bool {
        let Some(suffix) = Self::sidecar_suffix(video_path, path) else {
            return false;
        };
        suffix.contains('.') && suffix.split('.').any(Self::is_caption_marker)
    }

    /// Check missing subtitles using a folder listing.
    pub fn video_missing_subtitle_in_listing(
        video_path: &Path,
        selected_languages: &[String],
        exclude_hearing_impaired: bool,
        folder_subtitles: &[PathBuf],
    ) -> bool {
        let Some(stem) = video_path.file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        let subtitle_files =
            Self::subtitle_files_for_video_in_listing(video_path, folder_subtitles)
                .into_iter()
                .filter(|path| {
                    !exclude_hearing_impaired || !Self::is_hearing_impaired_path(video_path, path)
                })
                .collect::<Vec<_>>();
        let has_generic = subtitle_files.iter().any(|path| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(stem))
        });

        for lang in selected_languages {
            let has_language_specific = subtitle_files
                .iter()
                .any(|path| Self::matches_language(video_path, path, lang));
            if !has_language_specific && !has_generic {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::SubtitleUtils;


    #[test]
    fn missing_subtitle_uses_language_generic_and_sdh_rules() {
        let folder = std::env::temp_dir().join(format!(
            "jenova-missing-sub-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let video = folder.join("Movie.mkv");
        std::fs::write(&video, []).unwrap();
        let langs = vec!["en".to_string()];

        assert!(SubtitleUtils::video_missing_subtitle_in_listing(
            &video,
            &langs,
            false,
            &SubtitleUtils::list_subtitle_files_in_folder(&folder),
        ));

        std::fs::write(folder.join("Movie.en.srt"), "sub").unwrap();
        let listing = SubtitleUtils::list_subtitle_files_in_folder(&folder);
        assert!(!SubtitleUtils::video_missing_subtitle_in_listing(
            &video, &langs, false, &listing
        ));

        std::fs::remove_file(folder.join("Movie.en.srt")).unwrap();
        std::fs::write(folder.join("Movie.srt"), "sub").unwrap();
        let listing = SubtitleUtils::list_subtitle_files_in_folder(&folder);
        assert!(!SubtitleUtils::video_missing_subtitle_in_listing(
            &video, &langs, false, &listing
        ));

        std::fs::remove_file(folder.join("Movie.srt")).unwrap();
        std::fs::write(folder.join("Movie.en.sdh.srt"), "sub").unwrap();
        let listing = SubtitleUtils::list_subtitle_files_in_folder(&folder);
        assert!(!SubtitleUtils::video_missing_subtitle_in_listing(
            &video, &langs, false, &listing
        ));
        assert!(SubtitleUtils::video_missing_subtitle_in_listing(
            &video, &langs, true, &listing
        ));

        std::fs::remove_dir_all(folder).unwrap();
    }
}
