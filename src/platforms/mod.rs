mod adapter;
mod backend;
mod douyin_cdp;
mod douyin_api;
mod twitter_cdp;
mod twitter_api;
mod xhs_cdp;
mod xhs_api;
mod xueqiu_cdp;
mod xueqiu_api;
mod zhihu_cdp;
mod zhihu_api;

use crate::publish::job::{ManualPublishJob, PublishJob};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

pub use adapter::MediaPlatformAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Xhs,
    Zhihu,
    Twitter,
    Xueqiu,
    Douyin,
}

impl Platform {
    /// Every platform, in display order. Drives adapter construction, session
    /// listing, profile-dir creation, and tab cleanup so adding a platform is a
    /// one-line change here.
    pub const ALL: [Platform; 5] = [
        Platform::Xhs,
        Platform::Zhihu,
        Platform::Twitter,
        Platform::Xueqiu,
        Platform::Douyin,
    ];

    /// The canonical string form. MUST match the `snake_case` serde value and
    /// the `publish_platforms` config strings — never an alias.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Xhs => "xhs",
            Self::Zhihu => "zhihu",
            Self::Twitter => "twitter",
            Self::Xueqiu => "xueqiu",
            Self::Douyin => "douyin",
        }
    }

    /// Cookie domains owned by this platform (a cookie matches if its domain
    /// contains any entry).
    pub fn cookie_domains(self) -> &'static [&'static str] {
        match self {
            Self::Xhs => &["xiaohongshu.com"],
            Self::Zhihu => &["zhihu.com"],
            Self::Twitter => &["twitter.com", "x.com"],
            Self::Xueqiu => &["xueqiu.com"],
            Self::Douyin => &["douyin.com"],
        }
    }

    /// Cookies that must be present for the HTTP API backend to sign requests.
    /// Xueqiu/Douyin sets are best-guess: their API backends are experimental and
    /// the CDP backend is the verified path.
    pub fn required_cookies(self) -> &'static [&'static str] {
        match self {
            Self::Xhs => &["a1", "web_session"],
            Self::Zhihu => &["z_c0", "_xsrf", "d_c0"],
            Self::Twitter => &["auth_token", "ct0"],
            Self::Xueqiu => &["xq_a_token", "u"],
            Self::Douyin => &["sessionid"],
        }
    }

    /// SQLite `host_key LIKE` pattern for probing the Chrome cookie database.
    pub fn cookie_host_pattern(self) -> &'static str {
        match self {
            Self::Xhs => "%xiaohongshu%",
            Self::Zhihu => "%zhihu%",
            Self::Twitter => "%twitter%",
            Self::Xueqiu => "%xueqiu%",
            Self::Douyin => "%douyin%",
        }
    }

    /// Cookie names that indicate a logged-in session (used for the on-disk
    /// profile probe; may differ from [`Self::required_cookies`]).
    pub fn login_cookie_names(self) -> &'static [&'static str] {
        match self {
            Self::Xhs => &["web_session", "web_session_id"],
            Self::Zhihu => &["z_c0"],
            Self::Twitter => &["auth_token", "ct0"],
            Self::Xueqiu => &["xq_a_token"],
            Self::Douyin => &["sessionid", "sessionid_ss"],
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "xhs" | "xiaohongshu" => Ok(Self::Xhs),
            "zhihu" => Ok(Self::Zhihu),
            "twitter" | "x" => Ok(Self::Twitter),
            "xueqiu" | "snowball" | "xq" => Ok(Self::Xueqiu),
            "douyin" | "dy" | "tiktok" => Ok(Self::Douyin),
            other => anyhow::bail!("unknown platform: {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SessionStatus {
    Valid { account_name: Option<String> },
    Expired,
    Missing,
    NetworkError { message: String },
    RiskVerificationRequired,
}

impl SessionStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Valid { .. } => "已登录",
            Self::Expired => "已失效",
            Self::Missing => "未登录",
            Self::NetworkError { .. } => "网络错误",
            Self::RiskVerificationRequired => "需确认",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishResult {
    pub platform: Platform,
    pub remote_url: Option<String>,
    pub message: String,
}

#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    fn platform(&self) -> Platform;
    /// Whether CDP (browser) is the preferred backend; `false` means the HTTP API
    /// is preferred. The non-preferred backend is still used as a fallback.
    fn prefer_cdp(&self) -> bool;
    /// Switch the preferred backend at runtime.
    fn set_prefer_cdp(&self, prefer: bool);
    async fn validate_session(&self) -> Result<SessionStatus>;
    async fn login_interactive(&self) -> Result<SessionStatus>;
    async fn publish_image_article(&self, job: &PublishJob) -> Result<PublishResult>;
    async fn publish_manual_article(&self, job: &ManualPublishJob) -> Result<PublishResult>;
}
