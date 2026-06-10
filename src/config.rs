use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub root: PathBuf,
    pub bin_dir: PathBuf,
    pub conf_dir: PathBuf,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub auth_dir: PathBuf,
    pub browser_profiles_dir: PathBuf,
    pub config_file: PathBuf,
    pub state_file: PathBuf,
}

impl RuntimePaths {
    pub fn resolve() -> Result<Self> {
        let cwd = std::env::current_dir().context("resolve current directory")?;
        let root = if cwd.join("conf").exists() && cwd.join("data").exists() {
            cwd
        } else {
            let exe = std::env::current_exe().context("resolve current executable")?;
            exe.parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or(cwd)
        };

        let bin_dir = root.join("bin");
        let conf_dir = root.join("conf");
        let data_dir = root.join("data");
        let logs_dir = root.join("logs");
        let auth_dir = conf_dir.join("auth");
        let browser_profiles_dir = conf_dir.join("browser_profiles");
        let config_file = conf_dir.join("auto_media.toml");
        let state_file = conf_dir.join("state.sqlite");

        Ok(Self {
            root,
            bin_dir,
            conf_dir,
            data_dir,
            logs_dir,
            auth_dir,
            browser_profiles_dir,
            config_file,
            state_file,
        })
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.bin_dir,
            &self.conf_dir,
            &self.data_dir,
            &self.logs_dir,
            &self.auth_dir,
            &self.browser_profiles_dir,
            &self.browser_profiles_dir.join("xhs"),
            &self.browser_profiles_dir.join("zhihu"),
        ] {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub app: AppSection,
    pub scheduler: SchedulerSection,
    pub data: DataSection,
    pub publish: PublishSection,
    pub platforms: PlatformSections,
    pub startup: StartupSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSection {
    pub start_minimized: bool,
    pub single_instance: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerSection {
    pub timezone: String,
    pub sleep_minutes: u64,
    pub cutoff_time: String,
    pub run_immediately_on_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSection {
    pub dir: String,
    pub image_patterns: Vec<String>,
    pub multi_image_policy: MultiImagePolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiImagePolicy {
    FirstByName,
    Newest,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishSection {
    pub title_pattern: String,
    pub fallback_body_text: String,
    pub publish_platforms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSections {
    pub xhs: PlatformSection,
    pub zhihu: PlatformSection,
    #[serde(default = "default_twitter_platform")]
    pub twitter: PlatformSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSection {
    pub enabled: bool,
    pub mode: String,
    pub login_url: String,
    pub write_url: Option<String>,
    pub creator_url: Option<String>,
    pub cdp_port: u16,
}

fn default_twitter_platform() -> PlatformSection {
    PlatformSection {
        enabled: true,
        mode: "cdp".to_string(),
        login_url: "https://x.com/i/flow/login".to_string(),
        creator_url: Some("https://x.com".to_string()),
        write_url: Some("https://x.com/home".to_string()),
        cdp_port: 9225,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupSection {
    pub enabled: bool,
    pub minimize_to_tray_on_autostart: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app: AppSection {
                start_minimized: true,
                single_instance: true,
            },
            scheduler: SchedulerSection {
                timezone: "Asia/Shanghai".to_string(),
                sleep_minutes: 10,
                cutoff_time: "20:00:00".to_string(),
                run_immediately_on_start: true,
            },
            data: DataSection {
                dir: "data".to_string(),
                image_patterns: vec![
                    "{YYYYMMDD}*.jpg".to_string(),
                    "{YYYYMMDD}*.jpeg".to_string(),
                    "{YYYYMMDD}*.png".to_string(),
                    "{YYYYMMDD}*.webp".to_string(),
                    "{YYYY-MM-DD}*.jpg".to_string(),
                    "{YYYY-MM-DD}*.jpeg".to_string(),
                    "{YYYY-MM-DD}*.png".to_string(),
                    "{YYYY-MM-DD}*.webp".to_string(),
                ],
                multi_image_policy: MultiImagePolicy::FirstByName,
            },
            publish: PublishSection {
                title_pattern: "挑战千万美金 - {YYYYMMDD}".to_string(),
                fallback_body_text: "挑战千万美金 - {YYYYMMDD}".to_string(),
                publish_platforms: vec![
                    "xhs".to_string(),
                    "zhihu".to_string(),
                    "twitter".to_string(),
                ],
            },
            platforms: PlatformSections {
                xhs: PlatformSection {
                    enabled: true,
                    mode: "cdp".to_string(),
                    login_url: "https://www.xiaohongshu.com".to_string(),
                    creator_url: Some("https://creator.xiaohongshu.com".to_string()),
                    write_url: Some("https://creator.xiaohongshu.com/publish/publish".to_string()),
                    cdp_port: 9223,
                },
                zhihu: PlatformSection {
                    enabled: true,
                    mode: "cdp".to_string(),
                    login_url: "https://www.zhihu.com/signin".to_string(),
                    creator_url: None,
                    write_url: Some("https://zhuanlan.zhihu.com/write".to_string()),
                    cdp_port: 9224,
                },
                twitter: PlatformSection {
                    enabled: true,
                    mode: "cdp".to_string(),
                    login_url: "https://x.com/i/flow/login".to_string(),
                    creator_url: Some("https://x.com".to_string()),
                    write_url: Some("https://x.com/home".to_string()),
                    cdp_port: 9225,
                },
            },
            startup: StartupSection {
                enabled: true,
                minimize_to_tray_on_autostart: true,
            },
        }
    }
}

pub fn load_or_create(paths: &RuntimePaths) -> Result<AppConfig> {
    if paths.config_file.exists() {
        let text = fs::read_to_string(&paths.config_file)
            .with_context(|| format!("read {}", paths.config_file.display()))?;
        toml::from_str(&text).with_context(|| format!("parse {}", paths.config_file.display()))
    } else {
        let config = AppConfig::default();
        let text = toml::to_string_pretty(&config).context("serialize default config")?;
        fs::write(&paths.config_file, text)
            .with_context(|| format!("write {}", paths.config_file.display()))?;
        Ok(config)
    }
}

pub fn resolve_configured_data_dir(paths: &RuntimePaths, config: &AppConfig) -> PathBuf {
    let data = PathBuf::from(&config.data.dir);
    if data.is_absolute() {
        data
    } else {
        paths.root.join(data)
    }
}
