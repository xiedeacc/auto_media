use crate::{platforms::Platform, publish::job::PublishJob};
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex};

pub struct StateStore {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformStatusRecord {
    pub job_id: String,
    pub platform: String,
    pub status: String,
    pub remote_url: Option<String>,
    pub last_error: Option<String>,
    pub attempt_count: i64,
    pub updated_at: String,
}

impl StateStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().expect("state mutex poisoned");
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS publish_jobs (
              job_id TEXT PRIMARY KEY,
              target_date TEXT NOT NULL,
              title TEXT NOT NULL,
              image_path TEXT NOT NULL,
              image_size INTEGER NOT NULL,
              image_mtime INTEGER NOT NULL,
              created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS publish_platform_status (
              job_id TEXT NOT NULL,
              platform TEXT NOT NULL,
              status TEXT NOT NULL,
              remote_url TEXT,
              last_error TEXT,
              attempt_count INTEGER NOT NULL DEFAULT 0,
              updated_at TEXT NOT NULL,
              PRIMARY KEY (job_id, platform)
            );

            CREATE TABLE IF NOT EXISTS app_kv (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    pub fn upsert_job(&self, job: &PublishJob) -> Result<()> {
        let conn = self.conn.lock().expect("state mutex poisoned");
        conn.execute(
            r#"
            INSERT OR IGNORE INTO publish_jobs
              (job_id, target_date, title, image_path, image_size, image_mtime, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                job.job_id,
                job.target_date.to_string(),
                job.title,
                job.image_path.display().to_string(),
                job.image_size as i64,
                job.image_mtime,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn is_platform_success(&self, job_id: &str, platform: Platform) -> Result<bool> {
        let conn = self.conn.lock().expect("state mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT status FROM publish_platform_status WHERE job_id = ?1 AND platform = ?2",
        )?;
        let mut rows = stmt.query(params![job_id, platform.as_str()])?;
        if let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            Ok(status == "success")
        } else {
            Ok(false)
        }
    }

    pub fn mark_platform(
        &self,
        job_id: &str,
        platform: Platform,
        status: &str,
        remote_url: Option<&str>,
        last_error: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("state mutex poisoned");
        conn.execute(
            r#"
            INSERT INTO publish_platform_status
              (job_id, platform, status, remote_url, last_error, attempt_count, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)
            ON CONFLICT(job_id, platform) DO UPDATE SET
              status = excluded.status,
              remote_url = excluded.remote_url,
              last_error = excluded.last_error,
              attempt_count = publish_platform_status.attempt_count + 1,
              updated_at = excluded.updated_at
            "#,
            params![
                job_id,
                platform.as_str(),
                status,
                remote_url,
                last_error,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn recent_platform_statuses(&self, limit: usize) -> Result<Vec<PlatformStatusRecord>> {
        let conn = self.conn.lock().expect("state mutex poisoned");
        let mut stmt = conn.prepare(
            r#"
            SELECT job_id, platform, status, remote_url, last_error, attempt_count, updated_at
            FROM publish_platform_status
            ORDER BY updated_at DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(PlatformStatusRecord {
                job_id: row.get(0)?,
                platform: row.get(1)?,
                status: row.get(2)?,
                remote_url: row.get(3)?,
                last_error: row.get(4)?,
                attempt_count: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}
