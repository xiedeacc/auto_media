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
use crate::browser::cdp::{human_pause, CdpBrowser, CdpPage};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

/// Xueqiu statuses accept at most 3 topics.
const MAX_TOPICS: usize = 3;

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
        // Keep literal hashtags out of the text; add them as real #话题# tags.
        let text = join_text(content.title, &content.body_without_tags());
        page.evaluate(&fill_editable_script(STATUS_FIELD_SELECTORS, &text))
            .await?;
        let added = self.add_topics(page, &content.topic_keywords()).await;
        if added > 0 {
            Ok(format!("雪球动态正文已填充；已添加 {added} 个话题"))
        } else {
            Ok("雪球动态正文已填充".to_string())
        }
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

impl XueqiuCdp {
    /// Add each tag as a real Xueqiu `#话题#` tag: focus the composer at the end,
    /// type `#<keyword>`, wait for the `.mention-popup__item` list, and click the
    /// exact `#<keyword>#` item. No exact match → leave the typed text. Resolved
    /// keyword→tag titles are cached.
    async fn add_topics(&self, page: &mut CdpPage, keywords: &[String]) -> usize {
        if keywords.is_empty() {
            return 0;
        }
        let mut cache = self.load_topic_cache();
        let mut added = 0usize;
        // The composer already has trusted focus (prepare clicked it) and keeps it
        // across popup selections, so type `#topic` directly — a programmatic
        // refocus/selection here actually *breaks* Input.insertText.
        for keyword in keywords.iter().filter(|k| !k.is_empty()).take(MAX_TOPICS) {
            let _ = page.insert_text(" #").await;
            human_pause(300).await;
            let _ = page.type_text(keyword).await;
            human_pause(1500).await;
            if page
                .click_eval(&pick_topic_script(keyword))
                .await
                .unwrap_or(false)
            {
                added += 1;
                cache.insert(keyword.clone(), format!("#{keyword}#"));
            } else {
                let _ = page.insert_text(" ").await;
            }
            human_pause(500).await;
        }
        self.save_topic_cache(&cache);
        added
    }

    fn topic_cache_path(&self) -> PathBuf {
        self.profile_dir.join("xueqiu_topic_cache.json")
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

/// Return the center `{x,y}` of the topic-popup item whose text is exactly
/// `#<keyword>#` (Xueqiu wraps topics in hashes). Items are `.mention-popup__item`.
fn pick_topic_script(keyword: &str) -> String {
    let want = serde_json::to_string(&format!("#{keyword}#")).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const want={want};
  const vis=(el)=>{{const r=el.getBoundingClientRect();const s=getComputedStyle(el);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none';}};
  const items=[...document.querySelectorAll('.mention-popup__item')].filter(vis);
  const exact=items.find(e=>(e.innerText||'').trim()===want);
  if(!exact) return null;
  const r=exact.getBoundingClientRect();
  return {{ x: r.x + r.width / 2, y: r.y + r.height / 2 }};
}})()
"#
    )
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
