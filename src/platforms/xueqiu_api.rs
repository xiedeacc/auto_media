//! Experimental HTTP API backend for Xueqiu (雪球).
//!
//! Xueqiu's status-post endpoint (`https://xueqiu.com/statuses/update.json`)
//! requires the `xq_a_token`/`u` cookies plus a client-derived anti-replay token.
//! These can't be verified or networked in this environment, so this backend is a
//! structural scaffold only — the verified path is [`super::xueqiu_cdp`]. It returns
//! a clear "experimental" error so the adapter's fallback message stays honest.

use super::backend::{CookieStore, PublishBackend, PublishContent};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub struct XueqiuApi {
    cookies: Arc<CookieStore>,
}

impl XueqiuApi {
    pub fn new(cookies: Arc<CookieStore>) -> Self {
        Self { cookies }
    }
}

#[async_trait]
impl PublishBackend for XueqiuApi {
    async fn publish(&self, _content: PublishContent<'_>) -> Result<String> {
        // A real implementation would sign a status update from these cookies; the
        // scaffold loads them but does not (yet) call the unverified private API.
        let _cookies = self.cookies.load_or_capture().await.ok();
        anyhow::bail!("雪球 API 发布为实验性功能，尚未启用；请使用 CDP 浏览器发布")
    }
}
