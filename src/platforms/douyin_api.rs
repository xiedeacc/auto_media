//! Experimental HTTP API backend for Douyin (抖音).
//!
//! Douyin's image-text publish API sits behind heavy risk control (`X-Bogus`/`a_bogus`
//! signing, `msToken`, `sessionid`), which can't be reproduced or verified in this
//! environment. This backend is a structural scaffold only — the verified path is
//! [`super::douyin_cdp`]. It returns a clear "experimental" error so the adapter's
//! fallback message stays honest.

use super::backend::{CookieStore, PublishBackend, PublishContent};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub struct DouyinApi {
    cookies: Arc<CookieStore>,
}

impl DouyinApi {
    pub fn new(cookies: Arc<CookieStore>) -> Self {
        Self { cookies }
    }
}

#[async_trait]
impl PublishBackend for DouyinApi {
    async fn publish(&self, _content: PublishContent<'_>) -> Result<String> {
        // A real implementation would sign an image-text publish request from these
        // cookies; the scaffold loads them but does not call the unverified private API.
        let _cookies = self.cookies.load_or_capture().await.ok();
        anyhow::bail!("抖音 API 发布为实验性功能，尚未启用；请使用 CDP 浏览器发布")
    }
}
