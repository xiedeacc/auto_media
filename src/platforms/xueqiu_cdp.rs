//! Xueqiu (雪球) CDP backend — the verified path. Posts a status/动态 with an
//! image attached.
//!
//! Verified live: the composer is a `medium-editor` whose `contenteditable`
//! (`.medium-editor-element`) only initializes on a *trusted* focus, so we click
//! it via CDP first, then fill with `execCommand` (which enables the submit
//! button). Selectors are centralized here, so a Xueqiu UI change is a one-file edit.

use super::backend::{
    self, fill_editable_script, label_center_script, CdpFlow, PublishBackend, PublishContent,
};
use super::Platform;
use crate::browser::cdp::{CdpBrowser, CdpPage};
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

/// The composer's own file input lives inside `.lite-editor` (with an empty
/// `accept`); the page's `image/*` inputs belong to report/moderation forms, so
/// we must NOT use the generic image-input heuristic here. Files are set on this
/// input directly via CDP — clicking the toolbar image button would open a
/// blocking native file dialog.
const FILE_INPUT_SELECTORS: &[&str] = &[".lite-editor input[type=file]", "input[type=file]"];

/// The active editable is `medium-editor`'s `.medium-editor-element`; the others
/// are fallbacks for any future composer variant.
const STATUS_FIELD_SELECTORS: &str =
    ".medium-editor-element[contenteditable=true], .lite-editor__textarea [contenteditable=true], div[contenteditable=true], textarea";

pub struct XueqiuCdp {
    browser: CdpBrowser,
    port: u16,
    profile_dir: PathBuf,
    publish_url: String,
}

impl XueqiuCdp {
    pub fn new(port: u16, profile_dir: PathBuf, publish_url: String) -> Self {
        Self {
            browser: CdpBrowser,
            port,
            profile_dir,
            publish_url,
        }
    }
}

#[async_trait]
impl PublishBackend for XueqiuCdp {
    async fn publish(&self, content: PublishContent<'_>) -> Result<String> {
        backend::cdp_publish(
            &self.browser,
            Platform::Xueqiu,
            self.port,
            &self.profile_dir,
            &self.publish_url,
            self,
            content,
        )
        .await
    }
}

#[async_trait]
impl CdpFlow for XueqiuCdp {
    fn fill_before_upload(&self) -> bool {
        // Type the status text, then attach the image via the composer toolbar.
        true
    }

    async fn prepare(&self, page: &mut CdpPage) -> Result<()> {
        page.wait_for_truthy("(() => document.readyState !== 'loading')()", "雪球页面未加载完成")
            .await?;
        // A trusted CDP click activates the medium-editor; a synthetic focus does not.
        if !page.click_eval(EDITOR_CLICK_SCRIPT).await? {
            anyhow::bail!("没有找到雪球发帖输入框");
        }
        page.wait_for_truthy(EDITOR_READY_SCRIPT, "雪球发帖编辑框未就绪")
            .await
    }

    async fn upload_images(&self, page: &mut CdpPage, images: &[PathBuf]) -> Result<String> {
        page.set_file_input(FILE_INPUT_SELECTORS, images).await?;
        Ok(format!("已提交 {} 张图片到上传控件", images.len()))
    }

    async fn fill_text(&self, page: &mut CdpPage, content: PublishContent<'_>) -> Result<String> {
        let text = join_text(content.title, content.body);
        page.evaluate(&fill_editable_script(STATUS_FIELD_SELECTORS, &text))
            .await?;
        Ok("雪球动态正文已填充".to_string())
    }

    async fn click_publish(
        &self,
        page: &mut CdpPage,
        _content: PublishContent<'_>,
    ) -> Result<String> {
        for label in ["发布", "发表"] {
            if page.click_eval(&label_center_script(label)).await? {
                page.drain_dialog_events().await?;
                sleep(Duration::from_secs(2)).await;
                return Ok(format!("已自动点击雪球发布按钮：{label}"));
            }
        }
        anyhow::bail!("没有找到雪球发布按钮")
    }
}

fn join_text(title: &str, body: &str) -> String {
    [title.trim(), body.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Returns the center of the visible status composer box so it can be clicked
/// (with a trusted CDP click) to initialize the medium-editor contenteditable.
const EDITOR_CLICK_SCRIPT: &str = r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  const box = Array.from(document.querySelectorAll('.lite-editor__textarea, .medium-editor-element[contenteditable=true], textarea, [contenteditable=true]'))
    .filter(visible)[0];
  if (!box) return null;
  const rect = box.getBoundingClientRect();
  return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
})()
"#;

/// The medium-editor contenteditable has appeared (i.e. the box is active).
const EDITOR_READY_SCRIPT: &str = r#"
(() => Array.from(document.querySelectorAll('.medium-editor-element[contenteditable=true], .lite-editor__textarea [contenteditable=true], textarea, [contenteditable=true]'))
  .some(el => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; }))()
"#;
