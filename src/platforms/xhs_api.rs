use super::backend::{CookieStore, PublishBackend, PublishContent};
use crate::{browser::cdp::BrowserCookie, topic_cache};
use aes::Aes128;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cbc::{
    cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit},
    Encryptor,
};
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const CREATOR_HOST: &str = "https://creator.xiaohongshu.com";
const EDITH_HOST: &str = "https://edith.xiaohongshu.com";
const UPLOAD_HOST: &str = "https://ros-upload.xiaohongshu.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const AES_KEY: &[u8; 16] = b"7cc4adla5ay0701v";
const AES_IV: &[u8; 16] = b"4uzjr7mbsibcaldp";
const STANDARD_BASE64_ALPHABET: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const CUSTOM_BASE64_ALPHABET: &str =
    "ZmserbBoHQtNP+wOcza/LpngG8yJq42KWYj0DSfdikx3VT16IlUAFM97hECvuRX5";
const X3_BASE64_ALPHABET: &str = "MfgqrsbcyzPQRStuvC7mn501HIJBo2DEFTKdeNOwxWXYZap89+/A4UVLhijkl63G";
const MAIN_HEX_KEY: &str = "71a302257793271ddd273bcee3e4b98d9d7935e1da33f5765e2ea8afb6dc77a51a499d23b67c20660025860cbf13d4540d92497f58686c574e508f46e1956344f39139bf4faf22a3eef120b79258145b2feb5193b6478669961298e79bedca646e1a693a926154a5a7a1bd1cf0dedb742f917a747a1e388b234f2277516db7116035439730fa61e9822a0eca7bff72d8";
const MAIN_VERSION_BYTES: [u8; 4] = [121, 104, 96, 41];
const MAIN_PAYLOAD_LENGTH: usize = 144;
const ENV_TABLE: [u8; 15] = [
    115, 248, 83, 102, 103, 201, 181, 131, 99, 94, 4, 68, 250, 132, 21,
];
const ENV_CHECKS_DEFAULT: [u8; 15] = [0, 1, 18, 1, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0];
const A3_PREFIX: [u8; 4] = [2, 97, 51, 16];
const HASH_IV: [u32; 4] = [1831565813, 461845907, 2246822507, 3266489909];
const MAIN_SDK_VERSION: &str = "4.2.6";
const MAIN_APP_ID: &str = "xhs-pc-web";
const MAIN_PLATFORM: &str = "Windows";
const B1_SECRET_KEY: &str = "xhswebmplfbt";
const HEX_CHARS: &[u8; 16] = b"abcdef0123456789";

/// HTTP API backend for Xiaohongshu. Experimental fallback behind the CDP
/// backend: relies on reverse-engineered `x-s` signing that may break on changes.
pub struct XhsApi {
    cookies: Arc<CookieStore>,
    topic_cache: PathBuf,
}

impl XhsApi {
    pub fn new(cookies: Arc<CookieStore>, topic_cache: PathBuf) -> Self {
        Self {
            cookies,
            topic_cache,
        }
    }
}

#[async_trait]
impl PublishBackend for XhsApi {
    async fn publish(&self, content: PublishContent<'_>) -> Result<String> {
        let cookies = self.cookies.load_or_capture().await?;
        let message = publish_image_note(
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
            "xhs api article submitted"
        );
        Ok(message)
    }
}

pub async fn publish_image_note(
    cookies: &[BrowserCookie],
    title: &str,
    desc: &str,
    image_paths: &[PathBuf],
    topic_cache_path: Option<&Path>,
) -> Result<String> {
    if image_paths.is_empty() {
        anyhow::bail!("小红书 API 发布需要至少一张图片");
    }

    let session = XhsSession::new(cookies)?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    let mut file_ids = Vec::new();
    for path in image_paths {
        let (file_id, token) = session.get_upload_permit(&client).await?;
        session.upload_file(&client, &file_id, &token, path).await?;
        file_ids.push(file_id);
    }

    let response = session
        .create_image_note(&client, title, desc, &file_ids, topic_cache_path)
        .await?;
    Ok(format!(
        "小红书 API 已提交发布：{}",
        compact_json(&response)
    ))
}

struct XhsSession {
    cookie_header: String,
    a1: String,
}

impl XhsSession {
    fn new(cookies: &[BrowserCookie]) -> Result<Self> {
        let filtered = cookies
            .iter()
            .filter(|cookie| cookie.domain.contains("xiaohongshu.com"))
            .collect::<Vec<_>>();
        let a1 = filtered
            .iter()
            .find(|cookie| cookie.name == "a1")
            .map(|cookie| cookie.value.clone())
            .ok_or_else(|| anyhow!("小红书缺少 a1 cookie，无法生成 API 签名"))?;
        let cookie_header = filtered
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");
        Ok(Self { cookie_header, a1 })
    }

    async fn get_upload_permit(&self, client: &reqwest::Client) -> Result<(String, String)> {
        let uri = "/api/media/v1/upload/web/permit";
        let query = "biz_name=spectrum&scene=image&file_count=1&version=1&source=web";
        let data = self
            .creator_get(client, uri, Some(query))
            .await
            .context("获取小红书上传许可失败")?;
        let permit = data
            .pointer("/uploadTempPermits/0")
            .ok_or_else(|| anyhow!("小红书上传许可响应缺少 uploadTempPermits"))?;
        let file_id = permit
            .pointer("/fileIds/0")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("小红书上传许可响应缺少 fileId"))?
            .to_string();
        let token = permit
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("小红书上传许可响应缺少 token"))?
            .to_string();
        Ok((file_id, token))
    }

    async fn upload_file(
        &self,
        client: &reqwest::Client,
        file_id: &str,
        token: &str,
        path: &Path,
    ) -> Result<()> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read image {}", path.display()))?;
        let content_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_string();
        let response = client
            .put(format!("{UPLOAD_HOST}/{file_id}"))
            .header("X-Cos-Security-Token", token)
            .header(CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await
            .context("上传图片到小红书失败")?;
        if !response.status().is_success() {
            anyhow::bail!("上传图片到小红书失败: {}", response.status());
        }
        Ok(())
    }

    async fn create_image_note(
        &self,
        client: &reqwest::Client,
        title: &str,
        desc: &str,
        file_ids: &[String],
        topic_cache_path: Option<&Path>,
    ) -> Result<Value> {
        let images = file_ids
            .iter()
            .map(|file_id| json!({ "file_id": file_id, "metadata": { "source": -1 } }))
            .collect::<Vec<_>>();
        let business_binds = json!({
            "version": 1,
            "noteId": 0,
            "noteOrderBind": {},
            "notePostTiming": { "postTime": Value::Null },
            "noteCollectionBind": { "id": "" }
        });
        let topics = extract_topic_names(desc);
        let mut hash_tags = Vec::new();
        for topic in &topics {
            if let Some(tag) = self
                .find_topic(client, topic, title, desc, topic_cache_path)
                .await?
            {
                hash_tags.push(tag);
            }
        }
        let desc = render_xhs_desc(desc, &topics);
        let data = json!({
            "common": {
                "type": "normal",
                "title": title,
                "note_id": "",
                "desc": desc,
                "source": "{\"type\":\"web\",\"ids\":\"\",\"extraInfo\":\"{\\\"subType\\\":\\\"official\\\"}\"}",
                "business_binds": serde_json::to_string(&business_binds)?,
                "ats": [],
                "hash_tag": hash_tags,
                "post_loc": {},
                "privacy_info": { "op_type": 1, "type": 0 }
            },
            "image_info": { "images": images },
            "video_info": Value::Null
        });
        self.main_api_post(client, "/web_api/sns/v2/note", data)
            .await
            .context("调用小红书发布接口失败")
    }

    async fn search_topic(
        &self,
        client: &reqwest::Client,
        topic: &str,
        title: &str,
        desc: &str,
    ) -> Result<Option<Value>> {
        let data = json!({
            "keyword": topic,
            "suggest_topic_request": {
                "title": title,
                "desc": desc
            },
            "page": {
                "page_size": 20,
                "page": 1
            }
        });
        let response = self
            .main_api_post(client, "/web_api/sns/v1/search/topic", data)
            .await
            .with_context(|| format!("搜索小红书话题失败: {topic}"))?;
        Ok(find_topic_candidate(&response, topic))
    }

    async fn find_topic(
        &self,
        client: &reqwest::Client,
        topic: &str,
        title: &str,
        desc: &str,
        topic_cache_path: Option<&Path>,
    ) -> Result<Option<Value>> {
        if let Some(path) = topic_cache_path {
            match topic_cache::get_xhs(path, topic) {
                Ok(Some(cached)) => return Ok(Some(cached)),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    topic = topic,
                    error = %error,
                    "failed to read xhs topic cache"
                ),
            }
        }

        let found = self.search_topic(client, topic, title, desc).await?;
        if let (Some(path), Some(value)) = (topic_cache_path, found.as_ref()) {
            if let Err(error) = topic_cache::set_xhs(path, topic, value) {
                tracing::warn!(
                    topic = topic,
                    error = %error,
                    "failed to write xhs topic cache"
                );
            }
        }
        Ok(found)
    }

    async fn creator_get(
        &self,
        client: &reqwest::Client,
        uri: &str,
        query: Option<&str>,
    ) -> Result<Value> {
        let api = match query {
            Some(query) => format!("url={uri}?{query}"),
            None => format!("url={uri}"),
        };
        let (xs, xt) = sign_creator(&api, None, &self.a1)?;
        let full_uri = match query {
            Some(query) => format!("{uri}?{query}"),
            None => uri.to_string(),
        };
        let response = client
            .get(format!("{}{full_uri}", host_for_uri(uri)))
            .headers(self.headers(&xs, &xt)?)
            .send()
            .await?;
        handle_response(response).await
    }

    async fn main_api_post(
        &self,
        client: &reqwest::Client,
        uri: &str,
        data: Value,
    ) -> Result<Value> {
        let body = serde_json::to_string(&data)?;
        let signed = sign_main_api_post(uri, &body, &self.a1)?;
        let response = client
            .post(format!("{EDITH_HOST}{uri}"))
            .headers(self.main_headers(&signed)?)
            .body(body)
            .send()
            .await?;
        handle_response(response).await
    }

    fn headers(&self, xs: &str, xt: &str) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        headers.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.8"),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json;charset=UTF-8"),
        );
        headers.insert(COOKIE, HeaderValue::from_str(&self.cookie_header)?);
        headers.insert("origin", HeaderValue::from_static(CREATOR_HOST));
        headers.insert(
            "referer",
            HeaderValue::from_static("https://creator.xiaohongshu.com/"),
        );
        headers.insert("x-s", HeaderValue::from_str(xs)?);
        headers.insert("x-t", HeaderValue::from_str(xt)?);
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-site"));
        headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
        headers.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
        Ok(headers)
    }

    fn main_headers(&self, signed: &MainSignHeaders) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        headers.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.8"),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json;charset=UTF-8"),
        );
        headers.insert(COOKIE, HeaderValue::from_str(&self.cookie_header)?);
        headers.insert("origin", HeaderValue::from_static(CREATOR_HOST));
        headers.insert(
            "referer",
            HeaderValue::from_static(
                "https://creator.xiaohongshu.com/publish/publish?from=menu&target=image",
            ),
        );
        headers.insert("x-s", HeaderValue::from_str(&signed.xs)?);
        headers.insert("x-s-common", HeaderValue::from_str(&signed.xs_common)?);
        headers.insert("x-t", HeaderValue::from_str(&signed.xt)?);
        headers.insert("x-b3-traceid", HeaderValue::from_str(&signed.b3_traceid)?);
        headers.insert(
            "x-xray-traceid",
            HeaderValue::from_str(&signed.xray_traceid)?,
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-site"));
        headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
        headers.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
        Ok(headers)
    }
}

fn host_for_uri(uri: &str) -> &'static str {
    if uri.starts_with("/api/galaxy/") {
        CREATOR_HOST
    } else {
        EDITH_HOST
    }
}

fn extract_topic_names(desc: &str) -> Vec<String> {
    let mut topics = Vec::new();
    let Some(tag_line) = desc.lines().rev().find(|line| !line.trim().is_empty()) else {
        return topics;
    };
    for raw in tag_line.split_whitespace() {
        let topic = raw
            .trim()
            .trim_matches(|ch: char| {
                matches!(
                    ch,
                    ',' | '，' | '.' | '。' | ';' | '；' | ':' | '：' | '!' | '！'
                )
            })
            .trim_start_matches('#')
            .trim();
        if topic.is_empty() {
            continue;
        }
        let looks_like_tag = raw.trim_start().starts_with('#')
            || tag_line.split_whitespace().count() > 1 && tag_line.chars().count() <= 200;
        if !looks_like_tag {
            continue;
        }
        if !topics.iter().any(|existing| existing == topic) {
            topics.push(topic.to_string());
        }
    }
    topics
}

fn render_xhs_desc(desc: &str, topics: &[String]) -> String {
    if topics.is_empty() {
        return desc.to_string();
    }
    let topic_line = topics
        .iter()
        .map(|topic| format!("\u{feff}#{topic}[话题]#\u{feff}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut lines = desc.lines().collect::<Vec<_>>();
    if lines
        .last()
        .map(|line| {
            let tokens = line.split_whitespace().collect::<Vec<_>>();
            !tokens.is_empty()
                && tokens.iter().all(|token| {
                    let normalized = token.trim_start_matches('#');
                    topics.iter().any(|topic| topic == normalized)
                })
        })
        .unwrap_or(false)
    {
        lines.pop();
    }
    let body = lines.join("\n").trim().to_string();
    if body.is_empty() {
        topic_line
    } else {
        format!("{body}\n\n{topic_line}")
    }
}

fn find_topic_candidate(value: &Value, topic: &str) -> Option<Value> {
    let mut candidates = Vec::new();
    collect_topic_candidates(value, &mut candidates);
    candidates
        .iter()
        .find(|candidate| {
            candidate
                .get("name")
                .and_then(Value::as_str)
                .map(|name| normalize_topic_name(name) == topic)
                .unwrap_or(false)
        })
        .or_else(|| candidates.first())
        .cloned()
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
            let has_required = map.get("id").and_then(Value::as_str).is_some()
                && map.get("name").and_then(Value::as_str).is_some();
            if has_required {
                let mut topic = value.clone();
                if topic.get("type").is_none() {
                    topic["type"] = Value::String("topic".to_string());
                }
                candidates.push(topic);
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

fn sign_creator(api: &str, data_json: Option<&str>, a1: &str) -> Result<(String, String)> {
    let mut content = api.to_string();
    if let Some(data_json) = data_json {
        content.push_str(data_json);
    }
    let x1 = format!("{:x}", md5::compute(content.as_bytes()));
    let x2 = "0|0|0|1|0|0|1|0|0|0|1|0|0|0|0|1|0|0|0";
    let x4 = chrono::Utc::now().timestamp_millis();
    let plaintext = format!("x1={x1};x2={x2};x3={a1};x4={x4};");
    let payload_base64 = STANDARD.encode(plaintext.as_bytes());
    let payload = Encryptor::<Aes128>::new(AES_KEY.into(), AES_IV.into())
        .encrypt_padded_vec_mut::<Pkcs7>(payload_base64.as_bytes());
    let envelope = json!({
        "signSvn": "56",
        "signType": "x2",
        "appId": "ugc",
        "signVersion": "1",
        "payload": hex::encode(payload)
    });
    let xs = format!(
        "XYW_{}",
        STANDARD.encode(serde_json::to_string(&envelope)?.as_bytes())
    );
    Ok((xs, x4.to_string()))
}

struct MainSignHeaders {
    xs: String,
    xs_common: String,
    xt: String,
    b3_traceid: String,
    xray_traceid: String,
}

fn sign_main_api_post(uri: &str, body: &str, a1: &str) -> Result<MainSignHeaders> {
    let ts_ms = chrono::Utc::now().timestamp_millis();
    let content = format!("{uri}{body}");
    let d_value = format!("{:x}", md5::compute(content.as_bytes()));
    let payload = build_main_payload(&d_value, a1, &content, ts_ms)?;
    let xored = xor_transform(&payload)?;
    let x3 = x3_base64_encode(&xored[..MAIN_PAYLOAD_LENGTH]);
    let signature_json = format!(
        r#"{{"x0":"{MAIN_SDK_VERSION}","x1":"{MAIN_APP_ID}","x2":"{MAIN_PLATFORM}","x3":"mns0301_{x3}","x4":""}}"#
    );
    let xs = format!("XYS_{}", custom_base64_encode(signature_json.as_bytes()));
    let b1 = generate_b1()?;
    let x9 = crc32_js_int(&b1);
    let a1_json = serde_json::to_string(a1)?;
    let b1_json = serde_json::to_string(&b1)?;
    let xs_common_json = format!(
        r#"{{"s0":5,"s1":"","x0":"1","x1":"{MAIN_SDK_VERSION}","x2":"{MAIN_PLATFORM}","x3":"{MAIN_APP_ID}","x4":"4.86.0","x5":{a1_json},"x6":"","x7":"","x8":{b1_json},"x9":{x9},"x10":0,"x11":"normal"}}"#
    );
    Ok(MainSignHeaders {
        xs,
        xs_common: custom_base64_encode(xs_common_json.as_bytes()),
        xt: ts_ms.to_string(),
        b3_traceid: random_hex(16),
        xray_traceid: generate_xray_traceid(ts_ms),
    })
}

fn build_main_payload(hex_parameter: &str, a1: &str, content: &str, ts_ms: i64) -> Result<Vec<u8>> {
    let mut rng = rand::thread_rng();
    let mut payload = Vec::with_capacity(MAIN_PAYLOAD_LENGTH);
    payload.extend_from_slice(&MAIN_VERSION_BYTES);

    let seed: u32 = rng.gen();
    let seed_bytes = seed.to_le_bytes();
    let seed_byte0 = seed_bytes[0];
    payload.extend_from_slice(&seed_bytes);

    let ts_bytes = (ts_ms as u64).to_le_bytes();
    payload.extend_from_slice(&ts_bytes);

    let page_load_offset_ms: i64 = rng.gen_range(10_000..=50_000);
    payload.extend_from_slice(&((ts_ms - page_load_offset_ms) as u64).to_le_bytes());
    payload.extend_from_slice(&(rng.gen_range(15_u32..=50_u32)).to_le_bytes());
    payload.extend_from_slice(&(rng.gen_range(1000_u32..=1200_u32)).to_le_bytes());
    payload.extend_from_slice(&(content.as_bytes().len() as u32).to_le_bytes());

    let md5_bytes = hex_to_bytes(hex_parameter)?;
    for byte in md5_bytes.iter().take(8) {
        payload.push(byte ^ seed_byte0);
    }

    payload.push(52);
    let a1_bytes = a1.as_bytes();
    for i in 0..52 {
        payload.push(*a1_bytes.get(i).unwrap_or(&0));
    }

    payload.push(10);
    let app_bytes = MAIN_APP_ID.as_bytes();
    for i in 0..10 {
        payload.push(*app_bytes.get(i).unwrap_or(&0));
    }

    payload.push(1);
    payload.push(seed_byte0 ^ ENV_TABLE[0]);
    for i in 1..15 {
        payload.push(ENV_TABLE[i] ^ ENV_CHECKS_DEFAULT[i]);
    }

    let api_path = extract_api_path(content);
    let api_path_md5 = format!("{:x}", md5::compute(api_path.as_bytes()));
    let mut hash_input = ts_bytes.to_vec();
    hash_input.extend_from_slice(&hex_to_bytes(&api_path_md5)?);
    let hash_output = custom_hash_v2(&hash_input);
    payload.extend_from_slice(&A3_PREFIX);
    for byte in hash_output {
        payload.push(byte ^ seed_byte0);
    }

    if payload.len() != MAIN_PAYLOAD_LENGTH {
        anyhow::bail!("小红书主站签名 payload 长度异常: {}", payload.len());
    }
    Ok(payload)
}

fn xor_transform(source: &[u8]) -> Result<Vec<u8>> {
    let key = hex_to_bytes(MAIN_HEX_KEY)?;
    Ok(source
        .iter()
        .enumerate()
        .map(|(idx, byte)| byte ^ key.get(idx).copied().unwrap_or(0))
        .collect())
}

fn custom_hash_v2(input: &[u8]) -> Vec<u8> {
    let mut s0 = HASH_IV[0] ^ input.len() as u32;
    let mut s1 = HASH_IV[1] ^ ((input.len() as u32) << 8);
    let mut s2 = HASH_IV[2] ^ ((input.len() as u32) << 16);
    let mut s3 = HASH_IV[3] ^ ((input.len() as u32) << 24);

    for chunk in input.chunks_exact(8) {
        let v0 = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let v1 = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
        s0 = (s0.wrapping_add(v0) ^ s2).rotate_left(7);
        s1 = (v0 ^ s1).wrapping_add(s3).rotate_left(11);
        s2 = (s2.wrapping_add(v1) ^ s0).rotate_left(13);
        s3 = (s3 ^ v1).wrapping_add(s1).rotate_left(17);
    }

    let t0 = s0 ^ input.len() as u32;
    let t1 = s1 ^ t0;
    let t2 = s2.wrapping_add(t1);
    let t3 = s3 ^ t2;

    let rot_t0 = t0.rotate_left(9);
    let rot_t1 = t1.rotate_left(13);
    let rot_t2 = t2.rotate_left(17);
    let rot_t3 = t3.rotate_left(19);

    s0 = rot_t0.wrapping_add(rot_t2);
    s1 = rot_t1 ^ rot_t3;
    s2 = rot_t2.wrapping_add(s0);
    s3 = rot_t3 ^ s1;

    [s0, s1, s2, s3]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

fn generate_b1() -> Result<String> {
    let mut rng = rand::thread_rng();
    let b1_json = format!(
        r#"{{"x33":"0","x34":"0","x35":"0","x36":"{}","x37":"0|0|0|0|0|0|0|0|0|1|0|0|0|0|0|0|0|0|1|0|0|0|0|0","x38":"0|0|1|0|1|0|0|0|0|0|1|0|1|0|1|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0","x39":0,"x42":"3.4.4","x43":"742cc32c","x44":"{}","x45":"__SEC_CAV__1-1-1-1-1|__SEC_WSA__|","x46":"false","x48":"","x49":"{{list:[],type:}}","x50":"","x51":"","x52":"","x82":"_0x17a2|_0x1954"}}"#,
        rng.gen_range(1..=20),
        chrono::Utc::now().timestamp_millis()
    );
    let encrypted = rc4_encrypt(B1_SECRET_KEY.as_bytes(), b1_json.as_bytes());
    Ok(custom_base64_encode(&encode_uri_component_latin1_bytes(
        &encrypted,
    )))
}

fn rc4_encrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut state = [0_u8; 256];
    for (idx, item) in state.iter_mut().enumerate() {
        *item = idx as u8;
    }
    let mut j = 0_usize;
    for i in 0..256 {
        j = (j + state[i] as usize + key[i % key.len()] as usize) & 0xff;
        state.swap(i, j);
    }

    let mut i = 0_usize;
    let mut j2 = 0_usize;
    let mut output = Vec::with_capacity(data.len());
    for byte in data {
        i = (i + 1) & 0xff;
        j2 = (j2 + state[i] as usize) & 0xff;
        state.swap(i, j2);
        let k = state[(state[i] as usize + state[j2] as usize) & 0xff];
        output.push(byte ^ k);
    }
    output
}

fn encode_uri_component_latin1_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut encoded = String::new();
    for &byte in bytes {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric()
            || matches!(ch, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')')
        {
            encoded.push(ch);
        } else {
            let mut utf8 = [0_u8; 4];
            for byte in ch.encode_utf8(&mut utf8).as_bytes() {
                encoded.push('%');
                encoded.push_str(&format!("{byte:02X}"));
            }
        }
    }

    let mut result = Vec::new();
    for part in encoded.split('%').skip(1) {
        if part.len() >= 2 {
            if let Ok(byte) = u8::from_str_radix(&part[0..2], 16) {
                result.push(byte);
            }
            result.extend_from_slice(part[2..].as_bytes());
        }
    }
    result
}

fn crc32_js_int(data: &str) -> i32 {
    let mut table = [0_u32; 256];
    for (idx, slot) in table.iter_mut().enumerate() {
        let mut r = idx as u32;
        for _ in 0..8 {
            r = if r & 1 == 1 {
                (r >> 1) ^ 0xedb88320
            } else {
                r >> 1
            };
        }
        *slot = r;
    }

    let mut c = 0xffff_ffff_u32;
    for byte in data.bytes() {
        c = table[((c & 0xff) as u8 ^ byte) as usize] ^ (c >> 8);
    }
    let unsigned = (0xffff_ffff_u32 ^ c) ^ 0xedb88320;
    unsigned as i32
}

fn custom_base64_encode(bytes: &[u8]) -> String {
    translate_base64(&STANDARD.encode(bytes), CUSTOM_BASE64_ALPHABET)
}

fn x3_base64_encode(bytes: &[u8]) -> String {
    translate_base64(&STANDARD.encode(bytes), X3_BASE64_ALPHABET)
}

fn translate_base64(input: &str, to_alphabet: &str) -> String {
    input
        .chars()
        .map(|ch| {
            STANDARD_BASE64_ALPHABET
                .find(ch)
                .and_then(|idx| to_alphabet.as_bytes().get(idx).copied())
                .map(char::from)
                .unwrap_or(ch)
        })
        .collect()
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    if hex.len() % 2 != 0 {
        anyhow::bail!("invalid hex length");
    }
    (0..hex.len())
        .step_by(2)
        .map(|idx| Ok(u8::from_str_radix(&hex[idx..idx + 2], 16)?))
        .collect()
}

fn extract_api_path(content: &str) -> &str {
    let brace = content.find('{');
    let question = content.find('?');
    match (brace, question) {
        (Some(a), Some(b)) => &content[..a.min(b)],
        (Some(a), None) => &content[..a],
        (None, Some(b)) => &content[..b],
        (None, None) => content,
    }
}

fn random_hex(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| HEX_CHARS[rng.gen_range(0..16)] as char)
        .collect()
}

fn generate_xray_traceid(ts_ms: i64) -> String {
    let mut rng = rand::thread_rng();
    let seq = rng.gen_range(0_u64..=8_388_607);
    let part1 = (((ts_ms as u64) << 23) | seq).to_string();
    format!(
        "{:0>16x}{}",
        part1.parse::<u64>().unwrap_or(0),
        random_hex(16)
    )
}

async fn handle_response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    if status.as_u16() == 461 || status.as_u16() == 471 {
        anyhow::bail!("小红书触发验证码/风控: HTTP {status}");
    }
    let text = response.text().await?;
    if text.is_empty() {
        return Ok(Value::Null);
    }
    let value: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "小红书返回非 JSON: {}",
            text.chars().take(200).collect::<String>()
        )
    })?;
    if value
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(value.get("data").cloned().unwrap_or(Value::Bool(true)));
    }
    let code = value.get("code").cloned().unwrap_or(Value::Null);
    anyhow::bail!("小红书 API 错误 code={code}: {}", compact_json(&value));
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
    fn xhs_topics_are_extracted_from_last_tag_line() {
        let desc = "正文第一行\n\n#投资理财 #美股 富途";
        assert_eq!(extract_topic_names(desc), vec!["投资理财", "美股", "富途"]);
    }

    #[test]
    fn xhs_desc_replaces_plain_tag_line_with_topic_markup() {
        let desc = "正文第一行\n\n#投资理财 #美股 富途";
        let rendered = render_xhs_desc(
            desc,
            &[
                "投资理财".to_string(),
                "美股".to_string(),
                "富途".to_string(),
            ],
        );

        assert_eq!(
            rendered,
            "正文第一行\n\n\u{feff}#投资理财[话题]#\u{feff} \u{feff}#美股[话题]#\u{feff} \u{feff}#富途[话题]#\u{feff}"
        );
    }
}
