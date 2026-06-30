use crate::{
    browser::cdp::CdpBrowser,
    commands,
    config::{load_or_create, AppConfig, RuntimePaths},
    platforms::{MediaPlatformAdapter, Platform, PlatformAdapter, SessionStatus},
    publish::{job::ManualPublishJob, Publisher},
    scheduler::{RuntimeStatus, Scheduler},
    state::StateStore,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{collections::HashMap, sync::Arc};
use tauri::WindowEvent;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct SharedState {
    pub controller: Arc<AppController>,
}

pub struct AppController {
    pub paths: RuntimePaths,
    config: AppConfig,
    state: Arc<StateStore>,
    scheduler: Scheduler,
    adapters: HashMap<Platform, Arc<dyn PlatformAdapter>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PathSummary {
    pub root: String,
    pub bin: String,
    pub conf: String,
    pub data: String,
    pub logs: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlatformSessionSummary {
    pub platform: String,
    pub status: SessionStatus,
    pub label: String,
    /// Preferred backend: "cdp" or "api".
    pub mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManualPublishProgress {
    pub platform: Option<String>,
    pub status: String,
    pub message: String,
}

impl AppController {
    pub fn new(paths: RuntimePaths, config: AppConfig) -> Result<Self> {
        let state = Arc::new(StateStore::open(&paths.state_file)?);
        let adapters = build_adapters(&paths, &config);
        let publisher = Arc::new(Publisher::new(
            config.clone(),
            paths.clone(),
            state.clone(),
            adapters.clone(),
        ));
        let status = Arc::new(RwLock::new(RuntimeStatus::default()));
        let scheduler = Scheduler::new(config.clone(), publisher, status);

        Ok(Self {
            paths,
            config,
            state,
            scheduler,
            adapters,
        })
    }

    pub async fn run_now(&self, reason: &str) -> Result<()> {
        self.scheduler.tick(reason).await
    }

    pub async fn set_paused(&self, paused: bool) {
        self.scheduler.set_paused(paused).await;
    }

    pub async fn clear_records(&self) -> Result<()> {
        self.state.clear_publish_records()?;
        self.scheduler
            .set_message_with_records("idle", "最近任务已清空".to_string(), Vec::new())
            .await;
        Ok(())
    }

    pub async fn status(&self) -> RuntimeStatus {
        self.scheduler.status().await
    }

    pub async fn platform_sessions(&self) -> Vec<PlatformSessionSummary> {
        let mut sessions = Vec::new();
        for platform in Platform::ALL {
            if let Some(adapter) = self.adapters.get(&platform) {
                let status = adapter.validate_session().await.unwrap_or_else(|error| {
                    SessionStatus::NetworkError {
                        message: error.to_string(),
                    }
                });
                sessions.push(PlatformSessionSummary {
                    platform: platform.as_str().to_string(),
                    label: status.label().to_string(),
                    status,
                    mode: if adapter.prefer_cdp() { "cdp" } else { "api" }.to_string(),
                });
            }
        }
        sessions
    }

    pub async fn login_platform(&self, platform: Platform) -> Result<()> {
        let adapter = self
            .adapters
            .get(&platform)
            .with_context(|| format!("adapter not enabled: {platform}"))?;
        adapter.login_interactive().await?;
        Ok(())
    }

    pub async fn manual_publish_with_progress<F>(
        &self,
        title: String,
        text: String,
        tags: Option<String>,
        image_paths: Vec<String>,
        platforms: Option<Vec<String>>,
        mut progress: F,
    ) -> Result<String>
    where
        F: FnMut(ManualPublishProgress),
    {
        let image_paths = image_paths.into_iter().map(Into::into).collect::<Vec<_>>();
        let tags = match tags {
            Some(text) => parse_tags(&text),
            None => self.config.publish.tags.clone(),
        };
        let job = ManualPublishJob::new(title, text, image_paths, &tags)?;
        let mut messages = Vec::new();
        let platform_names = platforms
            .filter(|platforms| !platforms.is_empty())
            .unwrap_or_else(|| self.config.publish.publish_platforms.clone());

        progress(ManualPublishProgress {
            platform: None,
            status: "start".to_string(),
            message: format!("准备发送到 {} 个平台", platform_names.len()),
        });

        let mut reports: Vec<(Platform, String)> = Vec::new();
        // One shared window: open it, then publish to each platform's tab in turn
        // (sequential = the driven tab is foreground, so trusted clicks always
        // land and it reads like a person doing one platform at a time).
        self.ensure_browser().await;
        for platform_name in &platform_names {
            let platform: Platform = platform_name.parse()?;
            let Some(adapter) = self.adapters.get(&platform) else {
                let message = "平台适配器未启用".to_string();
                progress(ManualPublishProgress {
                    platform: Some(platform.to_string()),
                    status: "failed".to_string(),
                    message: message.clone(),
                });
                reports.push((platform, format!("{platform}: {message}")));
                continue;
            };

            progress(ManualPublishProgress {
                platform: Some(platform.to_string()),
                status: "publishing".to_string(),
                message: format!("正在发送到 {platform}"),
            });

            let adapter = adapter.clone();
            let state = self.state.clone();
            let job = job.clone();
            let result: Result<(String, String)> = async {
                    let session_status = match adapter.validate_session().await {
                        Ok(status) => status,
                        Err(error) => {
                            let message = format!("登录态检测失败：{error:#}");
                            tracing::warn!(
                                platform = %platform,
                                error = %error,
                                "manual publish session validation failed"
                            );
                            state.mark_platform(
                                &job.job_id,
                                platform,
                                "failed",
                                None,
                                Some(&message),
                            )?;
                            return Ok(("failed".to_string(), message));
                        }
                    };

                    match session_status {
                        SessionStatus::Valid { .. } | SessionStatus::RiskVerificationRequired => {}
                        status => {
                            let message = status.label().to_string();
                            tracing::warn!(
                                platform = %platform,
                                session_status = ?status,
                                "manual publish session invalid"
                            );
                            state.mark_platform(
                                &job.job_id,
                                platform,
                                "failed",
                                None,
                                Some(&message),
                            )?;
                            return Ok(("failed".to_string(), message));
                        }
                    }

                    match adapter.publish_manual_article(&job).await {
                        Ok(result) => {
                            state.mark_platform(
                                &job.job_id,
                                platform,
                                "success",
                                result.remote_url.as_deref(),
                                None,
                            )?;
                            Ok(("success".to_string(), result.message))
                        }
                        Err(error) => {
                            let message = format!("{error:#}");
                            tracing::warn!(
                                platform = %platform,
                                error = %message,
                                "manual publish platform failed"
                            );
                            state.mark_platform(
                                &job.job_id,
                                platform,
                                "failed",
                                None,
                                Some(&message),
                            )?;
                            Ok(("failed".to_string(), message))
                        }
                    }
                }
                .await;
            let (status, message) = match result {
                Ok(result) => result,
                Err(error) => ("failed".to_string(), format!("{error:#}")),
            };
            progress(ManualPublishProgress {
                platform: Some(platform.to_string()),
                status: status.clone(),
                message: message.clone(),
            });
            reports.push((platform, format!("{platform}: {message}")));
        }

        // Publishing done — close the shared browser, unless a tab needs the user
        // to finish a verification (SMS/captcha), in which case keep it open.
        let kept_open = !self.finish_browser_after_publish().await;
        if kept_open {
            messages.push("⚠ 检测到平台需要人工验证（如短信验证码），已保留浏览器窗口，请在窗口中完成验证后手动关闭".to_string());
        }

        reports.sort_by_key(|(platform, _)| {
            platform_names
                .iter()
                .position(|name| name == platform.as_str())
                .unwrap_or(usize::MAX)
        });
        messages.extend(reports.into_iter().map(|(_, message)| message));

        let message = messages.join("\n");
        let records = self.state.recent_platform_statuses(30)?;
        self.scheduler
            .set_message_with_records("manual_publish", message.clone(), records)
            .await;
        progress(ManualPublishProgress {
            platform: None,
            status: "done".to_string(),
            message: "手动发文已处理，详细内容见日志".to_string(),
        });
        Ok(message)
    }

    pub fn set_platform_mode(&self, platform: Platform, prefer_cdp: bool) -> Result<()> {
        if let Some(adapter) = self.adapters.get(&platform) {
            adapter.set_prefer_cdp(prefer_cdp);
        }
        let mode = if prefer_cdp { "cdp" } else { "api" };
        crate::config::update_platform_mode(&self.paths, platform, mode)
    }

    pub fn path_summary(&self) -> PathSummary {
        PathSummary {
            root: self.paths.root.display().to_string(),
            bin: self.paths.bin_dir.display().to_string(),
            conf: self.paths.conf_dir.display().to_string(),
            data: self.paths.data_dir.display().to_string(),
            logs: self.paths.logs_dir.display().to_string(),
        }
    }

    pub fn publish_tags(&self) -> Vec<String> {
        self.config.publish.tags.clone()
    }

    pub fn publish_title_pattern(&self) -> String {
        self.config.publish.title_pattern.clone()
    }

    /// Launch the single shared Chrome window once, before a publish fans out so
    /// the per-platform tabs land in one window (and don't race to launch it).
    pub async fn ensure_browser(&self) {
        if let Err(error) = CdpBrowser
            .ensure_running(&self.paths.shared_profile_dir, crate::config::SHARED_CDP_PORT)
            .await
        {
            tracing::warn!(error = %error, "failed to ensure shared browser running");
        }
    }

    pub async fn close_browser_tabs(&self) {
        let port = crate::config::SHARED_CDP_PORT;
        match CdpBrowser.close_browser(port).await {
            Ok(()) => tracing::info!(port, "closed shared browser"),
            Err(error) => tracing::warn!(port, error = %error, "failed to close shared browser"),
        }
    }

    /// Close the shared browser after a publish — unless a tab is showing a
    /// verification the user must complete (SMS/captcha/scan), in which case keep
    /// the window open so they can finish it. Returns whether it was closed.
    pub async fn finish_browser_after_publish(&self) -> bool {
        let port = crate::config::SHARED_CDP_PORT;
        if CdpBrowser.has_pending_intervention(port).await {
            tracing::warn!(port, "verification prompt detected; keeping browser open");
            return false;
        }
        self.close_browser_tabs().await;
        true
    }
}

fn parse_tags(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToString::to_string)
        .collect()
}

pub fn run() -> Result<()> {
    let paths = RuntimePaths::resolve()?;
    paths.ensure()?;
    let config = load_or_create(&paths)?;
    let _log_guard = crate::logging::init(&paths.logs_dir)?;
    tracing::info!(root = %paths.root.display(), "auto_media starting");

    let controller = Arc::new(AppController::new(paths, config.clone())?);
    let shared = SharedState {
        controller: controller.clone(),
    };
    let launched_by_autostart = crate::startup::is_autostart_launch();

    let close_controller = controller.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(shared)
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::run_now,
            commands::select_images,
            commands::save_pasted_image,
            commands::read_image_preview,
            commands::manual_publish,
            commands::get_logs,
            commands::clear_records,
            commands::set_paused,
            commands::login_platform,
            commands::set_platform_mode,
            commands::set_autostart,
            commands::open_dir,
        ])
        .setup(move |app| {
            crate::tray::setup(app)?;

            if config.startup.enabled {
                let _ = crate::startup::set_autostart(app.handle(), true);
            }

            if launched_by_autostart && config.startup.minimize_to_tray_on_autostart {
                crate::startup::hide_main_window(app.handle());
            }

            // Automatic detection/publishing is disabled; publishing is manual only.
            let _ = &controller;

            Ok(())
        })
        .on_window_event(move |window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let controller = close_controller.clone();
                tauri::async_runtime::spawn(async move {
                    controller.close_browser_tabs().await;
                });
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())?;

    Ok(())
}

fn build_adapters(
    paths: &RuntimePaths,
    config: &AppConfig,
) -> HashMap<Platform, Arc<dyn PlatformAdapter>> {
    let mut adapters: HashMap<Platform, Arc<dyn PlatformAdapter>> = HashMap::new();

    for platform in Platform::ALL {
        let section = config.platforms.section_for(platform);
        if !section.enabled {
            continue;
        }
        // All platforms share one Chrome profile + port → one window, tabs per
        // platform. The per-platform login_url/write_url still differ.
        let mut section = section.clone();
        section.cdp_port = crate::config::SHARED_CDP_PORT;
        adapters.insert(
            platform,
            Arc::new(MediaPlatformAdapter::new(
                platform,
                section,
                paths.shared_profile_dir.clone(),
                paths
                    .auth_dir
                    .join(format!("{}.cookies.enc", platform.as_str())),
                paths.conf_dir.join("topic_cache.json"),
            )) as Arc<dyn PlatformAdapter>,
        );
    }

    adapters
}
