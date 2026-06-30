//! Xiaohongshu CDP (browser-automation) backend. The primary publish path.
//! All Xiaohongshu-specific selectors and the publish flow live here, so a
//! creator-center UI change is a one-file edit.

use super::backend::{self, CdpFlow, PublishBackend, PublishContent};
use super::Platform;
use crate::browser::cdp::{CdpBrowser, CdpPage, DEFAULT_FILE_INPUT_SELECTORS};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

/// Canonical image-note publish entry. The creator center routes other publish
/// URLs through onboarding, so we force the `target=image` flow.
const CANONICAL_PUBLISH_URL: &str =
    "https://creator.xiaohongshu.com/publish/publish?from=menu&target=image";

pub struct XhsCdp {
    browser: CdpBrowser,
    port: u16,
    profile_dir: PathBuf,
    publish_url: String,
}

impl XhsCdp {
    pub fn new(port: u16, profile_dir: PathBuf, publish_url: String) -> Self {
        let publish_url = if publish_url.contains("/publish/publish") {
            CANONICAL_PUBLISH_URL.to_string()
        } else {
            publish_url
        };
        Self {
            browser: CdpBrowser,
            port,
            profile_dir,
            publish_url,
        }
    }
}

#[async_trait]
impl PublishBackend for XhsCdp {
    async fn publish(&self, content: PublishContent<'_>) -> Result<String> {
        backend::cdp_publish(
            &self.browser,
            Platform::Xhs,
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
impl CdpFlow for XhsCdp {
    async fn prepare(&self, page: &mut CdpPage) -> Result<()> {
        page.wait_for_truthy(READY_SCRIPT, "小红书发布页关键控件未加载完成")
            .await?;
        if !page.click_eval(IMAGE_TAB_SCRIPT).await? {
            anyhow::bail!("没有找到小红书顶部上传图文 tab");
        }
        page.wait_for_truthy(UPLOAD_READY_SCRIPT, "小红书图片上传控件未加载完成")
            .await?;
        Ok(())
    }

    async fn upload_images(&self, page: &mut CdpPage, images: &[PathBuf]) -> Result<String> {
        page.set_file_input(DEFAULT_FILE_INPUT_SELECTORS, images)
            .await?;
        Ok(format!("已提交 {} 张图片到上传控件", images.len()))
    }

    async fn fill_text(&self, page: &mut CdpPage, content: PublishContent<'_>) -> Result<String> {
        let result = page
            .evaluate(&fill_script(content.title, content.body))
            .await?;
        Ok(result
            .pointer("/result/value/message")
            .and_then(Value::as_str)
            .unwrap_or("已尝试填充标题/正文")
            .to_string())
    }

    async fn wait_publish_ready(&self, page: &mut CdpPage) -> Result<()> {
        // The submit button is gated on image processing; poll up to ~60s.
        for _ in 0..120 {
            let result = page.evaluate(PUBLISH_READY_SCRIPT).await?;
            if result
                .pointer("/result/value")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Ok(());
            }
            sleep(Duration::from_millis(500)).await;
        }
        anyhow::bail!("小红书发布按钮未就绪")
    }

    async fn click_publish(
        &self,
        page: &mut CdpPage,
        _content: PublishContent<'_>,
    ) -> Result<String> {
        for label in ["发布", "确认发布", "立即发布"] {
            let result = page.evaluate(&publish_script(label)).await?;
            if result
                .pointer("/result/value/clicked")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                page.drain_dialog_events().await?;
                sleep(Duration::from_secs(2)).await;
                return Ok(format!("已自动点击小红书底部发布按钮：{label}"));
            }
        }
        anyhow::bail!("没有找到小红书底部发布按钮")
    }
}

const READY_SCRIPT: &str = r#"
(() => Array.from(document.querySelectorAll('.creator-tab'))
  .some(el => (el.innerText || el.textContent || '').trim() === '上传图文'))()
"#;

const UPLOAD_READY_SCRIPT: &str = r#"
(() => Array.from(document.querySelectorAll('input[type=file]'))
  .some(el => /jpg|jpeg|png|webp|image/i.test(el.accept || '')))()
"#;

const IMAGE_TAB_SCRIPT: &str = r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && rect.x >= 0 && rect.y >= 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  const tabs = Array.from(document.querySelectorAll('.creator-tab'))
    .filter(visible)
    .map(el => {
      const label = (el.innerText || el.textContent || '').trim();
      const rect = el.getBoundingClientRect();
      return { label, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, area: rect.width * rect.height };
    })
    .filter(item => item.label === '上传图文' && item.area >= 1000)
    .sort((a, b) => a.area - b.area);
  return tabs[0] || null;
})()
"#;

const PUBLISH_READY_SCRIPT: &str = r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  const disabled = (el) => {
    const aria = el.getAttribute('aria-disabled');
    const cls = String(el.className || '').toLowerCase();
    return el.disabled || aria === 'true' || cls.includes('disabled') || el.getAttribute('submit-disabled') === 'true';
  };
  const host = Array.from(document.querySelectorAll('xhs-publish-btn'))
    .filter(visible)
    .find(el => ['发布', '确认发布', '立即发布'].includes((el.getAttribute('submit-text') || '').trim()) && !disabled(el));
  if (host) return true;
  return Array.from(document.querySelectorAll('button, [role=button], div'))
    .filter(visible)
    .some(el => ['发布', '确认发布', '立即发布'].includes((el.innerText || el.textContent || '').replace(/\s+/g, '').trim()) && !disabled(el));
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
  const byPlaceholder = (word) => candidates('input, textarea').find(el => ((el.placeholder || '').includes(word)));
  const editable = () => candidates('[contenteditable=true], [contenteditable="true"]');

  let titleEl = byPlaceholder('标题') || candidates('input, textarea').find(visible);
  if (titleEl) setText(titleEl, title);

  let bodyEl = byPlaceholder('正文');
  if (!bodyEl) {{
    const editors = editable().filter(el => el !== titleEl);
    bodyEl = editors.find(el => (el.innerText || el.textContent || '').includes('正文')) || editors[0];
  }}
  if (!bodyEl) {{
    bodyEl = candidates('textarea').filter(el => el !== titleEl)[0];
  }}
  if (bodyEl && body) setText(bodyEl, body);

  return {{ message: `已尝试填充草稿：标题${{titleEl ? '成功' : '未找到'}}，正文${{bodyEl ? '成功' : '未找到'}}。` }};
}})()
"#
    )
}

fn publish_script(text: &str) -> String {
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(async () => {{
  const wanted = {text};
  const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));
  const visible = (el) => {{
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }};
  const disabled = (el) => {{
    const aria = el.getAttribute('aria-disabled');
    const cls = String(el.className || '').toLowerCase();
    return el.disabled || aria === 'true' || cls.includes('disabled');
  }};
  const textOf = (el) => (el.innerText || el.textContent || '').replace(/\s+/g, '').trim();
  const clickable = (el) => el.closest('button, [role=button]') || el;
  const fire = async (el) => {{
    el.focus?.();
    const rect = el.getBoundingClientRect();
    const x = rect.x + rect.width / 2;
    const y = rect.y + rect.height / 2;
    const mouseOptions = {{ bubbles: true, cancelable: true, composed: true, view: window, clientX: x, clientY: y }};
    const pointerOptions = {{ ...mouseOptions, pointerId: 1, pointerType: 'mouse', isPrimary: true }};
    for (const type of ['pointerover', 'pointerenter', 'mouseover', 'mouseenter', 'pointerdown']) el.dispatchEvent(new PointerEvent(type, pointerOptions));
    el.dispatchEvent(new MouseEvent('mousedown', mouseOptions));
    for (const type of ['pointerup', 'mouseup', 'click']) el.dispatchEvent(new MouseEvent(type, mouseOptions));
    el.click?.();
    for (const name of ['submit', 'publish', 'confirm', 'click-submit']) el.dispatchEvent(new CustomEvent(name, {{ bubbles: true, cancelable: true, composed: true, detail: {{ source: 'auto_media' }} }}));
    await sleep(300);
    return {{ x, y }};
  }};

  const host = Array.from(document.querySelectorAll('xhs-publish-btn'))
    .filter(visible)
    .filter(el => (el.getAttribute('submit-text') || '').trim() === wanted)
    .filter(el => (el.getAttribute('submit-disabled') || '').trim() !== 'true')
    .sort((a, b) => {{ const ar = a.getBoundingClientRect(); const br = b.getBoundingClientRect(); return br.y - ar.y || br.x - ar.x; }})[0];
  if (host) {{ const p = await fire(host); return {{ clicked: true, label: wanted, x: p.x, y: p.y }}; }}

  const item = Array.from(document.querySelectorAll('button, [role=button], div'))
    .filter(visible)
    .filter(el => !disabled(clickable(el)))
    .map(el => {{ const target = clickable(el); const rect = target.getBoundingClientRect(); return {{ el: target, label: textOf(target), x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, area: rect.width * rect.height }}; }})
    .filter(it => it.label === wanted && it.area >= 300 && it.area <= 60000)
    .sort((a, b) => b.y - a.y || b.x - a.x || a.area - b.area)[0];
  if (!item) return {{ clicked: false }};
  const p = await fire(item.el);
  return {{ clicked: true, label: item.label, x: p.x, y: p.y }};
}})()
"#
    )
}
