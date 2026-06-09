use super::{Platform, PlatformAdapter, PublishResult, SessionStatus};
use crate::{
    browser::cdp::{BrowserLaunch, CdpBrowser},
    config::PlatformSection,
    publish::job::PublishJob,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use std::{fs, path::PathBuf};

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
