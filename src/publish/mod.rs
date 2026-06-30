pub mod image_scanner;
pub mod job;

use crate::{
    config::{resolve_configured_data_dir, AppConfig, RuntimePaths},
    platforms::{Platform, PlatformAdapter, SessionStatus},
    state::{PlatformStatusRecord, StateStore},
};
use anyhow::{Context, Result};
use chrono::NaiveDate;
use futures_util::{stream::FuturesUnordered, StreamExt};
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
            &self.config.publish.tags,
        )?;
        self.state.upsert_job(&job)?;

        let mut platform_results = Vec::new();
        let mut publish_tasks = FuturesUnordered::new();
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

            let state = self.state.clone();
            let adapter = adapter.clone();
            let job = job.clone();
            publish_tasks.push(async move {
                let result: Result<PlatformRunReport> = async {
                    match adapter.validate_session().await {
                        Ok(SessionStatus::Valid { .. }) => {}
                        Ok(status) => {
                            let message = format!("登录状态需要人工确认: {status:?}");
                            let _ = adapter.login_interactive().await;
                            state.mark_platform(
                                &job.job_id,
                                platform,
                                "auth_required",
                                None,
                                Some(&message),
                            )?;
                            return Ok(PlatformRunReport {
                                platform,
                                status: "auth_required".to_string(),
                                message,
                                remote_url: None,
                            });
                        }
                        Err(error) => {
                            let message = format!("登录态检测失败：{error:#}");
                            state.mark_platform(
                                &job.job_id,
                                platform,
                                "failed",
                                None,
                                Some(&message),
                            )?;
                            return Ok(PlatformRunReport {
                                platform,
                                status: "failed".to_string(),
                                message,
                                remote_url: None,
                            });
                        }
                    }

                    match adapter.publish_image_article(&job).await {
                        Ok(result) => {
                            state.mark_platform(
                                &job.job_id,
                                platform,
                                "success",
                                result.remote_url.as_deref(),
                                None,
                            )?;
                            Ok(PlatformRunReport {
                                platform,
                                status: "success".to_string(),
                                message: result.message,
                                remote_url: result.remote_url,
                            })
                        }
                        Err(error) => {
                            let message = format!("{error:#}");
                            state.mark_platform(
                                &job.job_id,
                                platform,
                                "failed",
                                None,
                                Some(&message),
                            )?;
                            Ok(PlatformRunReport {
                                platform,
                                status: "failed".to_string(),
                                message,
                                remote_url: None,
                            })
                        }
                    }
                }
                .await;
                (platform, result)
            });
        }

        while let Some((platform, result)) = publish_tasks.next().await {
            match result {
                Ok(report) => platform_results.push(report),
                Err(error) => platform_results.push(PlatformRunReport {
                    platform,
                    status: "failed".to_string(),
                    message: format!("发布任务异常：{error:#}"),
                    remote_url: None,
                }),
            }
        }

        platform_results.sort_by_key(|report| {
            self.config
                .publish
                .publish_platforms
                .iter()
                .position(|platform| platform == report.platform.as_str())
                .unwrap_or(usize::MAX)
        });

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
