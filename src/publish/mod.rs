pub mod image_scanner;
pub mod job;

use crate::{
    config::{resolve_configured_data_dir, AppConfig, RuntimePaths},
    platforms::{Platform, PlatformAdapter, SessionStatus},
    state::{PlatformStatusRecord, StateStore},
};
use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

use self::{image_scanner::ImageScanner, job::PublishJob};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickReport {
    pub target_date: String,
    pub message: String,
    pub image_path: Option<String>,
    pub platform_results: Vec<PlatformRunReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformRunReport {
    pub platform: Platform,
    pub status: String,
    pub message: String,
    pub remote_url: Option<String>,
}

pub struct Publisher {
    config: AppConfig,
    paths: RuntimePaths,
    state: Arc<StateStore>,
    adapters: HashMap<Platform, Arc<dyn PlatformAdapter>>,
}

impl Publisher {
    pub fn new(
        config: AppConfig,
        paths: RuntimePaths,
        state: Arc<StateStore>,
        adapters: HashMap<Platform, Arc<dyn PlatformAdapter>>,
    ) -> Self {
        Self {
            config,
            paths,
            state,
            adapters,
        }
    }

    pub async fn run_for_date(&self, target_date: NaiveDate) -> Result<TickReport> {
        if self.state.has_success_for_target_date(target_date)? {
            return Ok(TickReport {
                target_date: target_date.to_string(),
                message: "目标日期已成功发送，今日跳过检测".to_string(),
                image_path: None,
                platform_results: Vec::new(),
            });
        }

        let data_dir = resolve_configured_data_dir(&self.paths, &self.config);
        let scanner = ImageScanner::new(
            data_dir,
            self.config.data.image_patterns.clone(),
            self.config.data.multi_image_policy,
        );

        let Some(image) = scanner
            .find_target_image(target_date)
            .with_context(|| format!("scan image for {target_date}"))?
        else {
            return Ok(TickReport {
                target_date: target_date.to_string(),
                message: "未找到目标日期图片".to_string(),
                image_path: None,
                platform_results: Vec::new(),
            });
        };

        let job = PublishJob::from_image(
            target_date,
            image.path,
            &self.config.publish.title_pattern,
            &self.config.publish.fallback_body_text,
        )?;
        self.state.upsert_job(&job)?;

        let mut platform_results = Vec::new();
        for platform_name in &self.config.publish.publish_platforms {
            let platform: Platform = platform_name.parse()?;
            let Some(adapter) = self.adapters.get(&platform) else {
                platform_results.push(PlatformRunReport {
                    platform,
                    status: "failed".to_string(),
                    message: "平台适配器未启用".to_string(),
                    remote_url: None,
                });
                continue;
            };
            debug_assert_eq!(adapter.platform(), platform);

            if self.state.is_platform_success(&job.job_id, platform)? {
                platform_results.push(PlatformRunReport {
                    platform,
                    status: "skipped_duplicate".to_string(),
                    message: "该平台已成功发布过，跳过重复发布".to_string(),
                    remote_url: None,
                });
                continue;
            }

            self.state
                .mark_platform(&job.job_id, platform, "publishing", None, None)?;

            match adapter.validate_session().await? {
                SessionStatus::Valid { .. } => {}
                status => {
                    let message = format!("登录状态需要人工确认: {status:?}");
                    let _ = adapter.login_interactive().await;
                    self.state.mark_platform(
                        &job.job_id,
                        platform,
                        "auth_required",
                        None,
                        Some(&message),
                    )?;
                    platform_results.push(PlatformRunReport {
                        platform,
                        status: "auth_required".to_string(),
                        message,
                        remote_url: None,
                    });
                    continue;
                }
            }

            match adapter.publish_image_article(&job).await {
                Ok(result) => {
                    self.state.mark_platform(
                        &job.job_id,
                        platform,
                        "success",
                        result.remote_url.as_deref(),
                        None,
                    )?;
                    platform_results.push(PlatformRunReport {
                        platform,
                        status: "success".to_string(),
                        message: result.message,
                        remote_url: result.remote_url,
                    });
                }
                Err(error) => {
                    let message = error.to_string();
                    self.state.mark_platform(
                        &job.job_id,
                        platform,
                        "failed",
                        None,
                        Some(&message),
                    )?;
                    platform_results.push(PlatformRunReport {
                        platform,
                        status: "failed".to_string(),
                        message,
                        remote_url: None,
                    });
                }
            }
        }

        Ok(TickReport {
            target_date: target_date.to_string(),
            message: "扫描完成".to_string(),
            image_path: Some(job.image_path.display().to_string()),
            platform_results,
        })
    }

    pub fn recent_statuses(&self) -> Result<Vec<PlatformStatusRecord>> {
        self.state.recent_platform_statuses(30)
    }
}
