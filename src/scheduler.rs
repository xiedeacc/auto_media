use crate::{config::AppConfig, publish::Publisher, state::PlatformStatusRecord};
use anyhow::{Context, Result};
use chrono::{Days, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::{
    sync::RwLock,
    time::{sleep, Duration},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub paused: bool,
    pub state: String,
    pub last_tick: Option<String>,
    pub next_wakeup: Option<String>,
    pub last_message: String,
    pub last_report: Option<crate::publish::TickReport>,
    pub recent_platform_statuses: Vec<PlatformStatusRecord>,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            paused: false,
            state: "idle".to_string(),
            last_tick: None,
            next_wakeup: None,
            last_message: "等待调度".to_string(),
            last_report: None,
            recent_platform_statuses: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Scheduler {
    config: AppConfig,
    publisher: Arc<Publisher>,
    status: Arc<RwLock<RuntimeStatus>>,
}

impl Scheduler {
    pub fn new(
        config: AppConfig,
        publisher: Arc<Publisher>,
        status: Arc<RwLock<RuntimeStatus>>,
    ) -> Self {
        Self {
            config,
            publisher,
            status,
        }
    }

    pub async fn run_forever(self) {
        if self.config.scheduler.run_immediately_on_start {
            if let Err(error) = self.tick("startup").await {
                tracing::error!(?error, "startup tick failed");
            }
        }

        loop {
            let sleep_duration =
                Duration::from_secs(self.config.scheduler.sleep_minutes.saturating_mul(60));
            {
                let mut status = self.status.write().await;
                status.next_wakeup = Some(
                    (chrono::Utc::now()
                        + chrono::Duration::from_std(sleep_duration).unwrap_or_default())
                    .to_rfc3339(),
                );
            }
            sleep(sleep_duration).await;

            let paused = self.status.read().await.paused;
            if paused {
                continue;
            }

            if let Err(error) = self.tick("schedule").await {
                tracing::error!(?error, "scheduled tick failed");
                let mut status = self.status.write().await;
                status.state = "error".to_string();
                status.last_message = error.to_string();
                status.last_tick = Some(Utc::now().to_rfc3339());
            }
        }
    }

    pub async fn tick(&self, reason: &str) -> Result<()> {
        let tz: Tz = self
            .config
            .scheduler
            .timezone
            .parse()
            .unwrap_or(chrono_tz::Asia::Shanghai);
        let now = Utc::now().with_timezone(&tz);
        let cutoff = NaiveTime::parse_from_str(&self.config.scheduler.cutoff_time, "%H:%M:%S")
            .context("parse cutoff_time")?;

        {
            let mut status = self.status.write().await;
            status.state = "scanning".to_string();
            status.last_tick = Some(now.to_rfc3339());
            status.last_message = format!("开始检测: {reason}");
        }

        if now.time() >= cutoff {
            let mut status = self.status.write().await;
            status.state = "sleeping".to_string();
            status.last_message = "已过 20:00，跳过扫描".to_string();
            status.recent_platform_statuses = self.publisher.recent_statuses().unwrap_or_default();
            return Ok(());
        }

        let target_date = now
            .date_naive()
            .checked_sub_days(Days::new(1))
            .context("calculate target date")?;
        let report = self.publisher.run_for_date(target_date).await?;

        let mut status = self.status.write().await;
        status.state = "sleeping".to_string();
        status.last_message = report.message.clone();
        status.last_report = Some(report);
        status.recent_platform_statuses = self.publisher.recent_statuses().unwrap_or_default();
        Ok(())
    }

    pub async fn set_paused(&self, paused: bool) {
        let mut status = self.status.write().await;
        status.paused = paused;
        status.state = if paused { "paused" } else { "idle" }.to_string();
        status.last_message = if paused {
            "已暂停".to_string()
        } else {
            "已恢复".to_string()
        };
    }

    pub async fn status(&self) -> RuntimeStatus {
        self.status.read().await.clone()
    }

    pub async fn set_message_with_records(
        &self,
        state_name: &str,
        message: String,
        records: Vec<PlatformStatusRecord>,
    ) {
        let mut status = self.status.write().await;
        status.state = state_name.to_string();
        status.last_tick = Some(Utc::now().to_rfc3339());
        status.last_message = message;
        status.recent_platform_statuses = records;
    }
}
