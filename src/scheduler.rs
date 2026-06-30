use crate::{config::AppConfig, publish::Publisher, state::PlatformStatusRecord};
use anyhow::{Context, Result};
use chrono::{Days, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

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
            paused: true,
            state: "stopped".to_string(),
            last_tick: None,
            next_wakeup: None,
            last_message: "已停止".to_string(),
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

    pub async fn tick(&self, reason: &str) -> Result<()> {
        let tz: Tz = self
            .config
            .scheduler
            .timezone
            .parse()
            .unwrap_or(chrono_tz::Asia::Shanghai);
        let now = Utc::now().with_timezone(&tz);
        let (active_start, active_end) = self.active_window()?;

        {
            let mut status = self.status.write().await;
            status.last_tick = Some(now.to_rfc3339());
        }

        if !is_in_active_window(now.time(), active_start, active_end) {
            let mut status = self.status.write().await;
            status.state = "sleeping".to_string();
            status.last_message = format!("当前不在检测窗口 {active_start}-{active_end}，跳过扫描");
            status.recent_platform_statuses = self.publisher.recent_statuses().unwrap_or_default();
            return Ok(());
        }

        {
            let mut status = self.status.write().await;
            status.state = "scanning".to_string();
            status.last_message = format!("开始检测: {reason}");
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
        status.state = if paused { "stopped" } else { "idle" }.to_string();
        status.last_message = if paused {
            "已停止".to_string()
        } else {
            "已启动".to_string()
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

    fn active_window(&self) -> Result<(NaiveTime, NaiveTime)> {
        let active_start =
            NaiveTime::parse_from_str(&self.config.scheduler.active_start_time, "%H:%M:%S")
                .context("parse active_start_time")?;
        let active_end =
            NaiveTime::parse_from_str(&self.config.scheduler.active_end_time, "%H:%M:%S")
                .context("parse active_end_time")?;
        Ok((active_start, active_end))
    }

}

fn is_in_active_window(now: NaiveTime, active_start: NaiveTime, active_end: NaiveTime) -> bool {
    now >= active_start && now < active_end
}
