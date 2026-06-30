use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
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

/// A randomized, human-like pause centered on `base_ms` — used in place of fixed
/// sleeps so automated steps don't share one robotic cadence (anti-风控).
pub async fn human_pause(base_ms: u64) {
    sleep(Duration::from_millis(jitter_ms(base_ms))).await;
}

/// Scale `base_ms` by a random factor (~0.6–1.6×) plus a little absolute jitter.
/// (RNG is dropped before any await — `thread_rng` is not `Send`.)
fn jitter_ms(base_ms: u64) -> u64 {
    let mut rng = rand::thread_rng();
    let factor = rng.gen_range(0.6_f64..1.6);
    let extra = rng.gen_range(0_u64..90);
    (base_ms as f64 * factor) as u64 + extra
}

/// A single human-like inter-keystroke delay (occasionally a longer think pause).
fn keystroke_delay() -> u64 {
    let mut rng = rand::thread_rng();
    if rng.gen_bool(0.12) {
        rng.gen_range(280_u64..620)
    } else {
        rng.gen_range(70_u64..185)
    }
}

/// Detects a visible manual-verification prompt (SMS code / captcha / slider /
/// scan-login) on the page, so a publish run can pause instead of auto-closing.
/// The phrase must sit inside a modal/overlay (role=dialog, aria-modal, or a
/// fixed-position ancestor) — real verification is always a popup, so this avoids
/// false positives from ordinary page text that happens to contain "验证".
const INTERVENTION_SCRIPT: &str = r#"
(() => {
  const vis=(el)=>{const r=el.getBoundingClientRect();const s=getComputedStyle(el);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none';};
  const phrases=['短信验证码','安全验证','人机验证','拖动滑块','滑块验证','扫码登录','完成验证','验证身份','请输入验证码','captcha'];
  const inOverlay=(el)=>{let n=el;for(let i=0;i<10&&n;i++){const role=n.getAttribute&&n.getAttribute('role');const modal=n.getAttribute&&n.getAttribute('aria-modal');const pos=getComputedStyle(n).position;if(role==='dialog'||modal==='true'||pos==='fixed') return true;n=n.parentElement;}return false;};
  const els=[...document.querySelectorAll('div,span,p,h1,h2,h3,label')].filter(vis);
  return els.some(e=>{const t=(e.innerText||'').trim();return t.length>0&&t.length<60&&phrases.some(p=>t.includes(p))&&inOverlay(e);});
})()
"#;

/// Default ordered candidate selectors for locating an image file input.
/// Per-platform flows can pass their own list to [`CdpPage::set_file_input`].
pub const DEFAULT_FILE_INPUT_SELECTORS: &[&str] = &[
    "input[type=file][accept*='image/']",
    "input[type=file][accept*='.jpg'], input[type=file][accept*='.jpeg'], input[type=file][accept*='.png'], input[type=file][accept*='.webp']",
    "input[type=file]",
];

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
                // Keep existing tabs open: one shared window holds a tab per
                // platform during a manual publish (they're closed together after).
                Some(response) => match response.json::<TargetInfo>().await.ok() {
                    Some(target) => target.web_socket_debugger_url,
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

    /// Resolve the page websocket for a launch, connect, and bring the tab to front.
    /// This is the single entry point every flow uses to obtain a [`CdpPage`].
    pub async fn connect_page(&self, launch: &BrowserLaunch) -> Result<CdpPage> {
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
        Ok(page)
    }

    /// Returns true if any open tab is showing a manual-intervention UI — SMS code,
    /// captcha, slider, or scan-login verification — so the post-publish auto-close
    /// can keep the window open for the user to finish.
    pub async fn has_pending_intervention(&self, port: u16) -> bool {
        let url = format!("http://127.0.0.1:{port}/json");
        let targets = match http_client().get(url).send().await {
            Ok(response) if response.status().is_success() => {
                response.json::<Vec<TargetInfo>>().await.unwrap_or_default()
            }
            _ => return false,
        };
        for target in targets.into_iter().filter(|t| t.target_type == "page") {
            let Some(ws) = target.web_socket_debugger_url else {
                continue;
            };
            let Ok(mut page) = CdpPage::connect(&ws).await else {
                continue;
            };
            // Guard against a tab stuck on a native dialog (evaluate would hang).
            if let Ok(Ok(value)) =
                timeout(Duration::from_secs(3), page.evaluate(INTERVENTION_SCRIPT)).await
            {
                if value
                    .pointer("/result/value")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return true;
                }
            }
        }
        false
    }

    pub async fn get_cookies(&self, launch: &BrowserLaunch) -> Result<Vec<BrowserCookie>> {
        let mut page = self.connect_page(launch).await?;
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

    /// Gracefully close the whole browser (all tabs + the process) via the
    /// browser-level `Browser.close`, falling back to closing each tab.
    pub async fn close_browser(&self, port: u16) -> Result<()> {
        let ws = match http_client()
            .get(format!("http://127.0.0.1:{port}/json/version"))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => response
                .json::<Value>()
                .await
                .ok()
                .and_then(|value| {
                    value
                        .get("webSocketDebuggerUrl")
                        .and_then(Value::as_str)
                        .map(String::from)
                }),
            _ => None,
        };

        let Some(ws) = ws else {
            let _ = self.close_all_tabs(port).await;
            return Ok(());
        };
        if let Ok((mut socket, _)) = connect_async(ws).await {
            let _ = socket
                .send(Message::Text(
                    json!({ "id": 1, "method": "Browser.close" })
                        .to_string()
                        .into(),
                ))
                .await;
        }
        Ok(())
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

/// A connected CDP page. Exposes only generic, platform-agnostic primitives;
/// every platform-specific selector/flow lives in a `platforms/*_cdp.rs` module
/// that drives these primitives.
pub struct CdpPage {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    next_id: u64,
    accept_beforeunload: bool,
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

    /// Navigate the page to `url`.
    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        self.call("Page.navigate", json!({ "url": url })).await?;
        Ok(())
    }

    /// Evaluate a JS expression with `awaitPromise` + `returnByValue` and return
    /// the raw CDP result `Value` (callers pointer into `/result/value`).
    pub async fn evaluate(&mut self, expression: &str) -> Result<Value> {
        self.call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await
    }

    /// Evaluate `expression` and return the `{x, y}` center point it yields, if any.
    pub async fn eval_point(&mut self, expression: &str) -> Result<Option<(f64, f64)>> {
        let result = self.evaluate(expression).await?;
        Ok(point_from_result(&result))
    }

    /// Evaluate `expression` (expected to return a center point) and click it.
    /// Returns `true` if a point was found and clicked.
    pub async fn click_eval(&mut self, expression: &str) -> Result<bool> {
        match self.eval_point(expression).await? {
            Some((x, y)) => {
                self.click_point(x, y).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Poll `expression` until it evaluates truthy, or bail with `message`.
    pub async fn wait_for_truthy(&mut self, expression: &str, message: &str) -> Result<()> {
        for _ in 0..40 {
            let result = self.evaluate(expression).await?;
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

    pub async fn click_point(&mut self, x: f64, y: f64) -> Result<()> {
        self.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": x, "y": y }),
        )
        .await?;
        // Settle after the move, then hold the button for a human-like moment.
        human_pause(45).await;
        self.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        )
        .await?;
        let hold = {
            let mut rng = rand::thread_rng();
            rng.gen_range(45_u64..135)
        };
        sleep(Duration::from_millis(hold)).await;
        self.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        )
        .await?;
        Ok(())
    }

    /// Type `text` via per-character "char" key events — the post-IME input path
    /// that DraftJS-style editors (X/Twitter) actually commit to state, and which
    /// handles CJK (unlike `insertText`/execCommand, which only touch the DOM).
    /// Newlines become Enter key presses (composer line breaks). Use after a
    /// trusted click that focuses the editor; delays are randomized (human-like).
    pub async fn type_chars(&mut self, text: &str) -> Result<()> {
        for ch in text.chars() {
            if ch == '\n' {
                for kind in ["keyDown", "keyUp"] {
                    self.call(
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": kind,
                            "key": "Enter",
                            "code": "Enter",
                            "windowsVirtualKeyCode": 13,
                            "nativeVirtualKeyCode": 13,
                        }),
                    )
                    .await?;
                }
            } else {
                let s = ch.to_string();
                self.call(
                    "Input.dispatchKeyEvent",
                    json!({ "type": "char", "text": s, "key": s, "unmodifiedText": s }),
                )
                .await?;
            }
            sleep(Duration::from_millis(keystroke_delay())).await;
        }
        Ok(())
    }

    /// Type `text` into the focused element one character at a time with
    /// randomized, human-like inter-keystroke delays.
    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        for ch in text.chars() {
            let unit = ch.to_string();
            self.insert_text(&unit).await?;
            sleep(Duration::from_millis(keystroke_delay())).await;
        }
        Ok(())
    }

    /// Type `text` into the currently focused element as if typed (fires the
    /// editor's input/composition handlers — needed for mention/topic search boxes).
    pub async fn insert_text(&mut self, text: &str) -> Result<()> {
        self.call("Input.insertText", json!({ "text": text })).await?;
        Ok(())
    }

    /// Evaluate `expression` and read its boolean `/result/value` (false on miss).
    pub async fn eval_bool(&mut self, expression: &str) -> Result<bool> {
        Ok(self
            .evaluate(expression)
            .await?
            .pointer("/result/value")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    /// Dispatch a Ctrl+Enter key chord — the universal "send" shortcut for most
    /// composers (Twitter/X, many editors). The composer must already be focused.
    pub async fn press_ctrl_enter(&mut self) -> Result<()> {
        for kind in ["keyDown", "keyUp"] {
            self.call(
                "Input.dispatchKeyEvent",
                json!({
                    "type": kind,
                    "modifiers": 2,
                    "key": "Enter",
                    "code": "Enter",
                    "windowsVirtualKeyCode": 13,
                    "nativeVirtualKeyCode": 13,
                }),
            )
            .await?;
        }
        Ok(())
    }

    /// Press Backspace `count` times in the focused element — used to erase text
    /// just typed into an editor (e.g. an unresolved topic keyword) so no literal
    /// leftover remains.
    pub async fn press_backspace(&mut self, count: usize) -> Result<()> {
        for _ in 0..count {
            for kind in ["keyDown", "keyUp"] {
                self.call(
                    "Input.dispatchKeyEvent",
                    json!({
                        "type": kind,
                        "key": "Backspace",
                        "code": "Backspace",
                        "windowsVirtualKeyCode": 8,
                        "nativeVirtualKeyCode": 8,
                    }),
                )
                .await?;
            }
            sleep(Duration::from_millis(keystroke_delay())).await;
        }
        Ok(())
    }

    /// Attach `image_paths` to the first file input matching any selector in
    /// `selectors` (tried in order). Pass [`DEFAULT_FILE_INPUT_SELECTORS`] for the
    /// generic image-input heuristic.
    pub async fn set_file_input(
        &mut self,
        selectors: &[&str],
        image_paths: &[PathBuf],
    ) -> Result<()> {
        let document = self
            .call("DOM.getDocument", json!({ "depth": 2, "pierce": true }))
            .await?;
        let root_node_id = document
            .pointer("/root/nodeId")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("CDP DOM root node was not found"))?;

        let mut node_id = 0_i64;
        for selector in selectors {
            let found = self
                .call(
                    "DOM.querySelector",
                    json!({ "nodeId": root_node_id, "selector": selector }),
                )
                .await?;
            node_id = found.get("nodeId").and_then(Value::as_i64).unwrap_or(0);
            if node_id != 0 {
                break;
            }
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

    /// Control whether `beforeunload` dialogs are accepted (used by flows that
    /// navigate away after submitting, e.g. Zhihu's publish confirmation).
    pub fn set_accept_beforeunload(&mut self, accept: bool) {
        self.accept_beforeunload = accept;
    }

    /// Keep reading socket events for ~`total_ms`, accepting any JS dialog that
    /// opens — used after a publish-confirm click whose navigation fires a
    /// `beforeunload` prompt a moment later (a short drain races and misses it).
    pub async fn pump_dialogs(&mut self, total_ms: u64) -> Result<()> {
        let iters = (total_ms / 120).max(1);
        for _ in 0..iters {
            let Ok(message) = timeout(Duration::from_millis(120), self.socket.next()).await else {
                continue; // quiet window — keep pumping until total_ms elapses
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

    /// Drain any pending `javascriptDialogOpening` events for a short window.
    pub async fn drain_dialog_events(&mut self) -> Result<()> {
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
}

/// Extract a `{x, y}` center point from a `Runtime.evaluate` result.
pub fn point_from_result(result: &Value) -> Option<(f64, f64)> {
    let value = result.pointer("/result/value")?;
    let x = value.get("x").and_then(Value::as_f64)?;
    let y = value.get("y").and_then(Value::as_f64)?;
    Some((x, y))
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
