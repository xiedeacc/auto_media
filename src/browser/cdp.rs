use super::{xhs, zhihu};
use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    process::Command,
    time::{sleep, timeout, Duration},
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

#[derive(Debug, Clone)]
pub struct BrowserLaunch {
    pub port: u16,
    pub url: String,
    pub web_socket_debugger_url: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct CdpBrowser;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BrowserCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
}

#[derive(Debug, Deserialize)]
struct VersionResponse {
    #[serde(rename = "Browser")]
    browser: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TargetInfo {
    id: String,
    #[serde(rename = "type")]
    target_type: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: Option<String>,
}

impl CdpBrowser {
    pub async fn open_visible(
        &self,
        profile_dir: &Path,
        port: u16,
        url: &str,
    ) -> Result<BrowserLaunch> {
        std::fs::create_dir_all(profile_dir)
            .with_context(|| format!("create browser profile {}", profile_dir.display()))?;

        let browser_was_ready = self.is_ready(port).await;
        if !browser_was_ready {
            let executable = find_browser_executable()
                .ok_or_else(|| anyhow!("Chrome or Edge executable was not found"))?;
            launch_browser(&executable, profile_dir, port, url).await?;
        }

        self.wait_until_ready(port).await?;
        let web_socket_debugger_url = if browser_was_ready {
            match http_client()
                .put(format!("http://127.0.0.1:{port}/json/new?{url}"))
                .send()
                .await
                .ok()
                .and_then(|response| response.error_for_status().ok())
            {
                Some(response) => match response.json::<TargetInfo>().await.ok() {
                    Some(target) => {
                        let _ = self.close_other_tabs(port, &target.id).await;
                        target.web_socket_debugger_url
                    }
                    None => None,
                },
                None => None,
            }
        } else {
            None
        };
        sleep(Duration::from_secs(2)).await;
        let web_socket_debugger_url = match web_socket_debugger_url {
            Some(url) => Some(url),
            None => self.find_page_ws_url(port, url).await.ok().flatten(),
        };

        Ok(BrowserLaunch {
            port,
            url: url.to_string(),
            web_socket_debugger_url,
        })
    }

    pub async fn fill_article_draft(
        &self,
        launch: &BrowserLaunch,
        platform: &str,
        title: &str,
        body: &str,
        image_paths: &[PathBuf],
        xhs_api_template_path: Option<&Path>,
    ) -> Result<String> {
        let web_socket_debugger_url = match &launch.web_socket_debugger_url {
            Some(url) => url.clone(),
            None => self
                .find_page_ws_url(launch.port, &launch.url)
                .await?
                .ok_or_else(|| anyhow!("CDP page websocket was not found"))?,
        };
        let mut page = CdpPage::connect(&web_socket_debugger_url).await?;
        page.call("Page.enable", json!({})).await?;
        page.call("Page.bringToFront", json!({})).await?;
        page.call(
            "Page.navigate",
            json!({
                "url": &launch.url,
            }),
        )
        .await?;
        sleep(Duration::from_secs(2)).await;
        page.wait_for_platform_ready(platform).await?;
        if platform == "xhs" {
            page.click_xhs_image_tab().await?;
            page.wait_for_image_upload_ready().await?;
        } else {
            page.call(
                "Runtime.evaluate",
                json!({
                    "expression": prepare_script(platform),
                    "awaitPromise": true,
                }),
            )
            .await?;
        }
        sleep(Duration::from_millis(800)).await;

        let mut messages = Vec::new();
        if platform == "zhihu" {
            match page.fill_text(platform, title, body).await {
                Ok(message) => messages.push(message),
                Err(error) => messages.push(format!("填标题/正文失败: {error}")),
            }
        }

        if !image_paths.is_empty() {
            match page.set_first_file_input(image_paths).await {
                Ok(()) => messages.push(format!("已提交 {} 张图片到上传控件", image_paths.len())),
                Err(error) => messages.push(format!("上传控件处理失败: {error}")),
            }
            sleep(Duration::from_secs(5)).await;
        }

        if platform != "zhihu" {
            match page.fill_text(platform, title, body).await {
                Ok(message) => messages.push(message),
                Err(error) => messages.push(format!("填标题/正文失败: {error}")),
            }
        }

        if platform == "xhs" {
            let template_path =
                xhs_api_template_path.ok_or_else(|| anyhow!("小红书 API 模板路径未配置"))?;
            messages.push(page.publish_xhs_via_api(template_path).await?);
        } else {
            messages.push(page.click_publish(platform).await?);
        }

        Ok(messages.join("；"))
    }

    pub async fn get_cookies(&self, launch: &BrowserLaunch) -> Result<Vec<BrowserCookie>> {
        let web_socket_debugger_url = match &launch.web_socket_debugger_url {
            Some(url) => url.clone(),
            None => self
                .find_page_ws_url(launch.port, &launch.url)
                .await?
                .ok_or_else(|| anyhow!("CDP page websocket was not found"))?,
        };
        let mut page = CdpPage::connect(&web_socket_debugger_url).await?;
        let result = page.call("Network.getAllCookies", json!({})).await?;
        let cookies = result
            .get("cookies")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        Ok(serde_json::from_value(cookies)?)
    }

    pub async fn close_all_tabs(&self, port: u16) -> Result<usize> {
        let url = format!("http://127.0.0.1:{port}/json");
        let targets = match http_client().get(url).send().await {
            Ok(response) if response.status().is_success() => {
                response.json::<Vec<TargetInfo>>().await.unwrap_or_default()
            }
            _ => return Ok(0),
        };

        let mut closed = 0;
        for target in targets
            .into_iter()
            .filter(|target| target.target_type == "page")
        {
            let close_url = format!("http://127.0.0.1:{port}/json/close/{}", target.id);
            if http_client().get(close_url).send().await.is_ok() {
                closed += 1;
            }
        }
        Ok(closed)
    }

    async fn close_other_tabs(&self, port: u16, keep_id: &str) -> Result<usize> {
        let url = format!("http://127.0.0.1:{port}/json");
        let targets = match http_client().get(url).send().await {
            Ok(response) if response.status().is_success() => {
                response.json::<Vec<TargetInfo>>().await.unwrap_or_default()
            }
            _ => return Ok(0),
        };

        let mut closed = 0;
        for target in targets
            .into_iter()
            .filter(|target| target.target_type == "page" && target.id != keep_id)
        {
            let close_url = format!("http://127.0.0.1:{port}/json/close/{}", target.id);
            if http_client().get(close_url).send().await.is_ok() {
                closed += 1;
            }
        }
        Ok(closed)
    }

    async fn wait_until_ready(&self, port: u16) -> Result<()> {
        for _ in 0..30 {
            if self.is_ready(port).await {
                return Ok(());
            }
            sleep(Duration::from_millis(250)).await;
        }
        anyhow::bail!("browser CDP endpoint did not become ready on port {port}")
    }

    async fn is_ready(&self, port: u16) -> bool {
        let url = format!("http://127.0.0.1:{port}/json/version");
        match http_client().get(url).send().await {
            Ok(response) if response.status().is_success() => {
                match response.json::<VersionResponse>().await {
                    Ok(version) => version.browser.is_some(),
                    Err(_) => true,
                }
            }
            _ => false,
        }
    }

    async fn find_page_ws_url(&self, port: u16, preferred_url: &str) -> Result<Option<String>> {
        let url = format!("http://127.0.0.1:{port}/json");
        let targets = http_client()
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<TargetInfo>>()
            .await?;

        let mut pages = targets
            .into_iter()
            .filter(|target| target.target_type == "page")
            .collect::<Vec<_>>();
        pages.sort_by_key(|target| {
            if target.url == preferred_url || target.url.contains(preferred_url) {
                0
            } else {
                1
            }
        });

        Ok(pages
            .into_iter()
            .find_map(|target| target.web_socket_debugger_url))
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

struct CdpPage {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    next_id: u64,
    accept_beforeunload: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CapturedApiRequest {
    url: String,
    method: String,
    post_data: Option<String>,
    content_type: Option<String>,
}

impl CdpPage {
    async fn connect(url: &str) -> Result<Self> {
        let (socket, _) = connect_async(url)
            .await
            .with_context(|| format!("connect CDP websocket {url}"))?;
        Ok(Self {
            socket,
            next_id: 1,
            accept_beforeunload: false,
        })
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        self.socket
            .send(Message::Text(request.to_string().into()))
            .await
            .with_context(|| format!("send CDP method {method}"))?;

        while let Some(message) = self.socket.next().await {
            let message = message?;
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            if value.get("method").and_then(Value::as_str) == Some("Page.javascriptDialogOpening") {
                let dialog_type = value
                    .pointer("/params/type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.handle_javascript_dialog(dialog_type).await;
                continue;
            }
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = value.get("error") {
                    anyhow::bail!("CDP {method} failed: {error}");
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        }

        anyhow::bail!("CDP connection closed while waiting for {method}")
    }

    async fn handle_javascript_dialog(&mut self, dialog_type: &str) {
        let accept = dialog_type != "beforeunload" || self.accept_beforeunload;
        let _ = self
            .socket
            .send(Message::Text(
                json!({
                    "id": self.next_id,
                    "method": "Page.handleJavaScriptDialog",
                    "params": { "accept": accept }
                })
                .to_string()
                .into(),
            ))
            .await;
        self.next_id += 1;
    }

    async fn drain_dialog_events(&mut self) -> Result<()> {
        for _ in 0..5 {
            let Ok(message) = timeout(Duration::from_millis(120), self.socket.next()).await else {
                return Ok(());
            };
            let Some(message) = message else {
                return Ok(());
            };
            let Message::Text(text) = message? else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            if value.get("method").and_then(Value::as_str) == Some("Page.javascriptDialogOpening") {
                let dialog_type = value
                    .pointer("/params/type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.handle_javascript_dialog(dialog_type).await;
            }
        }
        Ok(())
    }

    async fn click_xhs_image_tab(&mut self) -> Result<()> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": xhs::image_tab_center_script(),
                    "awaitPromise": true,
                    "returnByValue": true
                }),
            )
            .await?;
        let (x, y) =
            point_from_result(&result).ok_or_else(|| anyhow!("没有找到小红书顶部上传图文 tab"))?;
        self.click_point(x, y).await
    }

    async fn click_publish(&mut self, platform: &str) -> Result<String> {
        if platform == "xhs" {
            return self.click_xhs_publish().await;
        }
        if platform == "zhihu" {
            return self.click_zhihu_publish().await;
        }

        let labels = ["发布", "确认发布", "立即发布", ""];

        let mut clicked = Vec::new();
        for _ in 0..2 {
            let mut did_click = false;
            for label in labels {
                if label.is_empty() {
                    continue;
                }
                let expression = publish_button_center_script(label);
                let result = self
                    .call(
                        "Runtime.evaluate",
                        json!({
                            "expression": expression,
                            "awaitPromise": true,
                            "returnByValue": true
                        }),
                    )
                    .await?;
                if let Some((x, y)) = point_from_result(&result) {
                    self.click_point(x, y).await?;
                    clicked.push(label.to_string());
                    did_click = true;
                    sleep(Duration::from_secs(2)).await;
                    break;
                }
            }
            if !did_click {
                break;
            }
        }

        if clicked.is_empty() {
            anyhow::bail!("没有找到可点击的发布按钮");
        }

        Ok(format!("已自动点击发布按钮：{}", clicked.join(" -> ")))
    }

    async fn click_zhihu_publish(&mut self) -> Result<String> {
        self.accept_beforeunload = true;
        let publish_result = async {
            let first = self
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": zhihu::main_publish_center_script(),
                        "awaitPromise": true,
                        "returnByValue": true
                    }),
                )
                .await?;
            let (x, y) =
                point_from_result(&first).ok_or_else(|| anyhow!("没有找到知乎发布按钮"))?;
            self.click_point(x, y).await?;
            self.drain_dialog_events().await?;
            sleep(Duration::from_millis(300)).await;

            for _ in 0..20 {
                let confirm = self
                    .call(
                        "Runtime.evaluate",
                        json!({
                            "expression": zhihu::confirm_publish_center_script(),
                            "awaitPromise": true,
                            "returnByValue": true
                        }),
                    )
                    .await?;
                if let Some((x, y)) = point_from_result(&confirm) {
                    self.click_point(x, y).await?;
                    self.drain_dialog_events().await?;
                    sleep(Duration::from_secs(2)).await;
                    return Ok("已自动点击知乎发布确认按钮".to_string());
                }
                sleep(Duration::from_millis(250)).await;
            }

            Ok("已自动点击知乎底部发布按钮，未发现二次确认弹窗".to_string())
        }
        .await;
        self.accept_beforeunload = false;
        publish_result
    }

    async fn click_xhs_publish(&mut self) -> Result<String> {
        let mut clicked = Vec::new();
        for label in ["发布", "确认发布", "立即发布"] {
            let result = self
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": xhs::evaluate_publish_script(label),
                        "awaitPromise": true,
                        "returnByValue": true
                    }),
                )
                .await?;
            if result
                .pointer("/result/value/clicked")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                self.drain_dialog_events().await?;
                let matched_api = result
                    .pointer("/result/value/matchedApi")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if matched_api.is_empty() {
                    clicked.push(label.to_string());
                } else {
                    clicked.push(format!("{label} api={matched_api}"));
                }
                sleep(Duration::from_secs(2)).await;
                break;
            }
        }

        if clicked.is_empty() {
            anyhow::bail!("没有找到小红书底部发布按钮");
        }

        Ok(format!(
            "已自动点击小红书底部发布按钮：{}",
            clicked.join(" -> ")
        ))
    }

    async fn publish_xhs_via_api(&mut self, template_path: &Path) -> Result<String> {
        if template_path.exists() {
            let template = std::fs::read_to_string(template_path)
                .with_context(|| format!("read {}", template_path.display()))?;
            let template: CapturedApiRequest = serde_json::from_str(&template)
                .with_context(|| format!("parse {}", template_path.display()))?;
            let result = self
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": xhs::publish_with_api_template_script(&template.url, &template.method, template.post_data.as_deref(), template.content_type.as_deref()),
                        "awaitPromise": true,
                        "returnByValue": true
                    }),
                )
                .await?;
            let ok = result
                .pointer("/result/value/ok")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let status = result
                .pointer("/result/value/status")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let text = result
                .pointer("/result/value/text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !ok {
                anyhow::bail!("小红书 API 发布失败 status={status} response={text}");
            }
            return Ok(format!("小红书已通过 API 发布 status={status}"));
        }

        self.capture_xhs_publish_api(template_path).await
    }

    async fn capture_xhs_publish_api(&mut self, template_path: &Path) -> Result<String> {
        if let Some(parent) = template_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        self.call(
            "Network.enable",
            json!({
                "maxPostDataSize": 1024 * 1024
            }),
        )
        .await?;
        tracing::warn!(
            path = %template_path.display(),
            "waiting for manual xhs publish click to capture API request"
        );

        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let message = timeout(
                remaining.min(Duration::from_millis(500)),
                self.socket.next(),
            )
            .await;
            let Ok(Some(message)) = message else {
                continue;
            };
            let Message::Text(text) = message? else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            if value.get("method").and_then(Value::as_str) == Some("Page.javascriptDialogOpening") {
                let dialog_type = value
                    .pointer("/params/type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.handle_javascript_dialog(dialog_type).await;
                continue;
            }
            if value.get("method").and_then(Value::as_str) != Some("Network.requestWillBeSent") {
                continue;
            }
            let Some(request) = value.pointer("/params/request") else {
                continue;
            };
            let url = request
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let method = request
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("POST");
            if !is_xhs_publish_api(url, method) {
                continue;
            }
            let content_type = request
                .pointer("/headers/content-type")
                .or_else(|| request.pointer("/headers/Content-Type"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let captured = CapturedApiRequest {
                url: url.to_string(),
                method: method.to_string(),
                post_data: request
                    .get("postData")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                content_type,
            };
            let json = serde_json::to_string_pretty(&captured)?;
            std::fs::write(template_path, json)
                .with_context(|| format!("write {}", template_path.display()))?;
            return Ok(format!(
                "已捕获小红书发布 API 并保存到 {}，本次请以手动点击为准，后续将自动走 API",
                template_path.display()
            ));
        }

        anyhow::bail!("90 秒内未捕获到小红书发布 API，请保持页面打开并手动点击发布")
    }

    async fn wait_for_platform_ready(&mut self, platform: &str) -> Result<()> {
        let expression = match platform {
            "zhihu" => {
                r#"
(() => Boolean(
  document.querySelector('textarea[placeholder*="标题"]') &&
  document.querySelector('.public-DraftEditor-content[contenteditable=true]')
))()
"#
            }
            "xhs" => {
                r#"
(() => Array.from(document.querySelectorAll('.creator-tab'))
  .some(el => (el.innerText || el.textContent || '').trim() === '上传图文'))()
"#
            }
            _ => "(() => document.readyState !== 'loading')()",
        };
        self.wait_for_truthy(expression, "发布页关键控件未加载完成")
            .await
    }

    async fn wait_for_image_upload_ready(&mut self) -> Result<()> {
        self.wait_for_truthy(
            r#"
(() => Array.from(document.querySelectorAll('input[type=file]'))
  .some(el => /jpg|jpeg|png|webp|image/i.test(el.accept || '')))()
"#,
            "图片上传控件未加载完成",
        )
        .await
    }

    async fn wait_for_truthy(&mut self, expression: &str, message: &str) -> Result<()> {
        for _ in 0..40 {
            let result = self
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": expression,
                        "awaitPromise": true,
                        "returnByValue": true
                    }),
                )
                .await?;
            if result
                .pointer("/result/value")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Ok(());
            }
            sleep(Duration::from_millis(250)).await;
        }
        anyhow::bail!("{message}");
    }

    async fn click_point(&mut self, x: f64, y: f64) -> Result<()> {
        self.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": x, "y": y }),
        )
        .await?;
        self.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        )
        .await?;
        self.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        )
        .await?;
        Ok(())
    }

    async fn set_first_file_input(&mut self, image_paths: &[PathBuf]) -> Result<()> {
        let document = self
            .call("DOM.getDocument", json!({ "depth": 2, "pierce": true }))
            .await?;
        let root_node_id = document
            .pointer("/root/nodeId")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("CDP DOM root node was not found"))?;
        let image_input = self
            .call(
                "DOM.querySelector",
                json!({
                    "nodeId": root_node_id,
                    "selector": "input[type=file][accept*='image/']"
                }),
            )
            .await?;
        let mut node_id = image_input
            .get("nodeId")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if node_id == 0 {
            let extension_input = self
                .call(
                    "DOM.querySelector",
                    json!({
                        "nodeId": root_node_id,
                        "selector": "input[type=file][accept*='.jpg'], input[type=file][accept*='.jpeg'], input[type=file][accept*='.png'], input[type=file][accept*='.webp']"
                    }),
                )
                .await?;
            node_id = extension_input
                .get("nodeId")
                .and_then(Value::as_i64)
                .unwrap_or_default();
        }
        if node_id == 0 {
            let fallback = self
                .call(
                    "DOM.querySelector",
                    json!({
                        "nodeId": root_node_id,
                        "selector": "input[type=file]"
                    }),
                )
                .await?;
            node_id = fallback
                .get("nodeId")
                .and_then(Value::as_i64)
                .ok_or_else(|| anyhow!("页面中没有找到图片上传控件"))?;
        }
        if node_id == 0 {
            anyhow::bail!("页面中没有找到图片上传控件");
        }
        let files = image_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        self.call(
            "DOM.setFileInputFiles",
            json!({
                "nodeId": node_id,
                "files": files,
            }),
        )
        .await?;
        Ok(())
    }

    async fn fill_text(&mut self, platform: &str, title: &str, body: &str) -> Result<String> {
        if platform == "zhihu" {
            return self.fill_zhihu_text(title, body).await;
        }

        let expression = fill_script(platform, title, body);
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true
                }),
            )
            .await?;
        Ok(result
            .pointer("/result/value/message")
            .and_then(Value::as_str)
            .unwrap_or("已尝试填充标题/正文")
            .to_string())
    }

    async fn fill_zhihu_text(&mut self, title: &str, body: &str) -> Result<String> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": zhihu::fill_text_script(title, body),
                    "awaitPromise": true,
                    "returnByValue": true
                }),
            )
            .await?;
        Ok(result
            .pointer("/result/value/message")
            .and_then(Value::as_str)
            .unwrap_or("知乎草稿已填充")
            .to_string())
    }
}

fn point_from_result(result: &Value) -> Option<(f64, f64)> {
    let value = result.pointer("/result/value")?;
    let x = value.get("x").and_then(Value::as_f64)?;
    let y = value.get("y").and_then(Value::as_f64)?;
    Some((x, y))
}

fn is_xhs_publish_api(url: &str, method: &str) -> bool {
    if !method.eq_ignore_ascii_case("POST") {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    (lower.contains("xiaohongshu.com") || lower.contains("xhscdn.com"))
        && (lower.contains("publish")
            || lower.contains("note")
            || lower.contains("post")
            || lower.contains("submit"))
        && !lower.contains("upload")
        && !lower.contains("image")
}

fn prepare_script(platform: &str) -> String {
    let platform = serde_json::to_string(platform).unwrap_or_else(|_| "\"unknown\"".to_string());
    format!(
        r#"
(() => {{
  const platform = {platform};
  const visible = (el) => {{
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }};
  const visibleInViewport = (el) => {{
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && rect.x >= 0 && rect.y >= 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }};
  const clickByExactText = (text) => {{
    const els = Array.from(document.querySelectorAll('button, [role=button], div, span, a'))
      .filter(visibleInViewport)
      .filter(el => (el.innerText || el.textContent || '').trim() === text)
      .sort((a, b) => {{
        const ar = a.getBoundingClientRect();
        const br = b.getBoundingClientRect();
        return (ar.width * ar.height) - (br.width * br.height);
      }});
    const el = els[0];
    if (!el) return null;
    const target = el.closest('.creator-tab, button, [role=button], a') || el;
    target.dispatchEvent(new MouseEvent('mousedown', {{ bubbles: true }}));
    target.dispatchEvent(new MouseEvent('mouseup', {{ bubbles: true }}));
    target.click();
    return text;
  }};
  const clickByText = (texts) => {{
    for (const el of Array.from(document.querySelectorAll('button, [role=button], div, span, a'))) {{
      const text = (el.innerText || el.textContent || '').trim();
      if (visible(el) && texts.some(t => text.includes(t))) {{
        el.click();
        return text;
      }}
    }}
    return null;
  }};
  if (platform === 'xhs') {{
    clickByExactText('上传图文') || clickByText(['上传图文', '发布笔记']);
  }}
  return true;
}})()
"#
    )
}

fn publish_button_center_script(text: &str) -> String {
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const wanted = {text};
  const visible = (el) => {{
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && rect.x >= 0 && rect.y >= 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }};
  const disabled = (el) => {{
    const aria = el.getAttribute('aria-disabled');
    const cls = String(el.className || '').toLowerCase();
    return el.disabled || aria === 'true' || cls.includes('disabled');
  }};
  const items = Array.from(document.querySelectorAll('button, [role=button], a, div'))
    .filter(visible)
    .filter(el => !disabled(el))
    .map(el => {{
      const label = (el.innerText || el.textContent || '').trim();
      const rect = el.getBoundingClientRect();
      return {{ label, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, area: rect.width * rect.height }};
    }})
    .filter(item => item.label === wanted && item.area >= 400 && item.area <= 20000)
    .sort((a, b) => a.area - b.area);
  return items[0] || null;
}})()
"#
    )
}

fn fill_script(platform: &str, title: &str, body: &str) -> String {
    let platform = serde_json::to_string(platform).unwrap_or_else(|_| "\"unknown\"".to_string());
    let title = serde_json::to_string(title).unwrap_or_else(|_| "\"\"".to_string());
    let body = serde_json::to_string(body).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const platform = {platform};
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
      if (descriptor && descriptor.set) {{
        descriptor.set.call(el, value);
      }} else {{
        el.value = value;
      }}
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

  let titleEl = platform === 'zhihu'
    ? document.querySelector('textarea[placeholder*="标题"]')
    : null;
  titleEl = titleEl || byPlaceholder('标题') || candidates('input, textarea').find(el => visible(el));
  if (!titleEl && platform === 'zhihu') {{
    titleEl = candidates('textarea, input, [contenteditable=true], [contenteditable="true"]').find(el => (el.innerText || el.textContent || el.placeholder || '').includes('标题'));
  }}
  if (titleEl) setText(titleEl, title);

  let bodyEl = platform === 'zhihu'
    ? document.querySelector('.public-DraftEditor-content[contenteditable=true]')
    : null;
  bodyEl = bodyEl || byPlaceholder('正文');
  if (!bodyEl) {{
    const editors = editable().filter(el => el !== titleEl);
    bodyEl = editors.find(el => (el.innerText || el.textContent || '').includes('正文')) || editors[0];
  }}
  if (!bodyEl) {{
    const areas = candidates('textarea').filter(el => el !== titleEl);
    bodyEl = areas[0];
  }}
  if (bodyEl && body) setText(bodyEl, body);

  return {{
    message: `已尝试填充草稿：标题${{titleEl ? '成功' : '未找到'}}，正文${{bodyEl ? '成功' : '未找到'}}。已上传图片控件由 CDP 处理，最终发布按钮暂未自动点击。`
  }};
}})()
"#
    )
}

async fn launch_browser(executable: &Path, profile_dir: &Path, port: u16, url: &str) -> Result<()> {
    tracing::info!(
        executable = %executable.display(),
        profile_dir = %profile_dir.display(),
        port,
        url,
        "launching CDP browser"
    );
    Command::new(executable)
        .arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-allow-origins=*")
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg("--no-first-run")
        .arg("--new-window")
        .arg("--disable-background-mode")
        .arg("--disable-session-crashed-bubble")
        .arg("--hide-crash-restore-bubble")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("launch browser {}", executable.display()))?;
    Ok(())
}

fn find_browser_executable() -> Option<PathBuf> {
    let candidates = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ];

    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
        .or_else(|| find_on_path("chrome.exe"))
        .or_else(|| find_on_path("msedge.exe"))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.exists())
}
