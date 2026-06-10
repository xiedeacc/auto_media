use super::{
    twitter_api, xhs_api, zhihu_api, Platform, PlatformAdapter, PublishResult, SessionStatus,
};
use crate::{
    browser::cdp::{BrowserCookie, BrowserLaunch, CdpBrowser},
    config::PlatformSection,
    publish::job::{ManualPublishJob, PublishJob},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, time::SystemTime};

pub struct CdpPlatformAdapter {
    platform: Platform,
    platform_config: PlatformSection,
    profile_dir: PathBuf,
    auth_file: PathBuf,
    browser: CdpBrowser,
}

#[derive(Debug, Serialize, Deserialize)]
struct CookieSnapshot {
    platform: String,
    saved_at: String,
    cookies: Vec<BrowserCookie>,
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
        let url = if self.platform == Platform::Xhs && url.contains("/publish/publish") {
            "https://creator.xiaohongshu.com/publish/publish?from=menu&target=image"
        } else {
            url
        };
        self.browser
            .open_visible(&self.profile_dir, self.platform_config.cdp_port, url)
            .await
            .with_context(|| format!("open {} in browser", self.platform))
    }

    fn xhs_api_template_path(&self) -> Option<PathBuf> {
        if self.platform != Platform::Xhs {
            return None;
        }
        let conf_dir = self
            .profile_dir
            .parent()
            .and_then(|path| path.parent())
            .map(PathBuf::from)?;
        Some(conf_dir.join("xhs_publish_api.json"))
    }
}

#[async_trait]
impl PlatformAdapter for CdpPlatformAdapter {
    fn platform(&self) -> Platform {
        self.platform
    }

    async fn validate_session(&self) -> Result<SessionStatus> {
        if self.load_cookie_snapshot().is_ok() {
            Ok(SessionStatus::Valid { account_name: None })
        } else if self.try_save_cookie_snapshot_from_running_browser().await? {
            Ok(SessionStatus::Valid { account_name: None })
        } else if self.has_login_cookie()? {
            Ok(SessionStatus::RiskVerificationRequired)
        } else if self.has_recent_profile_activity()? || self.has_browser_profile() {
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
        Ok(SessionStatus::RiskVerificationRequired)
    }

    async fn publish_image_article(&self, job: &PublishJob) -> Result<PublishResult> {
        if self.platform == Platform::Xhs {
            let cookies = self.load_or_capture_cookies().await?;
            let message = xhs_api::publish_image_note(
                &cookies,
                &job.title,
                &job.body_text,
                std::slice::from_ref(&job.image_path),
            )
            .await?;
            tracing::warn!(
                platform = %self.platform,
                message = %message,
                "xhs api article submitted"
            );
            return Ok(PublishResult {
                platform: self.platform,
                remote_url: None,
                message,
            });
        }

        if self.platform == Platform::Zhihu {
            let cookies = self.load_or_capture_cookies().await?;
            let message = zhihu_api::publish_image_article(
                &cookies,
                &job.title,
                &job.body_text,
                std::slice::from_ref(&job.image_path),
            )
            .await?;
            tracing::warn!(
                platform = %self.platform,
                message = %message,
                "zhihu api article submitted"
            );
            return Ok(PublishResult {
                platform: self.platform,
                remote_url: None,
                message,
            });
        }

        if self.platform == Platform::Twitter {
            let cookies = self.load_or_capture_cookies().await?;
            let message = twitter_api::publish_tweet(
                &cookies,
                &job.title,
                &job.body_text,
                std::slice::from_ref(&job.image_path),
            )
            .await?;
            tracing::warn!(
                platform = %self.platform,
                message = %message,
                "twitter api article submitted"
            );
            return Ok(PublishResult {
                platform: self.platform,
                remote_url: None,
                message,
            });
        }

        let launch = self.open_visible(self.publish_url()).await?;
        let fill_message = self
            .browser
            .fill_article_draft(
                &launch,
                self.platform.as_str(),
                &job.title,
                &job.body_text,
                std::slice::from_ref(&job.image_path),
                self.xhs_api_template_path().as_deref(),
            )
            .await?;
        let message = format!("{} 图文已自动提交。{}", self.platform, fill_message);

        tracing::warn!(
            platform = %self.platform,
            port = launch.port,
            url = %launch.url,
            fill_message = %fill_message,
            "cdp article submitted"
        );

        Ok(PublishResult {
            platform: self.platform,
            remote_url: None,
            message,
        })
    }

    async fn publish_manual_article(&self, job: &ManualPublishJob) -> Result<PublishResult> {
        if self.platform == Platform::Xhs {
            let cookies = self.load_or_capture_cookies().await?;
            let message =
                xhs_api::publish_image_note(&cookies, &job.title, &job.body_text, &job.image_paths)
                    .await?;
            tracing::warn!(
                platform = %self.platform,
                image_count = job.image_paths.len(),
                message = %message,
                "manual xhs api article submitted"
            );
            return Ok(PublishResult {
                platform: self.platform,
                remote_url: None,
                message,
            });
        }

        if self.platform == Platform::Zhihu {
            let cookies = self.load_or_capture_cookies().await?;
            let message = zhihu_api::publish_image_article(
                &cookies,
                &job.title,
                &job.body_text,
                &job.image_paths,
            )
            .await?;
            tracing::warn!(
                platform = %self.platform,
                image_count = job.image_paths.len(),
                message = %message,
                "manual zhihu api article submitted"
            );
            return Ok(PublishResult {
                platform: self.platform,
                remote_url: None,
                message,
            });
        }

        if self.platform == Platform::Twitter {
            let cookies = self.load_or_capture_cookies().await?;
            let message =
                twitter_api::publish_tweet(&cookies, &job.title, &job.body_text, &job.image_paths)
                    .await?;
            tracing::warn!(
                platform = %self.platform,
                image_count = job.image_paths.len(),
                message = %message,
                "manual twitter api article submitted"
            );
            return Ok(PublishResult {
                platform: self.platform,
                remote_url: None,
                message,
            });
        }

        let launch = self.open_visible(self.publish_url()).await?;
        let fill_message = self
            .browser
            .fill_article_draft(
                &launch,
                self.platform.as_str(),
                &job.title,
                &job.body_text,
                &job.image_paths,
                self.xhs_api_template_path().as_deref(),
            )
            .await?;
        let message = format!("{} 多图文已自动提交。{}", self.platform, fill_message);

        tracing::warn!(
            platform = %self.platform,
            port = launch.port,
            url = %launch.url,
            image_count = job.image_paths.len(),
            fill_message = %fill_message,
            "manual cdp article submitted"
        );

        Ok(PublishResult {
            platform: self.platform,
            remote_url: None,
            message,
        })
    }
}

impl CdpPlatformAdapter {
    async fn load_or_capture_cookies(&self) -> Result<Vec<BrowserCookie>> {
        if let Ok(cookies) = self.load_cookie_snapshot() {
            return Ok(cookies);
        }
        if self.try_save_cookie_snapshot_from_running_browser().await? {
            return self.load_cookie_snapshot();
        }
        anyhow::bail!(
            "{} cookie 快照不存在或已失效，请先登录并等待状态显示已登录",
            self.platform
        )
    }

    fn load_cookie_snapshot(&self) -> Result<Vec<BrowserCookie>> {
        let text = fs::read_to_string(&self.auth_file)
            .with_context(|| format!("read {}", self.auth_file.display()))?;
        let snapshot: CookieSnapshot = serde_json::from_str(&text)
            .with_context(|| format!("parse {}", self.auth_file.display()))?;
        self.validate_required_cookies(&snapshot.cookies)?;
        Ok(snapshot.cookies)
    }

    async fn try_save_cookie_snapshot_from_running_browser(&self) -> Result<bool> {
        let launch = BrowserLaunch {
            port: self.platform_config.cdp_port,
            url: self.platform_config.login_url.clone(),
            web_socket_debugger_url: None,
        };
        let cookies = match self.browser.get_cookies(&launch).await {
            Ok(cookies) => cookies,
            Err(_) => return Ok(false),
        };
        let cookies = cookies
            .into_iter()
            .filter(|cookie| self.cookie_belongs_to_platform(cookie))
            .collect::<Vec<_>>();
        if self.validate_required_cookies(&cookies).is_err() {
            return Ok(false);
        }
        self.save_cookie_snapshot(cookies)?;
        Ok(true)
    }

    fn save_cookie_snapshot(&self, cookies: Vec<BrowserCookie>) -> Result<()> {
        if let Some(parent) = self.auth_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create auth dir {}", parent.display()))?;
        }
        let snapshot = CookieSnapshot {
            platform: self.platform.to_string(),
            saved_at: Utc::now().to_rfc3339(),
            cookies,
        };
        let text = serde_json::to_string_pretty(&snapshot)?;
        fs::write(&self.auth_file, text)
            .with_context(|| format!("write {}", self.auth_file.display()))
    }

    fn cookie_belongs_to_platform(&self, cookie: &BrowserCookie) -> bool {
        match self.platform {
            Platform::Xhs => cookie.domain.contains("xiaohongshu.com"),
            Platform::Zhihu => cookie.domain.contains("zhihu.com"),
            Platform::Twitter => {
                cookie.domain.contains("twitter.com") || cookie.domain.contains("x.com")
            }
        }
    }

    fn validate_required_cookies(&self, cookies: &[BrowserCookie]) -> Result<()> {
        let required: &[&str] = match self.platform {
            Platform::Xhs => &["a1", "web_session"],
            Platform::Zhihu => &["z_c0", "_xsrf", "d_c0"],
            Platform::Twitter => &["auth_token", "ct0"],
        };
        for name in required {
            if !cookies.iter().any(|cookie| cookie.name == *name) {
                anyhow::bail!("missing required cookie {name}");
            }
        }
        Ok(())
    }

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
            Platform::Twitter => ("%twitter%", &["auth_token", "ct0"]),
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
            Platform::Twitter => &[b"auth_token", b"ct0"],
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

    fn has_browser_profile(&self) -> bool {
        self.profile_dir.join("Default").exists() || self.profile_dir.join("Local State").exists()
    }
}
