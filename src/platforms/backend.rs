use super::{Platform, SessionStatus};
use crate::{
    browser::cdp::{BrowserCookie, BrowserLaunch, CdpBrowser, CdpPage},
    config::PlatformSection,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime},
};
use tokio::time::sleep;

/// The content of a single publish action. Cheap to copy (all borrows).
#[derive(Debug, Clone, Copy)]
pub struct PublishContent<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub image_paths: &'a [PathBuf],
}

/// A single publishing transport for one platform. Both the CDP (browser
/// automation) and HTTP API backends implement this; the adapter prefers
/// CDP and falls back to the API.
#[async_trait]
pub trait PublishBackend: Send + Sync {
    /// Publish `content` and return a human-readable status message.
    async fn publish(&self, content: PublishContent<'_>) -> Result<String>;
}

// ---------------------------------------------------------------------------
// CDP flow: the per-platform step contract + a shared runner.
// ---------------------------------------------------------------------------

/// The per-platform browser-automation steps. Each `*_cdp.rs` implements this;
/// [`run_flow`] drives the steps in the right order. Adding a platform = writing
/// one `CdpFlow` impl; adapting to a UI change = editing one selector string.
#[async_trait]
pub trait CdpFlow: Send + Sync {
    /// Wait for the compose page to be ready and select the right tab / open the
    /// composer (e.g. Xiaohongshu's "上传图文" tab, Twitter's compose button).
    async fn prepare(&self, page: &mut CdpPage) -> Result<()>;

    /// Attach the images to the page's upload control.
    async fn upload_images(&self, page: &mut CdpPage, images: &[PathBuf]) -> Result<String>;

    /// Fill the title and/or body fields.
    async fn fill_text(&self, page: &mut CdpPage, title: &str, body: &str) -> Result<String>;

    /// Optional: wait for the publish button to become enabled before clicking.
    async fn wait_publish_ready(&self, _page: &mut CdpPage) -> Result<()> {
        Ok(())
    }

    /// Click the final publish/submit control.
    async fn click_publish(&self, page: &mut CdpPage) -> Result<String>;

    /// `true` if text must be filled *before* uploading images (Zhihu), `false`
    /// if images are uploaded first and text filled after (Xiaohongshu, default).
    fn fill_before_upload(&self) -> bool {
        false
    }
}

/// Open the publish page in the platform's browser profile and run `flow`.
pub async fn cdp_publish(
    browser: &CdpBrowser,
    platform: Platform,
    port: u16,
    profile_dir: &std::path::Path,
    publish_url: &str,
    flow: &dyn CdpFlow,
    content: PublishContent<'_>,
) -> Result<String> {
    let launch = browser
        .open_visible(profile_dir, port, publish_url)
        .await
        .with_context(|| format!("open {platform} in browser"))?;
    let message = run_flow(browser, &launch, flow, content).await?;
    tracing::warn!(
        platform = %platform,
        port = launch.port,
        url = %launch.url,
        image_count = content.image_paths.len(),
        message = %message,
        "cdp article submitted"
    );
    Ok(message)
}

/// Drive a [`CdpFlow`] over a connected page: prepare → (fill?) → upload →
/// (fill?) → wait → publish, preserving the platform's fill/upload ordering.
pub async fn run_flow(
    browser: &CdpBrowser,
    launch: &BrowserLaunch,
    flow: &dyn CdpFlow,
    content: PublishContent<'_>,
) -> Result<String> {
    let mut page = browser.connect_page(launch).await?;
    page.navigate(&launch.url).await?;
    sleep(Duration::from_secs(2)).await;
    flow.prepare(&mut page).await?;
    sleep(Duration::from_millis(800)).await;

    let mut messages = Vec::new();
    if flow.fill_before_upload() {
        match flow.fill_text(&mut page, content.title, content.body).await {
            Ok(message) => messages.push(message),
            Err(error) => messages.push(format!("填标题/正文失败: {error}")),
        }
    }

    if !content.image_paths.is_empty() {
        match flow.upload_images(&mut page, content.image_paths).await {
            Ok(message) => messages.push(message),
            Err(error) => messages.push(format!("上传控件处理失败: {error}")),
        }
        sleep(Duration::from_secs(5)).await;
    }

    if !flow.fill_before_upload() {
        match flow.fill_text(&mut page, content.title, content.body).await {
            Ok(message) => messages.push(message),
            Err(error) => messages.push(format!("填标题/正文失败: {error}")),
        }
    }

    flow.wait_publish_ready(&mut page).await?;
    messages.push(flow.click_publish(&mut page).await?);

    Ok(messages.join("；"))
}

/// JS returning the center `{x, y}` of the smallest visible, enabled element
/// whose trimmed text exactly equals `label` — for clicking publish-style buttons.
pub fn label_center_script(label: &str) -> String {
    let label = serde_json::to_string(label).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const wanted = {label};
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
  const items = Array.from(document.querySelectorAll('button, [role=button], a, div, span'))
    .filter(visible)
    .filter(el => !disabled(el))
    .map(el => {{ const label = (el.innerText || el.textContent || '').replace(/\s+/g, '').trim(); const rect = el.getBoundingClientRect(); return {{ label, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, area: rect.width * rect.height }}; }})
    .filter(item => item.label === wanted && item.area >= 300 && item.area <= 40000)
    .sort((a, b) => a.area - b.area);
  return items[0] || null;
}})()
"#
    )
}

/// JS returning the center `{x, y}` of the first visible, enabled element matching
/// `selector` — for clicking buttons identified by a stable selector such as a
/// `data-testid`, which is locale-proof (unlike matching button text).
pub fn selector_center_script(selector: &str) -> String {
    let selector = serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const sel = {selector};
  const visible = (el) => {{
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }};
  const disabled = (el) => el.disabled || el.getAttribute('aria-disabled') === 'true';
  const el = Array.from(document.querySelectorAll(sel)).filter(visible).find(e => !disabled(e));
  if (!el) return null;
  const rect = el.getBoundingClientRect();
  return {{ x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 }};
}})()
"#
    )
}

/// JS that fills the first visible `textarea`/`contenteditable` matching any of
/// `selectors` with `value` (used by the simpler status/note composers).
pub fn fill_editable_script(selectors: &str, value: &str) -> String {
    let selectors = serde_json::to_string(selectors).unwrap_or_else(|_| "\"\"".to_string());
    let value = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const selectors = {selectors};
  const value = {value};
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
  const el = Array.from(document.querySelectorAll(selectors)).filter(visible)[0];
  if (el) setText(el, value);
  return {{ message: el ? '已填充' : '未找到输入框' }};
}})()
"#
    )
}

// ---------------------------------------------------------------------------
// Cookie store: shared login-state + cookie capture for all backends.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct CookieSnapshot {
    platform: String,
    saved_at: String,
    cookies: Vec<BrowserCookie>,
}

/// Owns a platform's browser profile + encrypted cookie snapshot. API backends
/// pull cookies from here; the adapter uses it for session validation and login.
pub struct CookieStore {
    platform: Platform,
    platform_config: PlatformSection,
    profile_dir: PathBuf,
    auth_file: PathBuf,
    browser: CdpBrowser,
}

impl CookieStore {
    pub fn new(
        platform: Platform,
        platform_config: PlatformSection,
        profile_dir: PathBuf,
        auth_file: PathBuf,
    ) -> Self {
        Self {
            platform,
            platform_config,
            profile_dir,
            auth_file,
            browser: CdpBrowser,
        }
    }

    /// Probe login state from the cookie snapshot, the running browser, or the
    /// on-disk Chrome profile. CDP publishing only needs a live profile, so a
    /// present-but-unverified profile maps to `RiskVerificationRequired` rather
    /// than `Missing` (which keeps manual publish available).
    pub async fn validate_session(&self) -> Result<SessionStatus> {
        if self.load_snapshot().is_ok() || self.try_capture_from_running_browser().await? {
            Ok(SessionStatus::Valid { account_name: None })
        } else if self.has_login_cookie()?
            || self.has_recent_profile_activity()?
            || self.has_browser_profile()
        {
            Ok(SessionStatus::RiskVerificationRequired)
        } else {
            Ok(SessionStatus::Missing)
        }
    }

    /// Open the platform login page in a visible browser window.
    pub async fn open_login(&self) -> Result<SessionStatus> {
        self.browser
            .open_visible(
                &self.profile_dir,
                self.platform_config.cdp_port,
                &self.platform_config.login_url,
            )
            .await
            .with_context(|| format!("open {} login page", self.platform))?;
        if let Some(parent) = self.auth_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create auth dir {}", parent.display()))?;
        }
        Ok(SessionStatus::RiskVerificationRequired)
    }

    /// Return saved cookies, capturing a fresh snapshot from the running browser
    /// if needed. API backends call this before signing API requests.
    pub async fn load_or_capture(&self) -> Result<Vec<BrowserCookie>> {
        if let Ok(cookies) = self.load_snapshot() {
            return Ok(cookies);
        }
        if self.try_capture_from_running_browser().await? {
            return self.load_snapshot();
        }
        anyhow::bail!(
            "{} cookie 快照不存在或已失效，请先登录并等待状态显示已登录",
            self.platform
        )
    }

    fn load_snapshot(&self) -> Result<Vec<BrowserCookie>> {
        let text = fs::read_to_string(&self.auth_file)
            .with_context(|| format!("read {}", self.auth_file.display()))?;
        let snapshot: CookieSnapshot = serde_json::from_str(&text)
            .with_context(|| format!("parse {}", self.auth_file.display()))?;
        self.validate_required_cookies(&snapshot.cookies)?;
        Ok(snapshot.cookies)
    }

    async fn try_capture_from_running_browser(&self) -> Result<bool> {
        let launch = BrowserLaunch {
            port: self.platform_config.cdp_port,
            url: self.platform_config.login_url.clone(),
            web_socket_debugger_url: None,
        };
        let cookies = match self.browser.get_cookies(&launch).await {
            Ok(cookies) => cookies,
            Err(_) => return Ok(false),
        };
        let cookies = cookies
            .into_iter()
            .filter(|cookie| self.cookie_belongs_to_platform(cookie))
            .collect::<Vec<_>>();
        if self.validate_required_cookies(&cookies).is_err() {
            return Ok(false);
        }
        self.save_snapshot(cookies)?;
        Ok(true)
    }

    fn save_snapshot(&self, cookies: Vec<BrowserCookie>) -> Result<()> {
        if let Some(parent) = self.auth_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create auth dir {}", parent.display()))?;
        }
        let snapshot = CookieSnapshot {
            platform: self.platform.to_string(),
            saved_at: Utc::now().to_rfc3339(),
            cookies,
        };
        let text = serde_json::to_string_pretty(&snapshot)?;
        fs::write(&self.auth_file, text)
            .with_context(|| format!("write {}", self.auth_file.display()))
    }

    fn cookie_belongs_to_platform(&self, cookie: &BrowserCookie) -> bool {
        self.platform
            .cookie_domains()
            .iter()
            .any(|domain| cookie.domain.contains(domain))
    }

    fn validate_required_cookies(&self, cookies: &[BrowserCookie]) -> Result<()> {
        for name in self.platform.required_cookies() {
            if !cookies.iter().any(|cookie| cookie.name == *name) {
                anyhow::bail!("missing required cookie {name}");
            }
        }
        Ok(())
    }

    fn has_login_cookie(&self) -> Result<bool> {
        let cookie_db = self
            .profile_dir
            .join("Default")
            .join("Network")
            .join("Cookies");
        if !cookie_db.exists() {
            return Ok(false);
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
        let uri = format!(
            "file:{}?mode=ro&immutable=1",
            cookie_db.display().to_string().replace('\\', "/")
        );
        let conn = match Connection::open_with_flags(uri, flags) {
            Ok(conn) => conn,
            Err(error) => {
                tracing::warn!(
                    platform = %self.platform,
                    error = %error,
                    "cookie database is not readable yet"
                );
                return self.cookie_file_contains_login_cookie(&cookie_db);
            }
        };

        let host_pattern = self.platform.cookie_host_pattern();
        for cookie_name in self.platform.login_cookie_names() {
            let count: i64 = conn.query_row(
                "SELECT COUNT(1) FROM cookies WHERE host_key LIKE ?1 AND name = ?2",
                params![host_pattern, cookie_name],
                |row| row.get(0),
            )?;
            if count > 0 {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn cookie_file_contains_login_cookie(&self, cookie_db: &PathBuf) -> Result<bool> {
        let bytes = match fs::read(cookie_db) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(
                    platform = %self.platform,
                    error = %error,
                    "cookie database bytes are not readable"
                );
                return Ok(false);
            }
        };

        Ok(self
            .platform
            .login_cookie_names()
            .iter()
            .map(|name| name.as_bytes())
            .any(|name| bytes.windows(name.len()).any(|window| window == name)))
    }

    fn has_recent_profile_activity(&self) -> Result<bool> {
        let local_state = self.profile_dir.join("Local State");
        let default_dir = self.profile_dir.join("Default");
        let latest = [local_state, default_dir]
            .into_iter()
            .filter_map(|path| fs::metadata(path).ok())
            .filter_map(|metadata| metadata.modified().ok())
            .max();

        let Some(modified) = latest else {
            return Ok(false);
        };
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            .as_secs();
        Ok(age <= 60 * 60 * 24)
    }

    fn has_browser_profile(&self) -> bool {
        self.profile_dir.join("Default").exists() || self.profile_dir.join("Local State").exists()
    }
}
