use crate::{app::SharedState, platforms::Platform};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Local;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub async fn get_status(
    state: State<'_, SharedState>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let status = state.controller.status().await;
    let platform_sessions = state.controller.platform_sessions().await;
    let autostart = crate::startup::autostart_enabled(&app).unwrap_or(false);
    let payload = serde_json::json!({
        "status": status,
        "platform_sessions": platform_sessions,
        "paths": state.controller.path_summary(),
        "publish_tags": state.controller.publish_tags(),
        "publish_title_pattern": state.controller.publish_title_pattern(),
        "watermarks": state.controller.watermark_settings(),
        "autostart_enabled": autostart,
        "build_commit": env!("GIT_HASH"),
        "build_time": env!("BUILD_TIME")
    });
    Ok(payload)
}

#[tauri::command]
pub async fn run_now(state: State<'_, SharedState>) -> Result<(), String> {
    state
        .controller
        .run_now("manual")
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn select_images() -> Result<Vec<String>, String> {
    let files = rfd::FileDialog::new()
        .add_filter("Images", &["jpg", "jpeg", "png", "webp"])
        .set_title("选择图片")
        .pick_files()
        .unwrap_or_default();

    Ok(files
        .into_iter()
        .map(|path| path.display().to_string())
        .collect())
}

#[tauri::command]
pub async fn save_pasted_image(
    state: State<'_, SharedState>,
    file_name: String,
    bytes: Vec<u8>,
) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("粘贴图片为空".to_string());
    }

    let extension = image_extension(&file_name).unwrap_or("png");
    let dir = state.controller.paths.data_dir.join("pasted");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join(format!(
        "paste-{}-{}.{}",
        Local::now().format("%Y%m%d%H%M%S%3f"),
        sanitize_file_stem(&file_name),
        extension
    ));
    std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub async fn read_image_preview(path: String) -> Result<String, String> {
    let path = std::path::PathBuf::from(path);
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    if !mime.starts_with("image/") {
        return Err("只能预览图片文件".to_string());
    }

    let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

#[tauri::command]
pub async fn manual_publish(
    state: State<'_, SharedState>,
    app: AppHandle,
    title: String,
    text: String,
    tags: Option<String>,
    image_paths: Vec<String>,
    platforms: Option<Vec<String>>,
) -> Result<String, String> {
    let app_for_emit = app.clone();
    state
        .controller
        .manual_publish_with_progress(title, text, tags, image_paths, platforms, move |progress| {
            let _ = app_for_emit.emit("manual_publish_progress", progress);
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_logs(
    state: State<'_, SharedState>,
    lines: Option<usize>,
) -> Result<String, String> {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let preferred = state
        .controller
        .paths
        .logs_dir
        .join(format!("auto_media.log.{today}"));
    let path = if preferred.exists() {
        preferred
    } else {
        let mut files = std::fs::read_dir(&state.controller.paths.logs_dir)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_file())
            .collect::<Vec<_>>();
        files.sort_by_key(|entry| {
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        files
            .pop()
            .map(|entry| entry.path())
            .ok_or_else(|| "暂无日志文件".to_string())?
    };

    let content = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let line_count = lines.unwrap_or(240).clamp(20, 2000);
    let mut selected = content.lines().rev().take(line_count).collect::<Vec<_>>();
    selected.reverse();
    Ok(selected.join("\n"))
}

#[tauri::command]
pub async fn clear_records(state: State<'_, SharedState>) -> Result<(), String> {
    state
        .controller
        .clear_records()
        .await
        .map_err(|error| error.to_string())
}

fn image_extension(file_name: &str) -> Option<&'static str> {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("jpg")
    } else if lower.ends_with(".webp") {
        Some("webp")
    } else if lower.ends_with(".png") {
        Some("png")
    } else {
        None
    }
}

fn sanitize_file_stem(file_name: &str) -> String {
    let stem = file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("image")
        .split('.')
        .next()
        .unwrap_or("image");
    let clean = stem
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>();
    if clean.is_empty() {
        "image".to_string()
    } else {
        clean
    }
}

#[tauri::command]
pub async fn set_paused(state: State<'_, SharedState>, paused: bool) -> Result<(), String> {
    state.controller.set_paused(paused).await;
    Ok(())
}

#[tauri::command]
pub async fn login_platform(state: State<'_, SharedState>, platform: String) -> Result<(), String> {
    let platform = platform
        .parse::<Platform>()
        .map_err(|error| error.to_string())?;
    state
        .controller
        .login_platform(platform)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_platform_mode(
    state: State<'_, SharedState>,
    platform: String,
    mode: String,
) -> Result<(), String> {
    let platform = platform
        .parse::<Platform>()
        .map_err(|error| error.to_string())?;
    let prefer_cdp = !mode.eq_ignore_ascii_case("api");
    state
        .controller
        .set_platform_mode(platform, prefer_cdp)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_platform_watermark(
    state: State<'_, SharedState>,
    platform: String,
    enabled: bool,
    text: String,
) -> Result<(), String> {
    let platform = platform
        .parse::<Platform>()
        .map_err(|error| error.to_string())?;
    state
        .controller
        .set_platform_watermark(platform, enabled, text)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    crate::startup::set_autostart(&app, enabled).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn open_dir(state: State<'_, SharedState>, kind: String) -> Result<(), String> {
    let path = match kind.as_str() {
        "data" => state.controller.paths.data_dir.clone(),
        "logs" => state.controller.paths.logs_dir.clone(),
        "conf" => state.controller.paths.conf_dir.clone(),
        _ => return Err("unknown directory kind".to_string()),
    };
    opener::open(path).map_err(|error| error.to_string())
}
