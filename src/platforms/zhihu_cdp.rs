//! Zhihu CDP (browser-automation) backend. The primary publish path. Drives the
//! Zhihu column editor: fill title/body, upload images, then the two-stage
//! publish → confirm flow (which navigates away, so `beforeunload` is accepted).

use super::backend::{self, CdpFlow, PublishBackend, PublishContent};
use super::Platform;
use crate::browser::cdp::{human_pause, CdpBrowser, CdpPage, DEFAULT_FILE_INPUT_SELECTORS};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

/// Zhihu articles accept at most 3 topics.
const MAX_TOPICS: usize = 3;

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

    async fn click_publish_inner(&self, page: &mut CdpPage, topics: &[String]) -> Result<String> {
        // The editor's bottom 发布 opens the 发布设置 panel (it does NOT post).
        if !page.click_eval(MAIN_PUBLISH_SCRIPT).await? {
            anyhow::bail!("没有找到知乎发布按钮");
        }
        page.drain_dialog_events().await?;
        let _ = page
            .wait_for_truthy(PANEL_READY_SCRIPT, "知乎发布设置面板未出现")
            .await;
        sleep(Duration::from_millis(600)).await;

        let added = self.add_topics(page, topics).await;

        // The panel's 发布/确认发布 actually posts.
        for _ in 0..20 {
            if page.click_eval(CONFIRM_PUBLISH_SCRIPT).await? {
                page.drain_dialog_events().await?;
                sleep(Duration::from_secs(2)).await;
                return Ok(format!("已设置 {added} 个话题并点击知乎发布确认按钮"));
            }
            sleep(Duration::from_millis(250)).await;
        }
        Ok(format!(
            "已设置 {added} 个话题并点击底部发布，未发现二次确认弹窗"
        ))
    }

    /// Add each tag as a real Zhihu topic via the 发布设置 panel: open the topic
    /// search, type the keyword, and click the exact-match suggestion. Resolved
    /// keyword→topic titles are cached so repeats select the same topic.
    async fn add_topics(&self, page: &mut CdpPage, keywords: &[String]) -> usize {
        if keywords.is_empty() {
            return 0;
        }
        let mut cache = self.load_topic_cache();
        let mut added = 0usize;
        for keyword in keywords.iter().filter(|k| !k.is_empty()).take(MAX_TOPICS) {
            let want = cache.get(keyword).cloned().unwrap_or_else(|| keyword.clone());
            if eval_bool(page, ADD_TOPIC_BUTTON_SCRIPT).await != Some(true) {
                continue;
            }
            human_pause(700).await;
            if eval_bool(page, &set_search_script(keyword)).await != Some(true) {
                continue;
            }
            human_pause(1600).await;
            match eval_string(page, &pick_topic_script(&want)).await {
                Some(title) if !title.is_empty() => {
                    added += 1;
                    cache.insert(keyword.clone(), title);
                }
                _ => {
                    let _ = page.evaluate(CLEAR_SEARCH_SCRIPT).await;
                }
            }
            human_pause(600).await;
        }
        self.save_topic_cache(&cache);
        added
    }

    fn topic_cache_path(&self) -> PathBuf {
        self.profile_dir.join("zhihu_topic_cache.json")
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

/// Evaluate a script and read its boolean `/result/value`.
async fn eval_bool(page: &mut CdpPage, script: &str) -> Option<bool> {
    page.evaluate(script)
        .await
        .ok()?
        .pointer("/result/value")
        .and_then(Value::as_bool)
}

/// Evaluate a script and read its string `/result/value`.
async fn eval_string(page: &mut CdpPage, script: &str) -> Option<String> {
    page.evaluate(script)
        .await
        .ok()?
        .pointer("/result/value")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
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

    async fn fill_text(&self, page: &mut CdpPage, content: PublishContent<'_>) -> Result<String> {
        // Tags become real topics in the publish panel, so keep them out of the body.
        let body = content.body_without_tags();
        let result = page.evaluate(&fill_script(content.title, &body)).await?;
        Ok(result
            .pointer("/result/value/message")
            .and_then(Value::as_str)
            .unwrap_or("知乎草稿已填充")
            .to_string())
    }

    async fn click_publish(
        &self,
        page: &mut CdpPage,
        content: PublishContent<'_>,
    ) -> Result<String> {
        page.set_accept_beforeunload(true);
        let result = self
            .click_publish_inner(page, &content.topic_keywords())
            .await;
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

const PANEL_READY_SCRIPT: &str = r#"
(() => Boolean([...document.querySelectorAll('button,[role=button]')].some(e => (e.innerText || '').trim() === '添加话题')))()
"#;

/// Click the (smallest visible) "添加话题" button to reveal the topic search box.
const ADD_TOPIC_BUTTON_SCRIPT: &str = r#"
(() => {
  const vis=(el)=>{const r=el.getBoundingClientRect();const s=getComputedStyle(el);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none';};
  const btns=[...document.querySelectorAll('button,[role=button]')].filter(vis)
    .filter(e=>(e.innerText||'').trim()==='添加话题')
    .sort((a,b)=>{const ra=a.getBoundingClientRect(),rb=b.getBoundingClientRect();return ra.width*ra.height-rb.width*rb.height;});
  if(!btns[0]) return false;
  btns[0].scrollIntoView({block:'center'});
  btns[0].click();
  return true;
})()
"#;

/// Clear the topic search field (after a no-match keyword) for the next attempt.
const CLEAR_SEARCH_SCRIPT: &str = r#"
(() => {
  const el=[...document.querySelectorAll('input')].find(e=>e.getAttribute('placeholder')==='搜索话题...');
  if(!el) return false;
  const p=Object.getPrototypeOf(el);
  Object.getOwnPropertyDescriptor(p,'value').set.call(el,'');
  el.dispatchEvent(new InputEvent('input',{bubbles:true}));
  return true;
})()
"#;

/// Set the topic search field to `keyword` and fire the input event so Zhihu
/// fetches matching topic suggestions.
fn set_search_script(keyword: &str) -> String {
    let kw = serde_json::to_string(keyword).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const kw={kw};
  const el=[...document.querySelectorAll('input')].find(e=>e.getAttribute('placeholder')==='搜索话题...');
  if(!el) return false;
  el.focus();
  const p=Object.getPrototypeOf(el);
  Object.getOwnPropertyDescriptor(p,'value').set.call(el,kw);
  el.dispatchEvent(new InputEvent('input',{{bubbles:true,inputType:'insertText',data:kw}}));
  el.dispatchEvent(new KeyboardEvent('keyup',{{bubbles:true}}));
  return true;
}})()
"#
    )
}

/// Click the suggestion button whose text exactly equals `want` (a real topic);
/// returns the chosen title, or null if there's no exact match (avoids garbage
/// topics). Suggestions render as `<button>`s inside a `.Popover-content` popup.
fn pick_topic_script(want: &str) -> String {
    let want = serde_json::to_string(want).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const want={want};
  const vis=(el)=>{{const r=el.getBoundingClientRect();const s=getComputedStyle(el);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none';}};
  const pops=[...document.querySelectorAll('.Popover-content')].filter(vis);
  const btns=pops.flatMap(p=>[...p.querySelectorAll('button')]).filter(vis);
  const exact=btns.find(b=>(b.innerText||'').trim()===want);
  if(!exact) return null;
  const chosen=(exact.innerText||'').trim();
  exact.click();
  return chosen;
}})()
"#
    )
}

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
      return { el, label, cx: rect.x + rect.width / 2, cy: rect.y + rect.height / 2, area: rect.width * rect.height };
    })
    .filter(item => item.label === '发布' && item.area >= 500 && item.area <= 12000)
    // The editor's primary 发布 (opens the publish panel) is the bottom-right one.
    .sort((a, b) => (b.cy - a.cy) || (b.cx - a.cx) || (a.area - b.area));
  const top = candidates[0];
  if (!top) return null;
  top.el.scrollIntoView({ block: 'center' });
  const rect = top.el.getBoundingClientRect();
  return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
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
      return { el, label, cx: rect.x + rect.width / 2, cy: rect.y + rect.height / 2, area: rect.width * rect.height };
    })
    .filter(item => ['发布', '确认发布', '发布文章'].includes(item.label) && item.area >= 500 && item.area <= 20000)
    .sort((a, b) => b.cy - a.cy || b.cx - a.cx || a.area - b.area);
  const top = candidates[0];
  if (!top) return null;
  top.el.scrollIntoView({ block: 'center' });
  const rect = top.el.getBoundingClientRect();
  return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
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
