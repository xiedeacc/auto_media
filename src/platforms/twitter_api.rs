use super::backend::{CookieStore, PublishBackend, PublishContent};
use crate::browser::cdp::BrowserCookie;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, COOKIE},
    multipart,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const BEARER: &str = "Bearer AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs=1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const CREATE_TWEET_QID: &str = "7TKRKCPuAGsmYde0CudbVg";
const CREATE_TWEET_OP: &str = "CreateTweet";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

/// HTTP API backend for Twitter/X via the GraphQL CreateTweet endpoint.
pub struct TwitterApi {
    cookies: Arc<CookieStore>,
}

impl TwitterApi {
    pub fn new(cookies: Arc<CookieStore>) -> Self {
        Self { cookies }
    }
}

#[async_trait]
impl PublishBackend for TwitterApi {
    async fn publish(&self, content: PublishContent<'_>) -> Result<String> {
        let cookies = self.cookies.load_or_capture().await?;
        let message =
            publish_tweet(&cookies, content.title, content.body, content.image_paths).await?;
        tracing::warn!(
            image_count = content.image_paths.len(),
            message = %message,
            "twitter api article submitted"
        );
        Ok(message)
    }
}

pub async fn publish_tweet(
    cookies: &[BrowserCookie],
    title: &str,
    body: &str,
    image_paths: &[PathBuf],
) -> Result<String> {
    let session = TwitterSession::new(cookies)?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    let mut media_ids = Vec::new();
    for path in image_paths.iter().take(4) {
        media_ids.push(session.upload_media(&client, path).await?);
    }
    let body = normalize_hashtag_line(body);
    let text = [title.trim(), body.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        anyhow::bail!("Twitter/X 发文内容不能为空");
    }
    let response = session.create_tweet(&client, &text, &media_ids).await?;
    Ok(format!(
        "Twitter/X API 已提交发布：{}",
        compact_json(&response)
    ))
}

fn normalize_hashtag_line(body: &str) -> String {
    let mut lines = body.lines().map(ToString::to_string).collect::<Vec<_>>();
    let Some(last_index) = lines.iter().rposition(|line| !line.trim().is_empty()) else {
        return body.trim().to_string();
    };
    let tokens = lines[last_index].split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return body.trim().to_string();
    }
    let looks_like_tags = tokens.len() > 1 || tokens[0].trim_start().starts_with('#');
    if !looks_like_tags {
        return body.trim().to_string();
    }
    lines[last_index] = tokens
        .into_iter()
        .map(|token| {
            let normalized = token
                .trim()
                .trim_matches(|ch: char| {
                    matches!(
                        ch,
                        ',' | '，' | '.' | '。' | ';' | '；' | ':' | '：' | '!' | '！'
                    )
                })
                .trim_start_matches('#');
            if normalized.is_empty() {
                String::new()
            } else {
                format!("#{normalized}")
            }
        })
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    lines.join("\n").trim().to_string()
}

struct TwitterSession {
    cookie_header: String,
    csrf: String,
}

impl TwitterSession {
    fn new(cookies: &[BrowserCookie]) -> Result<Self> {
        let filtered = cookies
            .iter()
            .filter(|cookie| {
                cookie.domain.contains("twitter.com") || cookie.domain.contains("x.com")
            })
            .collect::<Vec<_>>();
        let find = |name: &str| {
            filtered
                .iter()
                .find(|cookie| cookie.name == name)
                .map(|cookie| cookie.value.clone())
        };
        let csrf = find("ct0").ok_or_else(|| anyhow!("Twitter/X 缺少 ct0 cookie"))?;
        if find("auth_token").is_none() {
            anyhow::bail!("Twitter/X 缺少 auth_token cookie");
        }
        let cookie_header = filtered
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");
        Ok(Self {
            cookie_header,
            csrf,
        })
    }

    async fn upload_media(&self, client: &reqwest::Client, path: &Path) -> Result<String> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read media {}", path.display()))?;
        let media_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_string();
        let category = if media_type.contains("gif") {
            "tweet_gif"
        } else if media_type.starts_with("video/") {
            "tweet_video"
        } else {
            "tweet_image"
        };

        let init = client
            .post("https://upload.twitter.com/i/media/upload.json")
            .headers(self.headers()?)
            .query(&[
                ("command", "INIT"),
                ("media_type", media_type.as_str()),
                ("total_bytes", &bytes.len().to_string()),
                ("media_category", category),
            ])
            .send()
            .await
            .context("Twitter/X 初始化媒体上传失败")?;
        let init = handle_response(init).await?;
        let media_id = init
            .get("media_id_string")
            .or_else(|| init.get("media_id"))
            .and_then(|value| {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| value.as_i64().map(|id| id.to_string()))
            })
            .ok_or_else(|| anyhow!("Twitter/X 媒体上传 INIT 响应缺少 media_id"))?;

        let form = multipart::Form::new().part("media", multipart::Part::bytes(bytes));
        let append = client
            .post("https://upload.twitter.com/i/media/upload.json")
            .headers(self.headers()?)
            .query(&[
                ("command", "APPEND"),
                ("media_id", media_id.as_str()),
                ("segment_index", "0"),
            ])
            .multipart(form)
            .send()
            .await
            .context("Twitter/X 上传媒体分片失败")?;
        if !append.status().is_success() {
            anyhow::bail!(
                "Twitter/X 上传媒体分片失败 HTTP {}: {}",
                append.status(),
                append.text().await.unwrap_or_default()
            );
        }

        let finalize = client
            .post("https://upload.twitter.com/i/media/upload.json")
            .headers(self.headers()?)
            .query(&[
                ("command", "FINALIZE"),
                ("media_id", media_id.as_str()),
                ("allow_async", "true"),
            ])
            .send()
            .await
            .context("Twitter/X 完成媒体上传失败")?;
        let finalized = handle_response(finalize).await?;
        self.wait_media_processing(client, &media_id, finalized.get("processing_info"))
            .await?;
        Ok(media_id)
    }

    async fn wait_media_processing(
        &self,
        client: &reqwest::Client,
        media_id: &str,
        processing: Option<&Value>,
    ) -> Result<()> {
        let mut processing = processing.cloned();
        while let Some(info) = processing {
            let state = info.get("state").and_then(Value::as_str).unwrap_or("");
            if state == "succeeded" {
                return Ok(());
            }
            if state == "failed" {
                anyhow::bail!("Twitter/X 媒体处理失败: {}", compact_json(&info));
            }
            let wait_secs = info
                .get("check_after_secs")
                .and_then(Value::as_u64)
                .unwrap_or(2);
            tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
            let status = client
                .get("https://upload.twitter.com/i/media/upload.json")
                .headers(self.headers()?)
                .query(&[("command", "STATUS"), ("media_id", media_id)])
                .send()
                .await
                .context("Twitter/X 查询媒体处理状态失败")?;
            let status = handle_response(status).await?;
            processing = status.get("processing_info").cloned();
        }
        Ok(())
    }

    async fn create_tweet(
        &self,
        client: &reqwest::Client,
        text: &str,
        media_ids: &[String],
    ) -> Result<Value> {
        let media_entities = media_ids
            .iter()
            .map(|id| json!({ "media_id": id, "tagged_users": [] }))
            .collect::<Vec<_>>();
        let variables = json!({
            "tweet_text": text,
            "dark_request": false,
            "media": {
                "media_entities": media_entities,
                "possibly_sensitive": false
            },
            "semantic_annotation_ids": []
        });
        let payload = json!({
            "variables": variables,
            "features": default_features()
        });
        let response = client
            .post(format!(
                "https://twitter.com/i/api/graphql/{CREATE_TWEET_QID}/{CREATE_TWEET_OP}"
            ))
            .headers(self.headers()?)
            .json(&payload)
            .send()
            .await
            .context("Twitter/X 创建推文失败")?;
        handle_response(response).await
    }

    fn headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static(BEARER));
        headers.insert(COOKIE, HeaderValue::from_str(&self.cookie_header)?);
        headers.insert("referer", HeaderValue::from_static("https://twitter.com/"));
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        headers.insert("x-csrf-token", HeaderValue::from_str(&self.csrf)?);
        headers.insert(
            "x-twitter-auth-type",
            HeaderValue::from_static("OAuth2Session"),
        );
        headers.insert("x-twitter-active-user", HeaderValue::from_static("yes"));
        headers.insert("x-twitter-client-language", HeaderValue::from_static("en"));
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        Ok(headers)
    }
}

fn default_features() -> Value {
    json!({
        "c9s_tweet_anatomy_moderator_badge_enabled": true,
        "responsive_web_home_pinned_timelines_enabled": true,
        "blue_business_profile_image_shape_enabled": true,
        "creator_subscriptions_tweet_preview_api_enabled": true,
        "freedom_of_speech_not_reach_fetch_enabled": true,
        "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
        "graphql_timeline_v2_bookmark_timeline": true,
        "hidden_profile_likes_enabled": true,
        "highlights_tweets_tab_ui_enabled": true,
        "interactive_text_enabled": true,
        "longform_notetweets_consumption_enabled": true,
        "longform_notetweets_inline_media_enabled": true,
        "longform_notetweets_rich_text_read_enabled": true,
        "longform_notetweets_richtext_consumption_enabled": true,
        "profile_foundations_tweet_stats_enabled": true,
        "profile_foundations_tweet_stats_tweet_frequency": true,
        "responsive_web_birdwatch_note_limit_enabled": true,
        "responsive_web_edit_tweet_api_enabled": true,
        "responsive_web_enhance_cards_enabled": false,
        "responsive_web_graphql_exclude_directive_enabled": true,
        "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
        "responsive_web_graphql_timeline_navigation_enabled": true,
        "responsive_web_media_download_video_enabled": false,
        "responsive_web_text_conversations_enabled": false,
        "responsive_web_twitter_article_data_v2_enabled": true,
        "responsive_web_twitter_article_tweet_consumption_enabled": false,
        "responsive_web_twitter_blue_verified_badge_is_enabled": true,
        "rweb_lists_timeline_redesign_enabled": true,
        "spaces_2022_h2_clipping": true,
        "spaces_2022_h2_spaces_communities": true,
        "standardized_nudges_misinfo": true,
        "subscriptions_verification_info_verified_since_enabled": true,
        "tweet_awards_web_tipping_enabled": false,
        "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
        "tweetypie_unmention_optimization_enabled": true,
        "verified_phone_label_enabled": false,
        "vibe_api_enabled": true,
        "view_counts_everywhere_api_enabled": true
    })
}

async fn handle_response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        anyhow::bail!(
            "Twitter/X API HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        );
    }
    if text.is_empty() {
        return Ok(Value::Null);
    }
    let value: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "Twitter/X 返回非 JSON: {}",
            text.chars().take(200).collect::<String>()
        )
    })?;
    if let Some(errors) = value.get("errors").and_then(Value::as_array) {
        if !errors.is_empty() {
            anyhow::bail!("Twitter/X API 错误: {}", compact_json(&value));
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
    fn twitter_normalizes_last_tag_line() {
        let body = "正文\n\n#投资理财 富途 盈透";
        assert_eq!(
            normalize_hashtag_line(body),
            "正文\n\n#投资理财 #富途 #盈透"
        );
    }
}
