//! Douyin (抖音) CDP backend — the verified path. Publishes an image-text note
//! (图文) via the creator upload page. Selectors are best-effort and centralized
//! here, so adapting to a creator-center UI change is a one-file edit.

use super::backend::{self, label_center_script, CdpFlow, PublishBackend, PublishContent};
use super::Platform;
use crate::browser::cdp::{CdpBrowser, CdpPage, DEFAULT_FILE_INPUT_SELECTORS};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

pub struct DouyinCdp {
    browser: CdpBrowser,
    port: u16,
    profile_dir: PathBuf,
    publish_url: String,
}

impl DouyinCdp {
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
impl PublishBackend for DouyinCdp {
    async fn publish(&self, content: PublishContent<'_>) -> Result<String> {
        backend::cdp_publish(
            &self.browser,
            Platform::Douyin,
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
impl CdpFlow for DouyinCdp {
    async fn prepare(&self, page: &mut CdpPage) -> Result<()> {
        page.wait_for_truthy("(() => document.readyState !== 'loading')()", "抖音页面未加载完成")
            .await?;
        // Select the image-text (图文) tab if the upload page shows mode tabs.
        page.evaluate(SELECT_IMAGE_TAB_SCRIPT).await?;
        page.wait_for_truthy(UPLOAD_READY_SCRIPT, "抖音图文上传控件未就绪")
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
            .unwrap_or("抖音图文标题/正文已填充")
            .to_string())
    }

    async fn click_publish(&self, page: &mut CdpPage) -> Result<String> {
        for label in ["发布", "发布作品", "立即发布"] {
            if page.click_eval(&label_center_script(label)).await? {
                page.drain_dialog_events().await?;
                sleep(Duration::from_secs(2)).await;
                return Ok(format!("已自动点击抖音发布按钮：{label}"));
            }
        }
        anyhow::bail!("没有找到抖音发布按钮")
    }
}

const SELECT_IMAGE_TAB_SCRIPT: &str = r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  // Only click the mode tab "发布图文" (verified safe). NEVER click "上传图文" — that
  // is the upload dropzone/button and clicking it opens a blocking native file
  // dialog. Images are attached by setting the <input type=file> directly via CDP.
  for (const el of Array.from(document.querySelectorAll('[role=tab], [class*="tab-item"], div, span, a'))) {
    const text = (el.innerText || el.textContent || '').replace(/\s+/g, '').trim();
    if (visible(el) && text === '发布图文' && !el.querySelector('input[type=file]')) { el.click(); return true; }
  }
  return true;
})()
"#;

const UPLOAD_READY_SCRIPT: &str = r#"
(() => Array.from(document.querySelectorAll('input[type=file]'))
  .some(el => /jpg|jpeg|png|webp|image/i.test(el.accept || '') || el.accept === ''))()
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
    el.dispatchEvent(new KeyboardEvent('keyup', {{ bubbles: true }}));
  }};
  const candidates = (selector) => Array.from(document.querySelectorAll(selector)).filter(visible);
  const byPlaceholder = (word) => candidates('input, textarea, [contenteditable=true]')
    .find(el => ((el.placeholder || el.getAttribute('data-placeholder') || '').includes(word)));

  let titleEl = byPlaceholder('标题') || candidates('input[type=text], input').find(visible);
  if (titleEl) setText(titleEl, title);

  let bodyEl = byPlaceholder('简介') || byPlaceholder('正文') || byPlaceholder('描述');
  if (!bodyEl) {{
    bodyEl = candidates('[contenteditable=true]').filter(el => el !== titleEl)[0]
      || candidates('textarea').filter(el => el !== titleEl)[0];
  }}
  if (bodyEl && body) setText(bodyEl, body);

  return {{ message: `抖音图文已填充：标题${{titleEl ? '成功' : '未找到'}}，正文${{bodyEl ? '成功' : '未找到'}}。` }};
}})()
"#
    )
}
