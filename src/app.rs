use crate::{
    commands,
    config::{load_or_create, AppConfig, RuntimePaths},
    platforms::{CdpPlatformAdapter, Platform, PlatformAdapter, SessionStatus},
    publish::Publisher,
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
            state,
            adapters.clone(),
        ));
        let status = Arc::new(RwLock::new(RuntimeStatus::default()));
        let scheduler = Scheduler::new(config, publisher, status);

        Ok(Self {
            paths,
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
        for platform in [Platform::Xhs, Platform::Zhihu] {
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

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(shared)
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::run_now,
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
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
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

    adapters
}
