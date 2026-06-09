use anyhow::Result;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

pub fn is_autostart_launch() -> bool {
    std::env::args().any(|arg| arg == "--autostart")
}

pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<()> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()?;
    } else {
        manager.disable()?;
    }
    Ok(())
}

pub fn autostart_enabled(app: &AppHandle) -> Result<bool> {
    Ok(app.autolaunch().is_enabled()?)
}

pub fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
