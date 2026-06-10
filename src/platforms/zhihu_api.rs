use crate::browser::cdp::BrowserCookie;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE};
use serde_json::{json, Value};
use sha1::Sha1;
use std::path::{Path, PathBuf};

const API_V4: &str = "https://www.zhihu.com/api/v4";
const IMAGE_API: &str = "https://api.zhihu.com/images";
const OSS_UPLOAD_URL: &str = "https://zhihu-pics-upload.zhimg.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

type HmacSha1 = Hmac<Sha1>;

pub async fn publish_image_article(
    cookies: &[BrowserCookie],
    title: &str,
    body: &str,
    image_paths: &[PathBuf],
) -> Result<String> {
    if body.chars().count() < 9 {
        anyhow::bail!("知乎正文至少需要 9 个字");
    }
    let session = ZhihuSession::new(cookies)?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    let mut image_infos = Vec::new();
    for path in image_paths {
        image_infos.push(session.upload_image(&client, path).await?);
    }
    let response = session
        .create_article(&client, title, body, &image_infos)
        .await?;
    Ok(format!("知乎 API 已提交发布：{}", compact_json(&response)))
}

struct ZhihuSession {
    cookie_header: String,
    xsrf: String,
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
        let image_hash = format!("{:x}", md5::compute(&bytes));
        let register = client
            .post(IMAGE_API)
            .headers(self.headers()?)
            .json(&json!({ "image_hash": image_hash, "source": "article" }))
            .send()
            .await
            .context("知乎图片注册失败")?;
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
        if state == 2 {
            let object_key = upload_file
                .get("object_key")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("知乎图片注册响应缺少 object_key"))?;
            let token = data
                .get("upload_token")
                .ok_or_else(|| anyhow!("知乎图片注册响应缺少 upload_token"))?;
            self.upload_to_oss(client, object_key, &bytes, token)
                .await?;
        } else if state != 1 {
            anyhow::bail!("知乎图片注册返回未知 state={state}");
        }
        self.poll_image(client, &image_id).await
    }

    async fn upload_to_oss(
        &self,
        client: &reqwest::Client,
        object_key: &str,
        bytes: &[u8],
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
        let content_type = "image/jpeg";
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

    async fn poll_image(&self, client: &reqwest::Client, image_id: &str) -> Result<ImageInfo> {
        for _ in 0..15 {
            let response = client
                .get(format!("{IMAGE_API}/{image_id}"))
                .headers(self.headers()?)
                .send()
                .await
                .context("知乎图片处理状态查询失败")?;
            let data = handle_response(response).await?;
            if data.get("status").and_then(Value::as_str) == Some("success") {
                return Ok(ImageInfo {
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
                });
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        anyhow::bail!("知乎图片处理超时")
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
        anyhow::bail!("知乎 API 错误: {message}");
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
