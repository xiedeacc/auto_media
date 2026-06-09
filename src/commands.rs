use crate::{app::SharedState, platforms::Platform};
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn get_status(
    state: State<'_, SharedState>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let status = state.controller.status().await;
    let autostart = crate::startup::autostart_enabled(&app).unwrap_or(false);
    let payload = serde_json::json!({
        "status": status,
        "paths": state.controller.path_summary(),
        "autostart_enabled": autostart
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
