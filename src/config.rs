use crate::platforms::Platform;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Every platform shares ONE Chrome profile + debug port, so manual publishing
/// opens a single window with one tab per platform (not a process per profile).
pub const SHARED_CDP_PORT: u16 = 9222;
/// Sub-directory name of the shared Chrome profile under `browser_profiles`.
pub const SHARED_PROFILE_NAME: &str = "shared";

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub root: PathBuf,
    pub bin_dir: PathBuf,
    pub conf_dir: PathBuf,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub auth_dir: PathBuf,
    pub browser_profiles_dir: PathBuf,
    pub shared_profile_dir: PathBuf,
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
        let shared_profile_dir = browser_profiles_dir.join(SHARED_PROFILE_NAME);
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
            shared_profile_dir,
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
            &self.shared_profile_dir,
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
    #[serde(default = "default_sleep_minutes")]
    pub sleep_minutes: u64,
    #[serde(default = "default_cutoff_time")]
    pub cutoff_time: String,
    #[serde(default = "default_active_start_time")]
    pub active_start_time: String,
    #[serde(default = "default_active_end_time")]
    pub active_end_time: String,
    #[serde(default = "default_active_sleep_minutes")]
    pub active_sleep_minutes: u64,
    pub run_immediately_on_start: bool,
}

fn default_sleep_minutes() -> u64 {
    10
}

fn default_cutoff_time() -> String {
    "20:00:00".to_string()
}

fn default_active_start_time() -> String {
    "20:00:00".to_string()
}

fn default_active_end_time() -> String {
    "21:00:00".to_string()
}

fn default_active_sleep_minutes() -> u64 {
    3
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
    #[serde(default = "default_tags")]
    pub tags: Vec<String>,
    pub publish_platforms: Vec<String>,
}

fn default_tags() -> Vec<String> {
    [
        "#投资理财",
        "#投资",
        "#理财",
        "#美股",
        "#交易员",
        "#期权",
        "#期权交易",
        "#美股期权",
        "#投资挑战",
        "#实盘记录",
        "#账户盈利",
        "#交易复盘",
        "富途",
        "盈透",
        "老虎",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSections {
    pub xhs: PlatformSection,
    pub zhihu: PlatformSection,
    #[serde(default = "default_twitter_platform")]
    pub twitter: PlatformSection,
    #[serde(default = "default_xueqiu_platform")]
    pub xueqiu: PlatformSection,
    #[serde(default = "default_douyin_platform")]
    pub douyin: PlatformSection,
}

impl PlatformSections {
    pub fn section_for(&self, platform: Platform) -> &PlatformSection {
        match platform {
            Platform::Xhs => &self.xhs,
            Platform::Zhihu => &self.zhihu,
            Platform::Twitter => &self.twitter,
            Platform::Xueqiu => &self.xueqiu,
            Platform::Douyin => &self.douyin,
        }
    }

    pub fn section_for_mut(&mut self, platform: Platform) -> &mut PlatformSection {
        match platform {
            Platform::Xhs => &mut self.xhs,
            Platform::Zhihu => &mut self.zhihu,
            Platform::Twitter => &mut self.twitter,
            Platform::Xueqiu => &mut self.xueqiu,
            Platform::Douyin => &mut self.douyin,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSection {
    pub enabled: bool,
    pub mode: String,
    pub login_url: String,
    pub write_url: Option<String>,
    pub creator_url: Option<String>,
    pub cdp_port: u16,
    /// Text stamped onto images before upload. Absent → use the platform's
    /// built-in default; present-but-empty → watermarking disabled for it.
    #[serde(default)]
    pub watermark: Option<String>,
}

fn default_twitter_platform() -> PlatformSection {
    PlatformSection {
        enabled: true,
        mode: "cdp".to_string(),
        login_url: "https://x.com/i/flow/login".to_string(),
        creator_url: Some("https://x.com".to_string()),
        write_url: Some("https://x.com/home".to_string()),
        cdp_port: 9225,
        watermark: Some("https://blog.xiedeacc.com".to_string()),
    }
}

fn default_xueqiu_platform() -> PlatformSection {
    PlatformSection {
        enabled: true,
        mode: "cdp".to_string(),
        login_url: "https://xueqiu.com".to_string(),
        creator_url: Some("https://xueqiu.com".to_string()),
        write_url: Some("https://xueqiu.com".to_string()),
        cdp_port: 9226,
        watermark: Some("https://blog.xiedeacc.com".to_string()),
    }
}

fn default_douyin_platform() -> PlatformSection {
    PlatformSection {
        enabled: true,
        mode: "cdp".to_string(),
        login_url: "https://www.douyin.com".to_string(),
        creator_url: Some("https://creator.douyin.com".to_string()),
        write_url: Some(
            "https://creator.douyin.com/creator-micro/content/upload?default-tab=3".to_string(),
        ),
        cdp_port: 9227,
        watermark: Some("xiedeacc".to_string()),
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
                cutoff_time: "21:00:00".to_string(),
                active_start_time: "20:00:00".to_string(),
                active_end_time: "21:00:00".to_string(),
                active_sleep_minutes: 3,
                run_immediately_on_start: true,
            },
            data: DataSection {
                dir: "data".to_string(),
                image_patterns: vec![
                    "*{YYYYMMDDHHMMSS}*.jpg".to_string(),
                    "*{YYYYMMDDHHMMSS}*.jpeg".to_string(),
                    "*{YYYYMMDDHHMMSS}*.png".to_string(),
                    "*{YYYYMMDDHHMMSS}*.webp".to_string(),
                    "*{YYYYMMDD}_{HH}{mm}{SS}*.jpg".to_string(),
                    "*{YYYYMMDD}_{HH}{mm}{SS}*.jpeg".to_string(),
                    "*{YYYYMMDD}_{HH}{mm}{SS}*.png".to_string(),
                    "*{YYYYMMDD}_{HH}{mm}{SS}*.webp".to_string(),
                    "*{YYYY-MM-DD}_{HH}-{mm}-{SS}*.jpg".to_string(),
                    "*{YYYY-MM-DD}_{HH}-{mm}-{SS}*.jpeg".to_string(),
                    "*{YYYY-MM-DD}_{HH}-{mm}-{SS}*.png".to_string(),
                    "*{YYYY-MM-DD}_{HH}-{mm}-{SS}*.webp".to_string(),
                    "*{YYYY}年{MM}月{DD}日{HH}时{mm}分{SS}秒*.jpg".to_string(),
                    "*{YYYY}年{MM}月{DD}日{HH}时{mm}分{SS}秒*.jpeg".to_string(),
                    "*{YYYY}年{MM}月{DD}日{HH}时{mm}分{SS}秒*.png".to_string(),
                    "*{YYYY}年{MM}月{DD}日{HH}时{mm}分{SS}秒*.webp".to_string(),
                ],
                multi_image_policy: MultiImagePolicy::Newest,
            },
            publish: PublishSection {
                title_pattern: "挑战千万美金 - {YYYYMMDD}".to_string(),
                fallback_body_text: "挑战千万美金 - {YYYYMMDD}".to_string(),
                tags: default_tags(),
                publish_platforms: vec![
                    "xhs".to_string(),
                    "zhihu".to_string(),
                    "twitter".to_string(),
                    "xueqiu".to_string(),
                    "douyin".to_string(),
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
                    watermark: Some("xiedeacc".to_string()),
                },
                zhihu: PlatformSection {
                    enabled: true,
                    mode: "cdp".to_string(),
                    login_url: "https://www.zhihu.com/signin".to_string(),
                    creator_url: None,
                    write_url: Some("https://zhuanlan.zhihu.com/write".to_string()),
                    cdp_port: 9224,
                    watermark: Some("https://blog.xiedeacc.com".to_string()),
                },
                twitter: PlatformSection {
                    enabled: true,
                    mode: "cdp".to_string(),
                    login_url: "https://x.com/i/flow/login".to_string(),
                    creator_url: Some("https://x.com".to_string()),
                    write_url: Some("https://x.com/home".to_string()),
                    cdp_port: 9225,
                    watermark: Some("https://blog.xiedeacc.com".to_string()),
                },
                xueqiu: default_xueqiu_platform(),
                douyin: default_douyin_platform(),
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

/// Persist a single platform's preferred backend mode ("cdp" or "api") to the
/// config file, preserving all other current on-disk settings.
pub fn update_platform_mode(paths: &RuntimePaths, platform: Platform, mode: &str) -> Result<()> {
    let mut config = load_or_create(paths)?;
    config.platforms.section_for_mut(platform).mode = mode.to_string();
    let text = toml::to_string_pretty(&config).context("serialize config")?;
    fs::write(&paths.config_file, text)
        .with_context(|| format!("write {}", paths.config_file.display()))
}

pub fn resolve_configured_data_dir(paths: &RuntimePaths, config: &AppConfig) -> PathBuf {
    let data = PathBuf::from(&config.data.dir);
    if data.is_absolute() {
        data
    } else {
        paths.root.join(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A legacy config written before Xueqiu/Douyin existed must still parse, with
    /// the two new platform sections supplied by their serde defaults.
    #[test]
    fn legacy_three_platform_config_parses_with_new_platform_defaults() {
        let legacy = r#"
[app]
start_minimized = true
single_instance = true

[scheduler]
timezone = "Asia/Shanghai"
run_immediately_on_start = true

[data]
dir = "data"
image_patterns = ["*{YYYYMMDD}*.jpg"]
multi_image_policy = "newest"

[publish]
title_pattern = "x"
fallback_body_text = "x"
publish_platforms = ["xhs", "zhihu", "twitter"]

[platforms.xhs]
enabled = true
mode = "cdp"
login_url = "https://www.xiaohongshu.com"
write_url = "https://creator.xiaohongshu.com/publish/publish"
cdp_port = 9223

[platforms.zhihu]
enabled = true
mode = "cdp"
login_url = "https://www.zhihu.com/signin"
cdp_port = 9224

[platforms.twitter]
enabled = true
mode = "cdp"
login_url = "https://x.com/i/flow/login"
cdp_port = 9225

[startup]
enabled = true
minimize_to_tray_on_autostart = true
"#;

        let config: AppConfig = toml::from_str(legacy).expect("legacy config parses");
        assert_eq!(config.platforms.section_for(Platform::Xueqiu).cdp_port, 9226);
        assert_eq!(config.platforms.section_for(Platform::Douyin).cdp_port, 9227);
        assert!(config.platforms.douyin.enabled);
        // The legacy scheduled platform list is preserved untouched.
        assert_eq!(
            config.publish.publish_platforms,
            vec!["xhs", "zhihu", "twitter"]
        );
    }
}
