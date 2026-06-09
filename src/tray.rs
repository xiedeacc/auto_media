use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

pub fn setup(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开主界面", true, None::<&str>)?;
    let run_now = MenuItem::with_id(app, "run_now", "立即检测", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &run_now, &separator, &quit])?;
    let icon = make_icon();

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("Auto Media")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => crate::startup::show_main_window(app),
            "run_now" => {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = handle.try_state::<crate::app::SharedState>() {
                        let _ = state.controller.run_now("tray").await;
                    }
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                crate::startup::show_main_window(&app);
            }
        })
        .build(app)?;

    Ok(())
}

fn make_icon() -> Image<'static> {
    let mut rgba = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32 {
        for x in 0..32 {
            let border = x < 3 || y < 3 || x > 28 || y > 28;
            let diagonal = (x as i32 - y as i32).abs() <= 2;
            let (r, g, b, a) = if border {
                (34, 94, 168, 255)
            } else if diagonal {
                (33, 150, 83, 255)
            } else {
                (245, 247, 250, 255)
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    Image::new_owned(rgba, 32, 32)
}
