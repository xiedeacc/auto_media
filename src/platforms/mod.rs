mod cdp_adapter;
mod twitter_api;
mod xhs_api;
mod zhihu_api;

use crate::publish::job::{ManualPublishJob, PublishJob};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

pub use cdp_adapter::CdpPlatformAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Xhs,
    Zhihu,
    Twitter,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Xhs => "xhs",
            Self::Zhihu => "zhihu",
            Self::Twitter => "twitter",
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
    async fn validate_session(&self) -> Result<SessionStatus>;
    async fn login_interactive(&self) -> Result<SessionStatus>;
    async fn publish_image_article(&self, job: &PublishJob) -> Result<PublishResult>;
    async fn publish_manual_article(&self, job: &ManualPublishJob) -> Result<PublishResult>;
}
