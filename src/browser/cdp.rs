use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    process::Command,
    time::{sleep, Duration},
};

#[derive(Debug, Clone)]
pub struct BrowserLaunch {
    pub port: u16,
    pub url: String,
}

#[derive(Debug, Default, Clone)]
pub struct CdpBrowser;

#[derive(Debug, Deserialize)]
struct VersionResponse {
    #[serde(rename = "Browser")]
    browser: Option<String>,
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

        if !self.is_ready(port).await {
            let executable = find_browser_executable()
                .ok_or_else(|| anyhow!("Chrome or Edge executable was not found"))?;
            launch_browser(&executable, profile_dir, port, url).await?;
        }

        self.wait_until_ready(port).await?;
        let _ = reqwest::Client::new()
            .put(format!("http://127.0.0.1:{port}/json/new?{url}"))
            .send()
            .await;

        Ok(BrowserLaunch {
            port,
            url: url.to_string(),
        })
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
        match reqwest::get(url).await {
            Ok(response) if response.status().is_success() => {
                match response.json::<VersionResponse>().await {
                    Ok(version) => version.browser.is_some(),
                    Err(_) => true,
                }
            }
            _ => false,
        }
    }
}

async fn launch_browser(executable: &Path, profile_dir: &Path, port: u16, url: &str) -> Result<()> {
    Command::new(executable)
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg("--no-first-run")
        .arg("--disable-background-mode")
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
