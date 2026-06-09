use crate::config::MultiImagePolicy;
use anyhow::{anyhow, Context, Result};
use chrono::NaiveDate;
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone)]
pub struct TargetImage {
    pub path: PathBuf,
    modified: SystemTime,
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

        match self.policy {
            MultiImagePolicy::FirstByName => {
                matches.sort_by(|a, b| a.path.cmp(&b.path));
                Ok(matches.into_iter().next())
            }
            MultiImagePolicy::Newest => {
                matches.sort_by(|a, b| b.modified.cmp(&a.modified));
                Ok(matches.into_iter().next())
            }
            MultiImagePolicy::Error if matches.len() > 1 => Err(anyhow!(
                "multiple target images found for {target_date}: {}",
                matches
                    .iter()
                    .map(|image| image.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            MultiImagePolicy::Error => Ok(matches.into_iter().next()),
        }
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
            if self.matches_date_pattern(file_name, target_date) {
                matches.push(TargetImage {
                    path,
                    modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                });
            }
        }

        Ok(())
    }

    fn matches_date_pattern(&self, file_name: &str, target_date: NaiveDate) -> bool {
        let compact = target_date.format("%Y%m%d").to_string();
        let dashed = target_date.format("%Y-%m-%d").to_string();
        let lower = file_name.to_ascii_lowercase();

        if !is_supported_image(&lower) {
            return false;
        }

        self.patterns.iter().any(|pattern| {
            let expanded = pattern
                .replace("{YYYYMMDD}", &compact)
                .replace("{YYYY-MM-DD}", &dashed)
                .replace('*', "");
            lower.starts_with(&expanded.to_ascii_lowercase())
        }) || lower.starts_with(&compact)
            || lower.starts_with(&dashed)
    }
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
    fn errors_when_multiple_images_and_policy_is_error() {
        let dir = make_temp_dir("multiple");
        fs::write(dir.join("20260609-a.jpg"), b"image").unwrap();
        fs::write(dir.join("20260609-b.png"), b"image").unwrap();

        let scanner = ImageScanner::new(
            dir.clone(),
            vec!["{YYYYMMDD}*.jpg".to_string(), "{YYYYMMDD}*.png".to_string()],
            MultiImagePolicy::Error,
        );

        let result = scanner.find_target_image(NaiveDate::from_ymd_opt(2026, 6, 9).unwrap());
        assert!(result.is_err());
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
