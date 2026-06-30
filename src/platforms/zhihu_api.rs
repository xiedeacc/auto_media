use super::backend::{CookieStore, PublishBackend, PublishContent};
use crate::{browser::cdp::BrowserCookie, topic_cache};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use std::sync::Arc;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE};
use serde_json::{json, Value};
use sha1::Sha1;
use std::path::{Path, PathBuf};

const API_V4: &str = "https://www.zhihu.com/api/v4";
const ZHUANLAN_API: &str = "https://zhuanlan.zhihu.com/api";
const IMAGE_API: &str = "https://api.zhihu.com/images";
const OSS_UPLOAD_URL: &str = "https://zhihu-pics-upload.zhimg.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";
const MAX_ARTICLE_TOPICS: usize = 3;

type HmacSha1 = Hmac<Sha1>;

/// HTTP API backend for Zhihu: image OSS upload + draft + publish + topic sync.
pub struct ZhihuApi {
    cookies: Arc<CookieStore>,
    topic_cache: PathBuf,
}

impl ZhihuApi {
    pub fn new(cookies: Arc<CookieStore>, topic_cache: PathBuf) -> Self {
        Self {
            cookies,
            topic_cache,
        }
    }
}

#[async_trait]
impl PublishBackend for ZhihuApi {
    async fn publish(&self, content: PublishContent<'_>) -> Result<String> {
        let cookies = self.cookies.load_or_capture().await?;
        let message = publish_image_article(
            &cookies,
            content.title,
            content.body,
            content.image_paths,
            Some(&self.topic_cache),
        )
        .await?;
        tracing::warn!(
            image_count = content.image_paths.len(),
            message = %message,
            "zhihu api article submitted"
        );
        Ok(message)
    }
}

pub async fn publish_image_article(
    cookies: &[BrowserCookie],
    title: &str,
    body: &str,
    image_paths: &[PathBuf],
    topic_cache_path: Option<&Path>,
) -> Result<String> {
    if body.chars().count() < 9 {
        anyhow::bail!("知乎正文至少需要 9 个字");
    }
    let session = ZhihuSession::new(cookies)?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    let topics = session
        .resolve_topics(&client, body, topic_cache_path)
        .await?;
    let mut image_infos = Vec::new();
    for path in image_paths {
        image_infos.push(session.upload_image(&client, path).await?);
    }
    let response = session
        .create_article(&client, title, body, &image_infos)
        .await?;
    let mut message = format!("知乎 API 已提交发布：{}", compact_json(&response));
    if !topics.is_empty() {
        if let Some(article_id) = extract_article_id(&response) {
            match session
                .sync_article_topics(&client, &article_id, &topics)
                .await
            {
                Ok(_) => {
                    message.push_str(&format!("；已同步 {} 个知乎话题", topics.len()));
                }
                Err(error) => {
                    tracing::warn!(
                        article_id = %article_id,
                        topic_count = topics.len(),
                        error = %error,
                        "zhihu article topic sync failed after publish"
                    );
                    message.push_str(&format!("；文章已发布，但知乎话题同步失败：{error:#}"));
                }
            }
        } else {
            tracing::warn!(
                response = %compact_json(&response),
                "zhihu article id missing, topic sync skipped"
            );
            message.push_str("；文章已发布，但响应缺少文章 id，未同步知乎话题");
        }
    }
    Ok(message)
}

struct ZhihuSession {
    cookie_header: String,
    xsrf: String,
}

struct RegisteredImage {
    image_id: String,
    state: i64,
    object_key: Option<String>,
    upload_token: Option<Value>,
}

enum ImagePollOutcome {
    Ready(ImageInfo),
    Timeout {
        image_id: String,
        last_response: Value,
    },
}

impl ZhihuSession {
    fn new(cookies: &[BrowserCookie]) -> Result<Self> {
        let filtered = cookies
            .iter()
            .filter(|cookie| cookie.domain.contains("zhihu.com"))
            .collect::<Vec<_>>();
        let find = |name: &str| {
            filtered
                .iter()
                .find(|cookie| cookie.name == name)
                .map(|cookie| cookie.value.clone())
        };
        let xsrf = find("_xsrf").ok_or_else(|| anyhow!("知乎缺少 _xsrf cookie"))?;
        for required in ["z_c0", "d_c0"] {
            if find(required).is_none() {
                anyhow::bail!("知乎缺少 {required} cookie");
            }
        }
        let cookie_header = filtered
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");
        Ok(Self {
            cookie_header,
            xsrf,
        })
    }

    async fn upload_image(&self, client: &reqwest::Client, path: &Path) -> Result<ImageInfo> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read image {}", path.display()))?;
        let content_type = image_content_type(path)?;
        let image_hash = format!("{:x}", md5::compute(&bytes));
        for register_attempt in 1..=2 {
            let registered = self
                .register_image(client, &image_hash)
                .await
                .with_context(|| format!("知乎图片注册失败: {}", path.display()))?;
            tracing::warn!(
                image_hash,
                register_attempt,
                image_id = %registered.image_id,
                state = registered.state,
                object_key = registered.object_key.as_deref().unwrap_or(""),
                "zhihu image registered"
            );
            if registered.state == 2 {
                let object_key = registered
                    .object_key
                    .as_deref()
                    .ok_or_else(|| anyhow!("知乎图片注册响应缺少 object_key"))?;
                let token = registered
                    .upload_token
                    .as_ref()
                    .ok_or_else(|| anyhow!("知乎图片注册响应缺少 upload_token"))?;
                self.upload_to_oss(client, object_key, &bytes, content_type, token)
                    .await?;
                tracing::warn!(
                    image_hash,
                    register_attempt,
                    image_id = %registered.image_id,
                    object_key,
                    content_type,
                    "zhihu image uploaded to oss"
                );
                if let Some(confirmed) = self
                    .confirm_uploaded_image(client, &image_hash, register_attempt)
                    .await?
                {
                    match self.poll_image(client, &confirmed.image_id).await? {
                        ImagePollOutcome::Ready(info) => return Ok(info),
                        ImagePollOutcome::Timeout {
                            image_id,
                            last_response,
                        } if register_attempt < 2
                            && last_response.get("status").and_then(Value::as_str)
                                == Some("init") =>
                        {
                            tracing::warn!(
                                image_hash,
                                image_id = %image_id,
                                last_response = %compact_json(&last_response),
                                "zhihu confirmed image stayed init after polling, re-registering same hash"
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            continue;
                        }
                        ImagePollOutcome::Timeout {
                            image_id,
                            last_response,
                        } => {
                            anyhow::bail!(
                                "知乎图片处理超时 image_id={image_id}，最后响应: {}",
                                compact_json(&last_response)
                            );
                        }
                    }
                }
            } else if registered.state != 1 {
                anyhow::bail!("知乎图片注册返回未知 state={}", registered.state);
            }

            match self.poll_image(client, &registered.image_id).await? {
                ImagePollOutcome::Ready(info) => return Ok(info),
                ImagePollOutcome::Timeout {
                    image_id,
                    last_response,
                } if register_attempt < 2
                    && last_response.get("status").and_then(Value::as_str) == Some("init") =>
                {
                    tracing::warn!(
                        image_hash,
                        image_id = %image_id,
                        last_response = %compact_json(&last_response),
                        "zhihu image stayed init after polling, re-registering same hash"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                ImagePollOutcome::Timeout {
                    image_id,
                    last_response,
                } => {
                    anyhow::bail!(
                        "知乎图片处理超时 image_id={image_id}，最后响应: {}",
                        compact_json(&last_response)
                    );
                }
            }
        }
        anyhow::bail!("知乎图片处理超时: {}", path.display())
    }

    async fn register_image(
        &self,
        client: &reqwest::Client,
        image_hash: &str,
    ) -> Result<RegisteredImage> {
        let register = client
            .post(IMAGE_API)
            .headers(self.headers()?)
            .json(&json!({ "image_hash": image_hash, "source": "article" }))
            .send()
            .await?;
        let data = handle_response(register).await?;
        let upload_file = data
            .get("upload_file")
            .ok_or_else(|| anyhow!("知乎图片注册响应缺少 upload_file"))?;
        let image_id = upload_file
            .get("image_id")
            .and_then(|value| {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| value.as_i64().map(|id| id.to_string()))
            })
            .ok_or_else(|| anyhow!("知乎图片注册响应缺少 image_id"))?;
        let state = upload_file
            .get("state")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let object_key = upload_file
            .get("object_key")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let upload_token = data.get("upload_token").cloned();
        Ok(RegisteredImage {
            image_id,
            state,
            object_key,
            upload_token,
        })
    }

    async fn confirm_uploaded_image(
        &self,
        client: &reqwest::Client,
        image_hash: &str,
        register_attempt: usize,
    ) -> Result<Option<RegisteredImage>> {
        for confirm_attempt in 1..=3 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let confirmed = self.register_image(client, image_hash).await?;
            tracing::warn!(
                image_hash,
                register_attempt,
                confirm_attempt,
                image_id = %confirmed.image_id,
                state = confirmed.state,
                object_key = confirmed.object_key.as_deref().unwrap_or(""),
                "zhihu image confirm registration after oss upload"
            );
            if confirmed.state == 1 {
                return Ok(Some(confirmed));
            }
        }
        Ok(None)
    }

    async fn upload_to_oss(
        &self,
        client: &reqwest::Client,
        object_key: &str,
        bytes: &[u8],
        content_type: &'static str,
        token: &Value,
    ) -> Result<()> {
        let access_token = token
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("知乎 OSS token 缺少 access_token"))?;
        let access_id = token
            .get("access_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("知乎 OSS token 缺少 access_id"))?;
        let access_key = token
            .get("access_key")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("知乎 OSS token 缺少 access_key"))?;
        let date = chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let string_to_sign = format!(
            "PUT\n\n{content_type}\n{date}\nx-oss-security-token:{access_token}\n/zhihu-pics/{object_key}"
        );
        let mut mac = HmacSha1::new_from_slice(access_key.as_bytes())?;
        mac.update(string_to_sign.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        let response = client
            .put(format!("{OSS_UPLOAD_URL}/{object_key}"))
            .header(CONTENT_TYPE, content_type)
            .header("Date", date)
            .header("x-oss-security-token", access_token)
            .header("Authorization", format!("OSS {access_id}:{signature}"))
            .body(bytes.to_vec())
            .send()
            .await
            .context("知乎 OSS 上传失败")?;
        if !response.status().is_success() {
            anyhow::bail!("知乎 OSS 上传失败: {}", response.status());
        }
        Ok(())
    }

    async fn poll_image(
        &self,
        client: &reqwest::Client,
        image_id: &str,
    ) -> Result<ImagePollOutcome> {
        let mut last_response = Value::Null;
        for attempt in 1..=15 {
            let response = client
                .get(format!("{IMAGE_API}/{image_id}"))
                .headers(self.headers()?)
                .send()
                .await
                .context("知乎图片处理状态查询失败")?;
            let data = handle_response(response).await?;
            last_response = data.clone();
            let status = data
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            tracing::debug!(
                image_id,
                attempt,
                status,
                response = %compact_json(&data),
                "zhihu image processing polled"
            );
            if status == "success" {
                return Ok(ImagePollOutcome::Ready(ImageInfo {
                    src: required_str(&data, "src")?,
                    original_src: data
                        .get("original_src")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| data.get("src").and_then(Value::as_str).unwrap_or(""))
                        .to_string(),
                    watermark: data
                        .get("watermark")
                        .and_then(Value::as_str)
                        .unwrap_or("watermark")
                        .to_string(),
                    watermark_src: data
                        .get("watermark_src")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                }));
            }
            if matches!(status, "failed" | "error" | "fail") {
                anyhow::bail!(
                    "知乎图片处理失败 image_id={image_id}: {}",
                    compact_json(&data)
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        Ok(ImagePollOutcome::Timeout {
            image_id: image_id.to_string(),
            last_response,
        })
    }

    async fn create_article(
        &self,
        client: &reqwest::Client,
        title: &str,
        body: &str,
        image_infos: &[ImageInfo],
    ) -> Result<Value> {
        let draft_id = self.create_content_draft(client).await?;
        let html = format!(
            "<p>{}</p>{}",
            html_escape(body),
            build_img_html(image_infos)
        );
        let payload = json!({
            "action": "article",
            "data": {
                "title": { "title": title },
                "hybrid": { "html": html, "textLength": body.chars().count() },
                "extra_info": { "publisher": "pc" },
                "draft": { "disabled": 1, "id": draft_id },
                "commentsPermission": { "comment_permission": "anyone" }
            }
        });
        let response = client
            .post(format!("{API_V4}/content/publish"))
            .headers(self.headers()?)
            .header("x-requested-with", "fetch")
            .json(&payload)
            .send()
            .await
            .context("知乎发布文章失败")?;
        handle_response(response).await
    }

    async fn resolve_topics(
        &self,
        client: &reqwest::Client,
        body: &str,
        topic_cache_path: Option<&Path>,
    ) -> Result<Vec<topic_cache::ZhihuTopicEntry>> {
        let mut topics: Vec<topic_cache::ZhihuTopicEntry> = Vec::new();
        for name in extract_tag_names(body).into_iter().take(MAX_ARTICLE_TOPICS) {
            if let Some(topic) = self.search_topic(client, &name, topic_cache_path).await? {
                if !topics.iter().any(|existing| existing.id == topic.id) {
                    topics.push(topic);
                }
            }
        }
        Ok(topics)
    }

    async fn search_topic(
        &self,
        client: &reqwest::Client,
        name: &str,
        topic_cache_path: Option<&Path>,
    ) -> Result<Option<topic_cache::ZhihuTopicEntry>> {
        if let Some(path) = topic_cache_path {
            match topic_cache::get_zhihu(path, name) {
                Ok(Some(cached)) => return Ok(Some(cached)),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    topic = name,
                    error = %error,
                    "failed to read zhihu topic cache"
                ),
            }
        }

        let response = client
            .get(format!("{API_V4}/search_v3"))
            .headers(self.headers()?)
            .query(&[
                ("gk_version", "gz-gaokao"),
                ("t", "topic"),
                ("q", name),
                ("correction", "1"),
                ("offset", "0"),
                ("limit", "20"),
                ("filter_fields", "lc_idx"),
                ("lc_idx", "0"),
                ("show_all_topics", "0"),
                ("search_source", "Normal"),
            ])
            .send()
            .await
            .with_context(|| format!("知乎搜索话题失败: {name}"))?;
        let value = handle_response(response).await?;
        let topic = find_topic_candidate(&value, name);
        if let (Some(path), Some(topic)) = (topic_cache_path, topic.as_ref()) {
            if let Err(error) = topic_cache::set_zhihu(path, &topic.name, &topic.id) {
                tracing::warn!(
                    topic = name,
                    error = %error,
                    "failed to write zhihu topic cache"
                );
            }
        }
        Ok(topic)
    }

    async fn sync_article_topics(
        &self,
        client: &reqwest::Client,
        article_id: &str,
        topics: &[topic_cache::ZhihuTopicEntry],
    ) -> Result<Value> {
        let headers = self.zhuanlan_headers(article_id)?;
        let mut last = Value::Null;
        for topic in topics {
            let response = client
                .post(format!("{ZHUANLAN_API}/articles/{article_id}/topics"))
                .headers(headers.clone())
                .json(&json!({
                    "url": format!("{API_V4}/topics/{}", topic.id),
                    "type": "topic",
                    "id": topic.id,
                    "name": topic.name,
                }))
                .send()
                .await
                .with_context(|| format!("知乎添加文章话题失败: {}", topic.name))?;
            last = handle_response(response).await?;
        }
        Ok(last)
    }

    async fn create_content_draft(&self, client: &reqwest::Client) -> Result<String> {
        let response = client
            .post(format!("{API_V4}/content/drafts"))
            .headers(self.headers()?)
            .json(&json!({ "action": "article" }))
            .send()
            .await
            .context("知乎创建草稿失败")?;
        let data = handle_response(response).await?;
        data.pointer("/data/content_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("知乎创建草稿响应缺少 content_id"))
    }

    fn headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(COOKIE, HeaderValue::from_str(&self.cookie_header)?);
        headers.insert(
            "referer",
            HeaderValue::from_static("https://www.zhihu.com/"),
        );
        headers.insert("x-xsrftoken", HeaderValue::from_str(&self.xsrf)?);
        headers.insert("sec-ch-ua-mobile", HeaderValue::from_static("?0"));
        headers.insert(
            "sec-ch-ua-platform",
            HeaderValue::from_static("\"Windows\""),
        );
        Ok(headers)
    }

    fn zhuanlan_headers(&self, article_id: &str) -> Result<HeaderMap> {
        let mut headers = self.headers()?;
        headers.insert(
            "referer",
            HeaderValue::from_str(&format!("https://zhuanlan.zhihu.com/p/{article_id}/edit"))?,
        );
        headers.insert(
            "origin",
            HeaderValue::from_static("https://zhuanlan.zhihu.com"),
        );
        Ok(headers)
    }
}

struct ImageInfo {
    src: String,
    original_src: String,
    watermark: String,
    watermark_src: String,
}

fn build_img_html(images: &[ImageInfo]) -> String {
    images
        .iter()
        .map(|info| {
            format!(
                r#"<img src="{}" data-caption="" data-size="normal" data-rawwidth="0" data-rawheight="0" data-watermark="{}" data-original-src="{}" data-watermark-src="{}" data-private-watermark-src=""/>"#,
                info.src, info.watermark, info.original_src, info.watermark_src
            )
        })
        .collect()
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "</p><p>")
}

fn required_str(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("知乎响应缺少 {field}"))
}

fn image_content_type(path: &Path) -> Result<&'static str> {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    match mime.essence_str() {
        "image/jpeg" => Ok("image/jpeg"),
        "image/png" => Ok("image/png"),
        "image/webp" => Ok("image/webp"),
        other => anyhow::bail!("知乎暂不支持该图片格式: {other} ({})", path.display()),
    }
}

fn extract_article_id(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(value_to_string)
        .or_else(|| value.pointer("/publish/id").and_then(value_to_string))
        .or_else(|| {
            value
                .pointer("/publish/article/id")
                .and_then(value_to_string)
        })
        .or_else(|| value.pointer("/data/id").and_then(value_to_string))
        .or_else(|| value.pointer("/article/id").and_then(value_to_string))
}

fn value_to_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_i64().map(|id| id.to_string()))
}

fn extract_tag_names(body: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let Some(tag_line) = body.lines().rev().find(|line| !line.trim().is_empty()) else {
        return tags;
    };
    let tokens = tag_line.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() || tokens.len() == 1 && !tokens[0].trim_start().starts_with('#') {
        return tags;
    }
    for raw in tokens {
        let tag = raw
            .trim()
            .trim_matches(|ch: char| {
                matches!(
                    ch,
                    ',' | '，' | '.' | '。' | ';' | '；' | ':' | '：' | '!' | '！'
                )
            })
            .trim_start_matches('#')
            .trim();
        if tag.is_empty() {
            continue;
        }
        if !tags.iter().any(|existing| existing == tag) {
            tags.push(tag.to_string());
        }
    }
    tags
}

fn find_topic_candidate(value: &Value, name: &str) -> Option<topic_cache::ZhihuTopicEntry> {
    let mut candidates = Vec::new();
    collect_topic_candidates(value, &mut candidates);
    candidates
        .iter()
        .find(|topic| {
            topic
                .get("name")
                .and_then(Value::as_str)
                .map(|topic_name| normalize_topic_name(topic_name) == name)
                .unwrap_or(false)
        })
        .and_then(|topic| {
            let id = topic.get("id").and_then(value_to_string)?;
            let name = topic
                .get("name")
                .and_then(Value::as_str)
                .map(normalize_topic_name)?;
            Some(topic_cache::ZhihuTopicEntry { id, name })
        })
}

fn normalize_topic_name(name: &str) -> String {
    let mut normalized = String::new();
    let mut in_tag = false;
    for ch in name.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => normalized.push(ch),
            _ => {}
        }
    }
    normalized
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_string()
}

fn collect_topic_candidates(value: &Value, candidates: &mut Vec<Value>) {
    match value {
        Value::Object(map) => {
            let type_is_topic = map.get("type").and_then(Value::as_str) == Some("topic");
            let has_topic_shape = map.get("id").is_some() && map.get("name").is_some();
            if type_is_topic && has_topic_shape {
                candidates.push(value.clone());
            }
            for child in map.values() {
                collect_topic_candidates(child, candidates);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_topic_candidates(child, candidates);
            }
        }
        _ => {}
    }
}

async fn handle_response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let text = response.text().await?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        anyhow::bail!("知乎登录态失效或无权限: HTTP {status}");
    }
    if !status.is_success() {
        anyhow::bail!(
            "知乎 API HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        );
    }
    if text.is_empty() {
        return Ok(Value::Null);
    }
    let value: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "知乎返回非 JSON: {}",
            text.chars().take(200).collect::<String>()
        )
    })?;
    let code = value.get("code").and_then(Value::as_i64);
    if code.is_some() && code != Some(0) {
        let message = value
            .get("message")
            .or_else(|| value.get("toast_message"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        anyhow::bail!("知乎 API 错误: {message}; 响应: {}", compact_json(&value));
    }
    if let Some(result) = value.pointer("/data/result").and_then(Value::as_str) {
        if let Ok(parsed) = serde_json::from_str(result) {
            return Ok(parsed);
        }
    }
    Ok(value)
}

fn compact_json(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    if text.chars().count() > 300 {
        format!("{}...", text.chars().take(300).collect::<String>())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zhihu_extracts_topic_names_from_last_tag_line() {
        let body = "正文内容\n\n#投资理财 #美股 富途";
        assert_eq!(extract_tag_names(body), vec!["投资理财", "美股", "富途"]);
    }

    #[test]
    fn zhihu_collects_topic_candidates() {
        let value = json!({
            "data": [
                { "object": { "type": "topic", "id": "123", "name": "<em>美股</em>" } }
            ]
        });
        let topic = find_topic_candidate(&value, "美股").unwrap();
        assert_eq!(topic.id, "123");
        assert_eq!(topic.name, "美股");
    }

    #[test]
    fn zhihu_extracts_article_id_from_publish_response() {
        assert_eq!(
            extract_article_id(&json!({ "id": "2048", "type": "article" })).unwrap(),
            "2048"
        );
        assert_eq!(
            extract_article_id(&json!({ "data": { "id": 2049 } })).unwrap(),
            "2049"
        );
        assert_eq!(
            extract_article_id(&json!({ "publish": { "id": "2050" } })).unwrap(),
            "2050"
        );
    }
}
