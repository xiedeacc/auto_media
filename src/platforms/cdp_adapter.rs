use super::{Platform, PlatformAdapter, PublishResult, SessionStatus};
use crate::{
    browser::cdp::{BrowserLaunch, CdpBrowser},
    config::PlatformSection,
    publish::job::PublishJob,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags};
use std::{fs, path::PathBuf, time::SystemTime};

pub struct CdpPlatformAdapter {
    platform: Platform,
    platform_config: PlatformSection,
    profile_dir: PathBuf,
    auth_file: PathBuf,
    browser: CdpBrowser,
}

impl CdpPlatformAdapter {
    pub fn new(
        platform: Platform,
        platform_config: PlatformSection,
        profile_dir: PathBuf,
        auth_file: PathBuf,
    ) -> Self {
        Self {
            platform,
            platform_config,
            profile_dir,
            auth_file,
            browser: CdpBrowser::default(),
        }
    }

    fn publish_url(&self) -> &str {
        self.platform_config
            .write_url
            .as_deref()
            .or(self.platform_config.creator_url.as_deref())
            .unwrap_or(&self.platform_config.login_url)
    }

    async fn open_visible(&self, url: &str) -> Result<BrowserLaunch> {
        self.browser
            .open_visible(&self.profile_dir, self.platform_config.cdp_port, url)
            .await
            .with_context(|| format!("open {} in browser", self.platform))
    }
}

#[async_trait]
impl PlatformAdapter for CdpPlatformAdapter {
    fn platform(&self) -> Platform {
        self.platform
    }

    async fn validate_session(&self) -> Result<SessionStatus> {
        if self.auth_file.exists() {
            Ok(SessionStatus::Valid { account_name: None })
        } else if self.has_login_cookie()? {
            Ok(SessionStatus::Valid { account_name: None })
        } else if self.has_recent_profile_activity()? {
            Ok(SessionStatus::RiskVerificationRequired)
        } else {
            Ok(SessionStatus::Missing)
        }
    }

    async fn login_interactive(&self) -> Result<SessionStatus> {
        self.open_visible(&self.platform_config.login_url).await?;
        if let Some(parent) = self.auth_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create auth dir {}", parent.display()))?;
        }
        fs::write(
            &self.auth_file,
            format!(
                "platform={}\nlogin_started_at={}\n",
                self.platform,
                Utc::now().to_rfc3339()
            ),
        )
        .with_context(|| format!("write auth marker {}", self.auth_file.display()))?;
        Ok(SessionStatus::RiskVerificationRequired)
    }

    async fn publish_image_article(&self, job: &PublishJob) -> Result<PublishResult> {
        let launch = self.open_visible(self.publish_url()).await?;
        let message = format!(
            "{} 发布页已通过 CDP 浏览器会话打开。标题: {}，图片: {}。选择器级自动填充/点击发布将在该适配器内继续完善。",
            self.platform,
            job.title,
            job.image_path.display()
        );

        tracing::warn!(
            platform = %self.platform,
            port = launch.port,
            url = %launch.url,
            "cdp page opened; selector automation pending"
        );

        anyhow::bail!(message)
    }
}

impl CdpPlatformAdapter {
    fn has_login_cookie(&self) -> Result<bool> {
        let cookie_db = self
            .profile_dir
            .join("Default")
            .join("Network")
            .join("Cookies");
        if !cookie_db.exists() {
            return Ok(false);
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
        let uri = format!(
            "file:{}?mode=ro&immutable=1",
            cookie_db.display().to_string().replace('\\', "/")
        );
        let conn = match Connection::open_with_flags(uri, flags) {
            Ok(conn) => conn,
            Err(error) => {
                tracing::warn!(
                    platform = %self.platform,
                    error = %error,
                    "cookie database is not readable yet"
                );
                return self.cookie_file_contains_login_cookie(&cookie_db);
            }
        };

        let (host_pattern, cookie_names): (&str, &[&str]) = match self.platform {
            Platform::Xhs => ("%xiaohongshu%", &["web_session", "web_session_id"]),
            Platform::Zhihu => ("%zhihu%", &["z_c0"]),
        };

        for cookie_name in cookie_names {
            let count: i64 = conn.query_row(
                "SELECT COUNT(1) FROM cookies WHERE host_key LIKE ?1 AND name = ?2",
                params![host_pattern, cookie_name],
                |row| row.get(0),
            )?;
            if count > 0 {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn cookie_file_contains_login_cookie(&self, cookie_db: &PathBuf) -> Result<bool> {
        let bytes = match fs::read(cookie_db) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(
                    platform = %self.platform,
                    error = %error,
                    "cookie database bytes are not readable"
                );
                return Ok(false);
            }
        };

        let names: &[&[u8]] = match self.platform {
            Platform::Xhs => &[b"web_session", b"web_session_id"],
            Platform::Zhihu => &[b"z_c0"],
        };

        Ok(names
            .iter()
            .any(|name| bytes.windows(name.len()).any(|window| window == *name)))
    }

    fn has_recent_profile_activity(&self) -> Result<bool> {
        let local_state = self.profile_dir.join("Local State");
        let default_dir = self.profile_dir.join("Default");
        let latest = [local_state, default_dir]
            .into_iter()
            .filter_map(|path| fs::metadata(path).ok())
            .filter_map(|metadata| metadata.modified().ok())
            .max();

        let Some(modified) = latest else {
            return Ok(false);
        };
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            .as_secs();
        Ok(age <= 60 * 60 * 24)
    }
}
