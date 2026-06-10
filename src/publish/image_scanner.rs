use crate::config::MultiImagePolicy;
use anyhow::{Context, Result};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use regex::RegexBuilder;
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

        for pattern in &self.patterns {
            if let Some(captured_at) = parse_datetime_by_pattern(pattern, file_name) {
                if captured_at.date() == target_date {
                    return Some(Some(captured_at));
                }
            }
        }
        None
    }
}

fn compare_newest_first(a: &TargetImage, b: &TargetImage) -> std::cmp::Ordering {
    b.captured_at
        .cmp(&a.captured_at)
        .then_with(|| b.modified.cmp(&a.modified))
        .then_with(|| b.path.cmp(&a.path))
}

fn parse_datetime_by_pattern(pattern: &str, file_name: &str) -> Option<NaiveDateTime> {
    let regex_pattern = pattern_to_regex(pattern)?;
    let regex = RegexBuilder::new(&regex_pattern)
        .case_insensitive(true)
        .build()
        .ok()?;
    let captures = regex.captures(file_name)?;
    let year = capture_u32(&captures, "year")? as i32;
    let month = capture_u32(&captures, "month")?;
    let day = capture_u32(&captures, "day")?;
    let hour = capture_u32(&captures, "hour").unwrap_or(0);
    let minute = capture_u32(&captures, "minute").unwrap_or(0);
    let second = capture_u32(&captures, "second").unwrap_or(0);
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = NaiveTime::from_hms_opt(hour, minute, second)?;
    Some(date.and_time(time))
}

fn capture_u32(captures: &regex::Captures<'_>, name: &str) -> Option<u32> {
    captures.name(name)?.as_str().parse().ok()
}

fn pattern_to_regex(pattern: &str) -> Option<String> {
    let mut regex = String::from("^");
    let mut rest = pattern;
    while !rest.is_empty() {
        if rest.starts_with('*') {
            regex.push_str(".*");
            rest = &rest[1..];
        } else if rest.starts_with("{YYYYMMDDHHMMSS}") {
            regex.push_str(
                r"(?P<year>\d{4})(?P<month>\d{2})(?P<day>\d{2})(?P<hour>\d{2})(?P<minute>\d{2})(?P<second>\d{2})",
            );
            rest = &rest["{YYYYMMDDHHMMSS}".len()..];
        } else if rest.starts_with("{YYYYMMDD}") {
            regex.push_str(r"(?P<year>\d{4})(?P<month>\d{2})(?P<day>\d{2})");
            rest = &rest["{YYYYMMDD}".len()..];
        } else if rest.starts_with("{YYYY-MM-DD}") {
            regex.push_str(r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})");
            rest = &rest["{YYYY-MM-DD}".len()..];
        } else if rest.starts_with("{YYYY}") {
            regex.push_str(r"(?P<year>\d{4})");
            rest = &rest["{YYYY}".len()..];
        } else if rest.starts_with("{MM}") {
            regex.push_str(r"(?P<month>\d{2})");
            rest = &rest["{MM}".len()..];
        } else if rest.starts_with("{DD}") {
            regex.push_str(r"(?P<day>\d{2})");
            rest = &rest["{DD}".len()..];
        } else if rest.starts_with("{HH}") {
            regex.push_str(r"(?P<hour>\d{2})");
            rest = &rest["{HH}".len()..];
        } else if rest.starts_with("{mm}") {
            regex.push_str(r"(?P<minute>\d{2})");
            rest = &rest["{mm}".len()..];
        } else if rest.starts_with("{SS}") {
            regex.push_str(r"(?P<second>\d{2})");
            rest = &rest["{SS}".len()..];
        } else if rest.starts_with('{') {
            return None;
        } else {
            let ch = rest.chars().next()?;
            regex.push_str(&regex::escape(&ch.to_string()));
            rest = &rest[ch.len_utf8()..];
        }
    }
    regex.push('$');
    Some(regex)
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

    #[test]
    fn supports_separated_timestamp_patterns() {
        let dir = make_temp_dir("separated_timestamp");
        fs::write(dir.join("微信图片_20260610_200646_older.png"), b"image").unwrap();
        fs::write(dir.join("微信图片_2026-06-10_21-06-46_newer.png"), b"image").unwrap();

        let scanner = ImageScanner::new(
            dir.clone(),
            vec![
                "*{YYYYMMDD}_{HH}{mm}{SS}*.png".to_string(),
                "*{YYYY-MM-DD}_{HH}-{mm}-{SS}*.png".to_string(),
            ],
            MultiImagePolicy::Newest,
        );
        let image = scanner
            .find_target_image(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(
            image.path.file_name().unwrap(),
            "微信图片_2026-06-10_21-06-46_newer.png"
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
