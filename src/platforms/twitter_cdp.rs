//! Twitter/X CDP (browser-automation) backend. Now the primary path (CDP-first
//! for every platform), with the HTTP API as the fallback. Composing a tweet
//! has no separate title, so title and body are joined into the tweet text here.

use super::backend::{
    self, label_center_script, selector_center_script, CdpFlow, PublishBackend, PublishContent,
};
use super::Platform;
use crate::browser::cdp::{human_pause, CdpBrowser, CdpPage, DEFAULT_FILE_INPUT_SELECTORS};
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

/// Stable, locale-proof post buttons, verified live against x.com:
/// `tweetButtonInline` is the home/inline composer, `tweetButton` the modal one.
/// The modal's post button is `tweetButton`; prefer it over the inline one.
const PUBLISH_SELECTORS: &[&str] = &[
    "[role=\"dialog\"] [data-testid=\"tweetButton\"]",
    "[data-testid=\"tweetButton\"]",
    "[data-testid=\"tweetButtonInline\"]",
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
    fn fill_before_upload(&self) -> bool {
        // Type the text into the fresh modal editor first; attaching the image
        // first re-renders the composer and the text won't commit.
        true
    }

    async fn prepare(&self, page: &mut CdpPage) -> Result<()> {
        page.wait_for_truthy("(() => document.readyState !== 'loading')()", "Twitter/X 页面未加载完成")
            .await?;
        // Open the compose MODAL via the black 发帖 button. The inline /home composer
        // rejects programmatic text input; the modal's editor accepts execCommand.
        let _ = page.click_eval(SIDENAV_POINT_SCRIPT).await;
        page.wait_for_truthy(MODAL_READY_SCRIPT, "Twitter/X 发帖窗口未打开")
            .await
    }

    async fn upload_images(&self, page: &mut CdpPage, images: &[PathBuf]) -> Result<String> {
        page.set_file_input(DEFAULT_FILE_INPUT_SELECTORS, images)
            .await?;
        Ok(format!("已提交 {} 张图片到上传控件", images.len()))
    }

    async fn fill_text(&self, page: &mut CdpPage, content: PublishContent<'_>) -> Result<String> {
        // Tweets have no topic control — keep hashtags inline (content.body already
        // carries the trailing "#tag" line).
        let text = [content.title.trim(), content.body.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        // In the compose modal, execCommand insertText commits to DraftJS state.
        page.evaluate(&fill_script(&text)).await?;
        human_pause(400).await;
        Ok("Twitter/X 草稿已填充正文".to_string())
    }

    async fn wait_publish_ready(&self, page: &mut CdpPage) -> Result<()> {
        // The post button stays aria-disabled until the composed text commits to
        // React state; wait for it to enable so the click/shortcut actually posts.
        let _ = page
            .wait_for_truthy(POST_BUTTON_ENABLED_SCRIPT, "发帖按钮未就绪")
            .await;
        Ok(())
    }

    async fn click_publish(
        &self,
        page: &mut CdpPage,
        _content: PublishContent<'_>,
    ) -> Result<String> {
        // Ctrl+Enter is X's native send shortcut and the most reliable trigger:
        // focus the composer, fire it, and verify the composer cleared (= posted).
        page.evaluate(FOCUS_COMPOSER_SCRIPT).await?;
        sleep(Duration::from_millis(200)).await;
        page.press_ctrl_enter().await?;
        page.drain_dialog_events().await?;
        sleep(Duration::from_secs(2)).await;
        if self.composer_empty(page).await {
            return Ok("已通过 Ctrl+Enter 发布推文".to_string());
        }

        // Fall back to clicking the post button (stable testid, then localized text).
        for _ in 0..10 {
            for selector in PUBLISH_SELECTORS {
                if page.click_eval(&selector_center_script(selector)).await? {
                    page.drain_dialog_events().await?;
                    sleep(Duration::from_secs(2)).await;
                    if self.composer_empty(page).await {
                        return Ok(format!("已自动点击发帖按钮 ({selector})"));
                    }
                }
            }
            for label in PUBLISH_LABELS {
                if page.click_eval(&label_center_script(label)).await? {
                    page.drain_dialog_events().await?;
                    sleep(Duration::from_secs(2)).await;
                    if self.composer_empty(page).await {
                        return Ok(format!("已自动点击发布按钮：{label}"));
                    }
                }
            }
            sleep(Duration::from_millis(500)).await;
        }
        anyhow::bail!("发帖按钮点击后推文仍未发送，请手动确认")
    }
}

impl TwitterCdp {
    /// `true` once the composer has emptied — the signal that the tweet posted.
    async fn composer_empty(&self, page: &mut CdpPage) -> bool {
        page.evaluate(COMPOSER_EMPTY_SCRIPT)
            .await
            .ok()
            .and_then(|value| {
                value
                    .pointer("/result/value")
                    .and_then(serde_json::Value::as_bool)
            })
            .unwrap_or(false)
    }
}

/// Center `{x,y}` of the black sidebar 发帖 button (opens the compose modal).
const SIDENAV_POINT_SCRIPT: &str = r#"
(() => {
  const vis=(el)=>{const r=el.getBoundingClientRect();const s=getComputedStyle(el);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none';};
  const b=[...document.querySelectorAll('[data-testid="SideNav_NewTweet_Button"], a[href="/compose/post"], a[href="/compose/tweet"]')].filter(vis)[0];
  if(!b) return null;
  b.scrollIntoView({block:'center'});
  const r=b.getBoundingClientRect();
  return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
})()
"#;

/// The compose modal is open: a tweet textarea exists inside a dialog.
const MODAL_READY_SCRIPT: &str = r#"
(() => {
  const vis=(el)=>{const r=el.getBoundingClientRect();const s=getComputedStyle(el);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none';};
  return [...document.querySelectorAll('[data-testid="tweetTextarea_0"]')].some(t => vis(t) && t.closest('[role="dialog"]'));
})()
"#;

const POST_BUTTON_ENABLED_SCRIPT: &str = r#"
(() => {
  const b = document.querySelector('[role="dialog"] [data-testid="tweetButton"], [data-testid="tweetButton"], [data-testid="tweetButtonInline"]');
  return !!b && b.getAttribute('aria-disabled') !== 'true';
})()
"#;

const FOCUS_COMPOSER_SCRIPT: &str = r#"
(() => {
  const vis=(el)=>{const r=el.getBoundingClientRect();return r.width>0&&r.height>0;};
  const tas=[...document.querySelectorAll('[data-testid="tweetTextarea_0"]')].filter(vis);
  const el=tas.find(t=>t.closest('[role="dialog"]'))||tas[0];
  if (el) { el.focus(); return true; }
  return false;
})()
"#;

const COMPOSER_EMPTY_SCRIPT: &str = r#"
(() => {
  const vis=(el)=>{const r=el.getBoundingClientRect();return r.width>0&&r.height>0;};
  const tas=[...document.querySelectorAll('[data-testid="tweetTextarea_0"]')].filter(vis);
  const el=tas.find(t=>t.closest('[role="dialog"]'))||tas[0];
  if (!el) return true;
  return (el.innerText || el.textContent || '').trim().length === 0;
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
  const tas = Array.from(document.querySelectorAll('[data-testid="tweetTextarea_0"]')).filter(visible);
  const bodyEl = tas.find(t => t.closest('[role="dialog"]')) || tas[0]
    || Array.from(document.querySelectorAll('[role="textbox"][contenteditable="true"]')).filter(visible)[0];
  if (bodyEl) setText(bodyEl, text);
  return {{ message: `Twitter/X 草稿已填充：正文${{bodyEl ? '成功' : '未找到'}}。` }};
}})()
"#
    )
}
