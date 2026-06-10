use crate::{
    browser::cdp::CdpBrowser,
    commands,
    config::{load_or_create, AppConfig, RuntimePaths},
    platforms::{CdpPlatformAdapter, Platform, PlatformAdapter, SessionStatus},
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

    pub async fn status(&self) -> RuntimeStatus {
        self.scheduler.status().await
    }

    pub async fn platform_sessions(&self) -> Vec<PlatformSessionSummary> {
        let mut sessions = Vec::new();
        for platform in [Platform::Xhs, Platform::Zhihu, Platform::Twitter] {
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

    pub async fn manual_publish(
        &self,
        title: String,
        text: String,
        image_paths: Vec<String>,
        platforms: Option<Vec<String>>,
    ) -> Result<String> {
        let image_paths = image_paths.into_iter().map(Into::into).collect::<Vec<_>>();
        let job = ManualPublishJob::new(title, text, image_paths)?;
        let mut messages = Vec::new();
        let platform_names = platforms
            .filter(|platforms| !platforms.is_empty())
            .unwrap_or_else(|| self.config.publish.publish_platforms.clone());

        for platform_name in &platform_names {
            let platform: Platform = platform_name.parse()?;
            let Some(adapter) = self.adapters.get(&platform) else {
                messages.push(format!("{platform}: 平台适配器未启用"));
                continue;
            };

            match adapter.validate_session().await? {
                SessionStatus::Valid { .. } | SessionStatus::RiskVerificationRequired => {}
                status => {
                    let message = status.label().to_string();
                    self.state.mark_platform(
                        &job.job_id,
                        platform,
                        "failed",
                        None,
                        Some(&message),
                    )?;
                    messages.push(format!("{platform}: {message}"));
                    continue;
                }
            }

            match adapter.publish_manual_article(&job).await {
                Ok(result) => {
                    self.state.mark_platform(
                        &job.job_id,
                        platform,
                        "success",
                        result.remote_url.as_deref(),
                        None,
                    )?;
                    messages.push(format!("{platform}: {}", result.message));
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    self.state.mark_platform(
                        &job.job_id,
                        platform,
                        "failed",
                        None,
                        Some(&message),
                    )?;
                    messages.push(format!("{platform}: {message}"));
                }
            }
        }

        let message = messages.join("\n");
        let records = self.state.recent_platform_statuses(30)?;
        self.scheduler
            .set_message_with_records("manual_publish", message.clone(), records)
            .await;
        Ok(message)
    }

    pub fn scheduler(&self) -> Scheduler {
        self.scheduler.clone()
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

    pub async fn close_browser_tabs(&self) {
        let browser = CdpBrowser::default();
        for (platform, enabled, port) in [
            (
                Platform::Xhs,
                self.config.platforms.xhs.enabled,
                self.config.platforms.xhs.cdp_port,
            ),
            (
                Platform::Zhihu,
                self.config.platforms.zhihu.enabled,
                self.config.platforms.zhihu.cdp_port,
            ),
            (
                Platform::Twitter,
                self.config.platforms.twitter.enabled,
                self.config.platforms.twitter.cdp_port,
            ),
        ] {
            if !enabled {
                continue;
            }

            match browser.close_all_tabs(port).await {
                Ok(count) if count > 0 => tracing::info!(
                    platform = %platform,
                    port,
                    count,
                    "closed browser tabs"
                ),
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    platform = %platform,
                    port,
                    error = %error,
                    "failed to close browser tabs"
                ),
            }
        }
    }
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
            commands::manual_publish,
            commands::get_logs,
            commands::set_paused,
            commands::login_platform,
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

            let scheduler = controller.scheduler();
            tauri::async_runtime::spawn(async move {
                scheduler.run_forever().await;
            });

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

    if config.platforms.xhs.enabled {
        adapters.insert(
            Platform::Xhs,
            Arc::new(CdpPlatformAdapter::new(
                Platform::Xhs,
                config.platforms.xhs.clone(),
                paths.browser_profiles_dir.join("xhs"),
                paths.auth_dir.join("xhs.cookies.enc"),
            )),
        );
    }

    if config.platforms.zhihu.enabled {
        adapters.insert(
            Platform::Zhihu,
            Arc::new(CdpPlatformAdapter::new(
                Platform::Zhihu,
                config.platforms.zhihu.clone(),
                paths.browser_profiles_dir.join("zhihu"),
                paths.auth_dir.join("zhihu.cookies.enc"),
            )),
        );
    }

    if config.platforms.twitter.enabled {
        adapters.insert(
            Platform::Twitter,
            Arc::new(CdpPlatformAdapter::new(
                Platform::Twitter,
                config.platforms.twitter.clone(),
                paths.browser_profiles_dir.join("twitter"),
                paths.auth_dir.join("twitter.cookies.enc"),
            )),
        );
    }

    adapters
}
