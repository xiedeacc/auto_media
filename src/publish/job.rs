use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishJob {
    pub job_id: String,
    pub target_date: NaiveDate,
    pub title: String,
    pub body_text: String,
    pub image_path: PathBuf,
    pub image_size: u64,
    pub image_mtime: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualPublishJob {
    pub job_id: String,
    pub title: String,
    pub body_text: String,
    pub image_paths: Vec<PathBuf>,
}

impl PublishJob {
    pub fn from_image(
        target_date: NaiveDate,
        image_path: PathBuf,
        title_pattern: &str,
        body_text: &str,
    ) -> Result<Self> {
        let metadata = fs::metadata(&image_path)
            .with_context(|| format!("read metadata {}", image_path.display()))?;
        let image_size = metadata.len();
        let image_mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();
        let title = render_date_pattern(title_pattern, target_date);
        let body_text = render_date_pattern(body_text, target_date);
        let job_id = make_job_id(target_date, &image_path, image_size, image_mtime);

        Ok(Self {
            job_id,
            target_date,
            title,
            body_text,
            image_path,
            image_size,
            image_mtime,
        })
    }
}

fn render_date_pattern(pattern: &str, target_date: NaiveDate) -> String {
    pattern
        .replace("{YYYYMMDD}", &target_date.format("%Y%m%d").to_string())
        .replace("{YYYY-MM-DD}", &target_date.format("%Y-%m-%d").to_string())
}

impl ManualPublishJob {
    pub fn new(title: String, body_text: String, image_paths: Vec<PathBuf>) -> Result<Self> {
        anyhow::ensure!(!image_paths.is_empty(), "请选择至少一张图片");
        let now = chrono::Local::now();
        let title = if title.trim().is_empty() {
            format!("手动发文 - {}", now.format("%Y%m%d%H%M%S"))
        } else {
            title.trim().to_string()
        };
        let mut hasher = Sha256::new();
        hasher.update(now.to_rfc3339());
        hasher.update(title.as_bytes());
        hasher.update(body_text.as_bytes());
        for image_path in &image_paths {
            let metadata = fs::metadata(image_path)
                .with_context(|| format!("read metadata {}", image_path.display()))?;
            anyhow::ensure!(metadata.is_file(), "{} 不是文件", image_path.display());
            anyhow::ensure!(metadata.len() > 0, "{} 是空文件", image_path.display());
            hasher.update(image_path.display().to_string());
            hasher.update(metadata.len().to_le_bytes());
        }

        Ok(Self {
            job_id: hex::encode(hasher.finalize()),
            title,
            body_text,
            image_paths,
        })
    }
}

fn make_job_id(
    target_date: NaiveDate,
    image_path: &Path,
    image_size: u64,
    image_mtime: i64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(target_date.to_string());
    hasher.update(image_path.display().to_string());
    hasher.update(image_size.to_le_bytes());
    hasher.update(image_mtime.to_le_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn builds_title_from_pattern() {
        let dir = make_temp_dir();
        let image = dir.join("20260609.jpg");
        fs::write(&image, b"image").unwrap();

        let job = PublishJob::from_image(
            NaiveDate::from_ymd_opt(2026, 6, 9).unwrap(),
            image,
            "挑战千万美金 - {YYYYMMDD}",
            "挑战千万美金 - {YYYYMMDD}",
        )
        .unwrap();

        assert_eq!(job.title, "挑战千万美金 - 20260609");
        assert_eq!(job.body_text, "挑战千万美金 - 20260609");
        assert!(!job.job_id.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    fn make_temp_dir() -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let dir = std::env::temp_dir().join(format!("auto_media_job_{millis}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
