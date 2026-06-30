//! Twitter/X CDP (browser-automation) backend. Now the primary path (CDP-first
//! for every platform), with the HTTP API as the fallback. Composing a tweet
//! has no separate title, so title and body are joined into the tweet text here.

use super::backend::{
    self, label_center_script, selector_center_script, CdpFlow, PublishBackend, PublishContent,
};
use super::Platform;
use crate::browser::cdp::{CdpBrowser, CdpPage, DEFAULT_FILE_INPUT_SELECTORS};
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

/// Stable, locale-proof post buttons, verified live against x.com:
/// `tweetButtonInline` is the home/inline composer, `tweetButton` the modal one.
const PUBLISH_SELECTORS: &[&str] = &[
    "[data-testid=\"tweetButtonInline\"]",
    "[data-testid=\"tweetButton\"]",
];
/// Text fallbacks if the testids ever change. Includes the Chinese UI's "发帖".
const PUBLISH_LABELS: &[&str] = &["发帖", "发布", "Post", "Tweet", "推文"];

pub struct TwitterCdp {
    browser: CdpBrowser,
    port: u16,
    profile_dir: PathBuf,
    publish_url: String,
}

impl TwitterCdp {
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
impl PublishBackend for TwitterCdp {
    async fn publish(&self, content: PublishContent<'_>) -> Result<String> {
        backend::cdp_publish(
            &self.browser,
            Platform::Twitter,
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
impl CdpFlow for TwitterCdp {
    async fn prepare(&self, page: &mut CdpPage) -> Result<()> {
        page.wait_for_truthy("(() => document.readyState !== 'loading')()", "Twitter/X 页面未加载完成")
            .await?;
        page.evaluate(PREPARE_SCRIPT).await?;
        Ok(())
    }

    async fn upload_images(&self, page: &mut CdpPage, images: &[PathBuf]) -> Result<String> {
        page.set_file_input(DEFAULT_FILE_INPUT_SELECTORS, images)
            .await?;
        Ok(format!("已提交 {} 张图片到上传控件", images.len()))
    }

    async fn fill_text(&self, page: &mut CdpPage, title: &str, body: &str) -> Result<String> {
        let text = [title.trim(), body.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        page.evaluate(&fill_script(&text)).await?;
        Ok("Twitter/X 草稿已填充正文".to_string())
    }

    async fn click_publish(&self, page: &mut CdpPage) -> Result<String> {
        // The post button stays aria-disabled until the text commits, so retry a
        // few times. Prefer the stable testid, fall back to localized labels.
        for _ in 0..10 {
            for selector in PUBLISH_SELECTORS {
                if page.click_eval(&selector_center_script(selector)).await? {
                    page.drain_dialog_events().await?;
                    sleep(Duration::from_secs(2)).await;
                    return Ok(format!("已自动点击发帖按钮 ({selector})"));
                }
            }
            for label in PUBLISH_LABELS {
                if page.click_eval(&label_center_script(label)).await? {
                    page.drain_dialog_events().await?;
                    sleep(Duration::from_secs(2)).await;
                    return Ok(format!("已自动点击发布按钮：{label}"));
                }
            }
            sleep(Duration::from_millis(500)).await;
        }
        anyhow::bail!("没有找到可点击的发布按钮")
    }
}

const PREPARE_SCRIPT: &str = r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  const clickByText = (texts) => {
    for (const el of Array.from(document.querySelectorAll('button, [role=button], div, span, a'))) {
      const text = (el.innerText || el.textContent || '').trim();
      if (visible(el) && texts.some(t => text.includes(t))) { el.click(); return text; }
    }
    return null;
  };
  if (!document.querySelector('[data-testid="tweetTextarea_0"], [role="textbox"][contenteditable="true"], [contenteditable="true"]')) {
    const compose = document.querySelector('[data-testid="SideNav_NewTweet_Button"], a[href="/compose/post"], a[href="/compose/tweet"]');
    if (compose) compose.click();
    else clickByText(['Post', 'Tweet', '发布', '发帖']);
  }
  return true;
})()
"#;

fn fill_script(text: &str) -> String {
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const text = {text};
  const visible = (el) => {{
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }};
  const setText = (el, value) => {{
    el.focus();
    if (el.isContentEditable) {{
      const range = document.createRange();
      range.selectNodeContents(el);
      range.deleteContents();
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      document.execCommand('insertText', false, value);
    }} else if ('value' in el) {{
      const proto = Object.getPrototypeOf(el);
      const descriptor = Object.getOwnPropertyDescriptor(proto, 'value');
      if (descriptor && descriptor.set) descriptor.set.call(el, value);
      else el.value = value;
    }} else {{
      el.textContent = value;
    }}
    el.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: value }}));
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }};
  const bodyEl = Array.from(document.querySelectorAll('[data-testid="tweetTextarea_0"], [role="textbox"][contenteditable="true"], [contenteditable="true"]'))
    .filter(visible)[0];
  if (bodyEl) setText(bodyEl, text);
  return {{ message: `Twitter/X 草稿已填充：正文${{bodyEl ? '成功' : '未找到'}}。` }};
}})()
"#
    )
}
