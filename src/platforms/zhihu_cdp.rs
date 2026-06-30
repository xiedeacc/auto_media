//! Zhihu CDP (browser-automation) backend. The primary publish path. Drives the
//! Zhihu column editor: fill title/body, upload images, then the two-stage
//! publish → confirm flow (which navigates away, so `beforeunload` is accepted).

use super::backend::{self, CdpFlow, PublishBackend, PublishContent};
use super::Platform;
use crate::browser::cdp::{CdpBrowser, CdpPage, DEFAULT_FILE_INPUT_SELECTORS};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

pub struct ZhihuCdp {
    browser: CdpBrowser,
    port: u16,
    profile_dir: PathBuf,
    publish_url: String,
}

impl ZhihuCdp {
    pub fn new(port: u16, profile_dir: PathBuf, publish_url: String) -> Self {
        Self {
            browser: CdpBrowser,
            port,
            profile_dir,
            publish_url,
        }
    }

    async fn click_publish_inner(&self, page: &mut CdpPage) -> Result<String> {
        if !page.click_eval(MAIN_PUBLISH_SCRIPT).await? {
            anyhow::bail!("没有找到知乎发布按钮");
        }
        page.drain_dialog_events().await?;
        sleep(Duration::from_millis(300)).await;

        for _ in 0..20 {
            if page.click_eval(CONFIRM_PUBLISH_SCRIPT).await? {
                page.drain_dialog_events().await?;
                sleep(Duration::from_secs(2)).await;
                return Ok("已自动点击知乎发布确认按钮".to_string());
            }
            sleep(Duration::from_millis(250)).await;
        }
        Ok("已自动点击知乎底部发布按钮，未发现二次确认弹窗".to_string())
    }
}

#[async_trait]
impl PublishBackend for ZhihuCdp {
    async fn publish(&self, content: PublishContent<'_>) -> Result<String> {
        backend::cdp_publish(
            &self.browser,
            Platform::Zhihu,
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
impl CdpFlow for ZhihuCdp {
    fn fill_before_upload(&self) -> bool {
        true
    }

    async fn prepare(&self, page: &mut CdpPage) -> Result<()> {
        page.wait_for_truthy(READY_SCRIPT, "知乎发布页关键控件未加载完成")
            .await
    }

    async fn upload_images(&self, page: &mut CdpPage, images: &[PathBuf]) -> Result<String> {
        page.set_file_input(DEFAULT_FILE_INPUT_SELECTORS, images)
            .await?;
        Ok(format!("已提交 {} 张图片到上传控件", images.len()))
    }

    async fn fill_text(&self, page: &mut CdpPage, title: &str, body: &str) -> Result<String> {
        let result = page.evaluate(&fill_script(title, body)).await?;
        Ok(result
            .pointer("/result/value/message")
            .and_then(Value::as_str)
            .unwrap_or("知乎草稿已填充")
            .to_string())
    }

    async fn click_publish(&self, page: &mut CdpPage) -> Result<String> {
        page.set_accept_beforeunload(true);
        let result = self.click_publish_inner(page).await;
        page.set_accept_beforeunload(false);
        result
    }
}

const READY_SCRIPT: &str = r#"
(() => Boolean(
  document.querySelector('textarea[placeholder*="标题"]') &&
  document.querySelector('.public-DraftEditor-content[contenteditable=true]')
))()
"#;

const MAIN_PUBLISH_SCRIPT: &str = r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && rect.x >= 0 && rect.y >= 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  const disabled = (el) => {
    const aria = el.getAttribute('aria-disabled');
    const cls = String(el.className || '').toLowerCase();
    return el.disabled || aria === 'true' || cls.includes('disabled');
  };
  const candidates = Array.from(document.querySelectorAll('button, [role=button]'))
    .filter(visible)
    .filter(el => !disabled(el))
    .map(el => {
      const label = (el.innerText || el.textContent || '').trim();
      const rect = el.getBoundingClientRect();
      return { label, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, area: rect.width * rect.height };
    })
    .filter(item => item.label === '发布' && item.area >= 500 && item.area <= 12000 && item.y > window.innerHeight - 90)
    .sort((a, b) => b.x - a.x || b.y - a.y || a.area - b.area);
  return candidates[0] || null;
})()
"#;

const CONFIRM_PUBLISH_SCRIPT: &str = r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && rect.x >= 0 && rect.y >= 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  const disabled = (el) => {
    const aria = el.getAttribute('aria-disabled');
    const cls = String(el.className || '').toLowerCase();
    return el.disabled || aria === 'true' || cls.includes('disabled');
  };
  const roots = Array.from(document.querySelectorAll('[role=dialog], .Modal, .Dialog, .modal, .popover'))
    .filter(visible);
  const searchRoots = roots.length ? roots : [document.body];
  const candidates = searchRoots.flatMap(root => Array.from(root.querySelectorAll('button, [role=button]')))
    .filter(visible)
    .filter(el => !disabled(el))
    .map(el => {
      const label = (el.innerText || el.textContent || '').replace(/\s+/g, '').trim();
      const rect = el.getBoundingClientRect();
      return { label, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, area: rect.width * rect.height };
    })
    .filter(item => ['发布', '确认发布', '发布文章'].includes(item.label) && item.area >= 500 && item.area <= 20000)
    .sort((a, b) => b.y - a.y || b.x - a.x || a.area - b.area);
  return candidates[0] || null;
})()
"#;

fn fill_script(title: &str, body: &str) -> String {
    let title = serde_json::to_string(title).unwrap_or_else(|_| "\"\"".to_string());
    let body = serde_json::to_string(body).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const title = {title};
  const body = {body};
  const visible = (el) => {{
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }};
  const fire = (el, eventName) => el.dispatchEvent(new Event(eventName, {{ bubbles: true }}));
  const setNativeValue = (el, value) => {{
    const proto = Object.getPrototypeOf(el);
    const descriptor = Object.getOwnPropertyDescriptor(proto, 'value');
    if (descriptor && descriptor.set) descriptor.set.call(el, value);
    else el.value = value;
    el.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: value }}));
    fire(el, 'change');
  }};
  const titleEl = Array.from(document.querySelectorAll('textarea[placeholder*="标题"], input[placeholder*="标题"]'))
    .filter(visible)[0];
  if (titleEl) setNativeValue(titleEl, title);

  const editors = Array.from(document.querySelectorAll('.public-DraftEditor-content[contenteditable=true], [contenteditable=true]'))
    .filter(visible)
    .filter(el => !el.closest('[placeholder*="标题"]'));
  const bodyEl = editors[0];
  if (bodyEl) {{
    bodyEl.focus();
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(bodyEl);
    range.collapse(true);
    selection.removeAllRanges();
    selection.addRange(range);
    document.execCommand('insertText', false, body);
    bodyEl.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: body }}));
    fire(bodyEl, 'change');
    bodyEl.blur();
  }}

  return {{ message: `知乎草稿已填充：标题${{titleEl ? '成功' : '未找到'}}，正文${{bodyEl ? '成功' : '未找到'}}。` }};
}})()
"#
    )
}
