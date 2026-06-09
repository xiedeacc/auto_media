use anyhow::{Context, Result};
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init(logs_dir: &Path) -> Result<WorkerGuard> {
    std::fs::create_dir_all(logs_dir).with_context(|| format!("create {}", logs_dir.display()))?;
    let file_appender = tracing_appender::rolling::daily(logs_dir, "auto_media.log");
    let (writer, guard) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .init();

    Ok(guard)
}
