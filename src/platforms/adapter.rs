use super::{
    backend::{CookieStore, PublishBackend, PublishContent},
    douyin_cdp::DouyinCdp,
    douyin_api::DouyinApi,
    twitter_cdp::TwitterCdp,
    twitter_api::TwitterApi,
    xhs_cdp::XhsCdp,
    xhs_api::XhsApi,
    xueqiu_cdp::XueqiuCdp,
    xueqiu_api::XueqiuApi,
    zhihu_cdp::ZhihuCdp,
    zhihu_api::ZhihuApi,
    Platform, PlatformAdapter, PublishResult, SessionStatus,
};
use crate::{
    config::PlatformSection,
    publish::job::{ManualPublishJob, PublishJob},
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// One adapter per platform. Owns the shared cookie store plus a CDP and an HTTP
/// API backend, and publishes via the preferred backend, falling back to the other.
pub struct MediaPlatformAdapter {
    platform: Platform,
    cookies: Arc<CookieStore>,
    cdp: Box<dyn PublishBackend>,
    api: Box<dyn PublishBackend>,
    prefer_cdp: AtomicBool,
}

impl MediaPlatformAdapter {
    pub fn new(
        platform: Platform,
        platform_config: PlatformSection,
        profile_dir: PathBuf,
        auth_file: PathBuf,
        topic_cache_file: PathBuf,
    ) -> Self {
        let cookies = Arc::new(CookieStore::new(
            platform,
            platform_config.clone(),
            profile_dir.clone(),
            auth_file,
        ));
        let publish_url = resolve_publish_url(&platform_config);
        // `mode = "api"` means prefer the HTTP API; anything else prefers CDP.
        let prefer_cdp = !platform_config.mode.eq_ignore_ascii_case("api");
        let cdp = cdp_backend(platform, &platform_config, profile_dir, publish_url);
        let api = api_backend(platform, cookies.clone(), topic_cache_file);
        Self {
            platform,
            cookies,
            cdp,
            api,
            prefer_cdp: AtomicBool::new(prefer_cdp),
        }
    }

    /// Publish via the preferred backend; on failure fall back to the other.
    /// This is the single place the user-facing message is composed.
    async fn publish_content(&self, content: PublishContent<'_>) -> Result<PublishResult> {
        let prefer_cdp = self.prefer_cdp.load(Ordering::Relaxed);
        let (primary, secondary, primary_name, secondary_name) = if prefer_cdp {
            (&self.cdp, &self.api, "浏览器(CDP)", "API")
        } else {
            (&self.api, &self.cdp, "API", "浏览器(CDP)")
        };

        let message = match primary.publish(content).await {
            Ok(message) => format!("{} 已通过{primary_name}提交。{message}", self.platform),
            Err(primary_error) => {
                tracing::warn!(
                    platform = %self.platform,
                    error = %primary_error,
                    "{primary_name} publish failed, falling back"
                );
                match secondary.publish(content).await {
                    Ok(secondary_message) => format!(
                        "{} {primary_name}发布失败，已回退{secondary_name}：{primary_error:#}；{secondary_message}",
                        self.platform
                    ),
                    Err(secondary_error) => {
                        return Err(anyhow!(
                            "{} {primary_name}与{secondary_name}均发布失败。{primary_name}：{primary_error:#}；{secondary_name}：{secondary_error:#}",
                            self.platform
                        ));
                    }
                }
            }
        };
        Ok(PublishResult {
            platform: self.platform,
            remote_url: None,
            message,
        })
    }
}

#[async_trait]
impl PlatformAdapter for MediaPlatformAdapter {
    fn platform(&self) -> Platform {
        self.platform
    }

    fn prefer_cdp(&self) -> bool {
        self.prefer_cdp.load(Ordering::Relaxed)
    }

    fn set_prefer_cdp(&self, prefer: bool) {
        self.prefer_cdp.store(prefer, Ordering::Relaxed);
    }

    async fn validate_session(&self) -> Result<SessionStatus> {
        self.cookies.validate_session().await
    }

    async fn login_interactive(&self) -> Result<SessionStatus> {
        self.cookies.open_login().await
    }

    async fn publish_image_article(&self, job: &PublishJob) -> Result<PublishResult> {
        let images = std::slice::from_ref(&job.image_path);
        self.publish_content(PublishContent {
            title: &job.title,
            body: &job.body_text,
            image_paths: images,
        })
        .await
    }

    async fn publish_manual_article(&self, job: &ManualPublishJob) -> Result<PublishResult> {
        self.publish_content(PublishContent {
            title: &job.title,
            body: &job.body_text,
            image_paths: &job.image_paths,
        })
        .await
    }
}

fn resolve_publish_url(config: &PlatformSection) -> String {
    config
        .write_url
        .clone()
        .or_else(|| config.creator_url.clone())
        .unwrap_or_else(|| config.login_url.clone())
}

fn cdp_backend(
    platform: Platform,
    config: &PlatformSection,
    profile_dir: PathBuf,
    publish_url: String,
) -> Box<dyn PublishBackend> {
    let port = config.cdp_port;
    match platform {
        Platform::Xhs => Box::new(XhsCdp::new(port, profile_dir, publish_url)),
        Platform::Zhihu => Box::new(ZhihuCdp::new(port, profile_dir, publish_url)),
        Platform::Twitter => Box::new(TwitterCdp::new(port, profile_dir, publish_url)),
        Platform::Xueqiu => Box::new(XueqiuCdp::new(port, profile_dir, publish_url)),
        Platform::Douyin => Box::new(DouyinCdp::new(port, profile_dir, publish_url)),
    }
}

fn api_backend(
    platform: Platform,
    cookies: Arc<CookieStore>,
    topic_cache_file: PathBuf,
) -> Box<dyn PublishBackend> {
    match platform {
        Platform::Xhs => Box::new(XhsApi::new(cookies, topic_cache_file)),
        Platform::Zhihu => Box::new(ZhihuApi::new(cookies, topic_cache_file)),
        Platform::Twitter => Box::new(TwitterApi::new(cookies)),
        Platform::Xueqiu => Box::new(XueqiuApi::new(cookies)),
        Platform::Douyin => Box::new(DouyinApi::new(cookies)),
    }
}
