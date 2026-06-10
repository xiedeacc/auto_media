use crate::config::MultiImagePolicy;
use anyhow::{Context, Result};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone)]
pub struct TargetImage {
    pub path: PathBuf,
    modified: SystemTime,
    captured_at: Option<NaiveDateTime>,
}

pub struct ImageScanner {
    data_dir: PathBuf,
    patterns: Vec<String>,
    policy: MultiImagePolicy,
}

impl ImageScanner {
    pub fn new(data_dir: PathBuf, patterns: Vec<String>, policy: MultiImagePolicy) -> Self {
        Self {
            data_dir,
            patterns,
            policy,
        }
    }

    pub fn find_target_image(&self, target_date: NaiveDate) -> Result<Option<TargetImage>> {
        if !self.data_dir.exists() {
            return Ok(None);
        }

        let mut matches = Vec::new();
        self.scan_dir(&self.data_dir, target_date, &mut matches)
            .with_context(|| format!("scan {}", self.data_dir.display()))?;

        if matches.is_empty() {
            return Ok(None);
        }

        let _policy = self.policy;
        matches.sort_by(|a, b| compare_newest_first(a, b));
        Ok(matches.into_iter().next())
    }

    fn scan_dir(
        &self,
        dir: &Path,
        target_date: NaiveDate,
        matches: &mut Vec<TargetImage>,
    ) -> Result<()> {
        for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;

            if metadata.is_dir() {
                self.scan_dir(&path, target_date, matches)?;
                continue;
            }

            if !metadata.is_file() || metadata.len() == 0 {
                continue;
            }

            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if file_name.starts_with('.') || file_name.ends_with(".tmp") {
                continue;
            }
            if let Some(captured_at) = self.match_target_date(file_name, target_date) {
                matches.push(TargetImage {
                    path,
                    modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    captured_at,
                });
            }
        }

        Ok(())
    }

    fn match_target_date(
        &self,
        file_name: &str,
        target_date: NaiveDate,
    ) -> Option<Option<NaiveDateTime>> {
        let lower = file_name.to_ascii_lowercase();

        if !is_supported_image(&lower) {
            return None;
        }

        for captured_at in extract_embedded_timestamps(file_name) {
            if captured_at.date() == target_date {
                let timestamp = captured_at.format("%Y%m%d%H%M%S").to_string();
                if self.patterns.iter().any(|pattern| {
                    let expanded = expand_pattern(pattern, target_date, Some(&timestamp));
                    wildcard_match(&expanded.to_ascii_lowercase(), &lower)
                }) {
                    return Some(Some(captured_at));
                }
            }
        }

        let matches_prefix = self.patterns.iter().any(|pattern| {
            if pattern.contains("{YYYYMMDDHHMMSS}") {
                return false;
            }
            let expanded = expand_pattern(pattern, target_date, None);
            wildcard_match(&expanded.to_ascii_lowercase(), &lower)
        });

        matches_prefix.then_some(None)
    }
}

fn compare_newest_first(a: &TargetImage, b: &TargetImage) -> std::cmp::Ordering {
    b.captured_at
        .cmp(&a.captured_at)
        .then_with(|| b.modified.cmp(&a.modified))
        .then_with(|| b.path.cmp(&a.path))
}

fn expand_pattern(pattern: &str, target_date: NaiveDate, timestamp: Option<&str>) -> String {
    pattern
        .replace("{YYYYMMDDHHMMSS}", timestamp.unwrap_or(""))
        .replace("{YYYYMMDD}", &target_date.format("%Y%m%d").to_string())
        .replace("{YYYY-MM-DD}", &target_date.format("%Y-%m-%d").to_string())
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star_index = None;
    let mut star_value_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == value[value_index] {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }

    pattern_index == pattern.len()
}

fn extract_embedded_timestamps(file_name: &str) -> Vec<NaiveDateTime> {
    let bytes = file_name.as_bytes();
    if bytes.len() < 14 {
        return Vec::new();
    }

    let mut timestamps = Vec::new();
    for start in 0..=bytes.len() - 14 {
        let candidate = &bytes[start..start + 14];
        if !candidate.iter().all(u8::is_ascii_digit) {
            continue;
        }

        let Ok(text) = std::str::from_utf8(candidate) else {
            continue;
        };
        let Ok(date) = NaiveDate::parse_from_str(&text[..8], "%Y%m%d") else {
            continue;
        };
        let Ok(time) = NaiveTime::parse_from_str(&text[8..], "%H%M%S") else {
            continue;
        };
        timestamps.push(date.and_time(time));
    }

    timestamps
}

fn is_supported_image(file_name: &str) -> bool {
    ["jpg", "jpeg", "png", "webp"]
        .iter()
        .any(|extension| file_name.ends_with(&format!(".{extension}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn finds_previous_day_image_by_compact_prefix() {
        let dir = make_temp_dir("compact");
        fs::write(dir.join("20260609-card.jpg"), b"image").unwrap();
        fs::write(dir.join("20260608-card.jpg"), b"image").unwrap();

        let scanner = ImageScanner::new(
            dir.clone(),
            vec!["{YYYYMMDD}*.jpg".to_string()],
            MultiImagePolicy::FirstByName,
        );
        let image = scanner
            .find_target_image(NaiveDate::from_ymd_opt(2026, 6, 9).unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(image.path.file_name().unwrap(), "20260609-card.jpg");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finds_image_by_embedded_timestamp() {
        let dir = make_temp_dir("embedded");
        fs::write(dir.join("微信图片_20260610200646_374_14.png"), b"image").unwrap();
        fs::write(dir.join("微信图片_20260609235959_001.png"), b"image").unwrap();

        let scanner = ImageScanner::new(
            dir.clone(),
            vec!["*{YYYYMMDDHHMMSS}*.png".to_string()],
            MultiImagePolicy::Newest,
        );
        let image = scanner
            .find_target_image(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(
            image.path.file_name().unwrap(),
            "微信图片_20260610200646_374_14.png"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn selects_latest_embedded_timestamp_for_same_day() {
        let dir = make_temp_dir("latest_embedded");
        fs::write(dir.join("微信图片_20260610200646_374_14.png"), b"image").unwrap();
        fs::write(dir.join("微信图片_20260610210646_374_15.png"), b"image").unwrap();
        fs::write(dir.join("微信图片_20260610190646_374_13.png"), b"image").unwrap();

        let scanner = ImageScanner::new(
            dir.clone(),
            vec!["*{YYYYMMDDHHMMSS}*.png".to_string()],
            MultiImagePolicy::Error,
        );
        let image = scanner
            .find_target_image(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(
            image.path.file_name().unwrap(),
            "微信图片_20260610210646_374_15.png"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn matches_timestamp_pattern_with_case_insensitive_extension() {
        let dir = make_temp_dir("uppercase_extension");
        fs::write(dir.join("微信图片_20260610200646_374_14.PNG"), b"image").unwrap();

        let scanner = ImageScanner::new(
            dir.clone(),
            vec!["*{YYYYMMDDHHMMSS}*.png".to_string()],
            MultiImagePolicy::Newest,
        );
        let image = scanner
            .find_target_image(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(
            image.path.file_name().unwrap(),
            "微信图片_20260610200646_374_14.PNG"
        );
        let _ = fs::remove_dir_all(dir);
    }

    fn make_temp_dir(name: &str) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let dir = std::env::temp_dir().join(format!("auto_media_{name}_{millis}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
