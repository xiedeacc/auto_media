//! Douyin (抖音) CDP backend — the verified path. Publishes an image-text note
//! (图文) via the creator upload page. Selectors are best-effort and centralized
//! here, so adapting to a creator-center UI change is a one-file edit.

use super::backend::{self, label_center_script, CdpFlow, PublishBackend, PublishContent};
use super::Platform;
use crate::browser::cdp::{CdpBrowser, CdpPage, DEFAULT_FILE_INPUT_SELECTORS};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

/// Cap how many topics we attach to a Douyin note.
const MAX_TOPICS: usize = 5;

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

    async fn fill_text(&self, page: &mut CdpPage, content: PublishContent<'_>) -> Result<String> {
        // Keep literal hashtags out of the description; add them as real topics.
        let body = content.body_without_tags();
        let result = page.evaluate(&fill_script(content.title, &body)).await?;
        let mut message = result
            .pointer("/result/value/message")
            .and_then(Value::as_str)
            .unwrap_or("抖音图文标题/正文已填充")
            .to_string();
        let added = self.add_topics(page, &content.topic_keywords()).await;
        if added > 0 {
            message.push_str(&format!("；已添加 {added} 个话题"));
        }
        Ok(message)
    }

    async fn click_publish(
        &self,
        page: &mut CdpPage,
        _content: PublishContent<'_>,
    ) -> Result<String> {
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

impl DouyinCdp {
    /// Add each tag as a real Douyin topic: click 「#添加话题」 (inserts `#` and
    /// focuses the editor), type the keyword, then click the exact-match
    /// suggestion in the `.mention-suggest-mount-dom` popup. No match → leave the
    /// literal hashtag (Douyin auto-links it) by committing a space.
    async fn add_topics(&self, page: &mut CdpPage, keywords: &[String]) -> usize {
        if keywords.is_empty() {
            return 0;
        }
        let mut cache = self.load_topic_cache();
        let mut added = 0usize;
        for keyword in keywords.iter().filter(|k| !k.is_empty()).take(MAX_TOPICS) {
            if !page.eval_bool(CLICK_ADD_TOPIC_SCRIPT).await.unwrap_or(false) {
                continue;
            }
            sleep(Duration::from_millis(700)).await;
            for ch in keyword.chars() {
                let _ = page.insert_text(&ch.to_string()).await;
                sleep(Duration::from_millis(120)).await;
            }
            sleep(Duration::from_millis(1600)).await;
            if page
                .click_eval(&pick_topic_script(keyword))
                .await
                .unwrap_or(false)
            {
                added += 1;
                cache.insert(keyword.clone(), format!("#{keyword}"));
            } else {
                let _ = page.insert_text(" ").await;
            }
            sleep(Duration::from_millis(500)).await;
        }
        self.save_topic_cache(&cache);
        added
    }

    fn topic_cache_path(&self) -> PathBuf {
        self.profile_dir.join("topic_cache.json")
    }

    fn load_topic_cache(&self) -> HashMap<String, String> {
        std::fs::read_to_string(self.topic_cache_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save_topic_cache(&self, cache: &HashMap<String, String>) {
        if let Ok(text) = serde_json::to_string_pretty(cache) {
            let _ = std::fs::write(self.topic_cache_path(), text);
        }
    }
}

/// JS-click the (smallest visible) "#添加话题" toolbar button.
const CLICK_ADD_TOPIC_SCRIPT: &str = r#"
(() => {
  const vis=(el)=>{const r=el.getBoundingClientRect();const s=getComputedStyle(el);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none';};
  const btn=[...document.querySelectorAll('div,button,span')].filter(vis)
    .filter(e=>(e.innerText||'').trim()==='#添加话题')
    .sort((a,b)=>{const ra=a.getBoundingClientRect(),rb=b.getBoundingClientRect();return ra.width*ra.height-rb.width*rb.height;})[0];
  if(!btn) return false;
  btn.scrollIntoView({block:'center'});
  btn.click();
  return true;
})()
"#;

/// Return the center `{x,y}` of the suggestion *row* whose topic equals
/// `#<keyword>` inside the `.mention-suggest-mount-dom` popup (exact match only).
/// Row classes carry per-session hashes and the list-container shares the same
/// first line, so match by an individual row's height (≈26–50px) and exact text
/// rather than by class — clicking the container's center would hit the wrong row.
fn pick_topic_script(keyword: &str) -> String {
    let want = serde_json::to_string(&format!("#{keyword}")).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const want={want};
  const vis=(el)=>{{const r=el.getBoundingClientRect();const s=getComputedStyle(el);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none';}};
  const mount=[...document.querySelectorAll('.mention-suggest-mount-dom')].filter(vis);
  const els=mount.flatMap(m=>[...m.querySelectorAll('*')]).filter(vis)
    .map(e=>({{e, r:e.getBoundingClientRect()}}))
    .filter(o=>o.r.height>=26 && o.r.height<=50 && ((o.e.innerText||'').trim().split('\n')[0])===want);
  if(!els.length) return null;
  els.sort((a,b)=>a.r.width*a.r.height - b.r.width*b.r.height);
  const r=els[0].r;
  return {{ x: r.x + r.width / 2, y: r.y + r.height / 2 }};
}})()
"#
    )
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
