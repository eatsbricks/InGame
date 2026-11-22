pub mod config;
pub mod key_filter;
pub mod state;
pub mod win32;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};

use crate::config::{EscapeKey, Settings};
use crate::state::ProtectionState;
pub use crate::win32::coordinator::AppSnapshot;
use crate::win32::coordinator::ProtectionCoordinator;

const TRAY_ID: &str = "main-tray";
const SETTINGS_WINDOW_LABEL: &str = "settings";
const APP_STATE_CHANGED_EVENT: &str = "app-state-changed";

const MENU_ID_STATUS: &str = "status";
const MENU_ID_OPEN: &str = "open";
const MENU_ID_TOGGLE_PAUSE: &str = "toggle_pause";
const MENU_ID_QUIT: &str = "quit";

const ICON_STANDBY_BYTES: &[u8] = include_bytes!("../icons/tray-standby.png");
const ICON_PROTECTED_BYTES: &[u8] = include_bytes!("../icons/tray-protected.png");
const ICON_PAUSED_BYTES: &[u8] = include_bytes!("../icons/tray-paused.png");

struct AppQuitFlag(AtomicBool);

struct TrayMenuHandles {
    status_item: MenuItem<tauri::Wry>,
    toggle_pause_item: MenuItem<tauri::Wry>,
    last_rendered_state: Mutex<Option<ProtectionState>>,
}

fn tray_tooltip(state: ProtectionState) -> &'static str {
    match state {
        ProtectionState::Protected => "InGame — กำลังป้องกัน",
        ProtectionState::Standby => "InGame — รอเปิด PUBG",
        ProtectionState::Paused => "InGame — พักการทำงาน",
    }
}

fn tray_status_label(state: ProtectionState) -> &'static str {
    tray_tooltip(state)
}

fn tray_toggle_pause_label(state: ProtectionState) -> &'static str {
    match state {
        ProtectionState::Paused => "เปิดการป้องกันต่อ",
        ProtectionState::Standby | ProtectionState::Protected => "พักการป้องกันชั่วคราว",
    }
}

fn tray_icon_image(state: ProtectionState) -> tauri::Result<Image<'static>> {
    let bytes = match state {
        ProtectionState::Protected => ICON_PROTECTED_BYTES,
        ProtectionState::Standby => ICON_STANDBY_BYTES,
        ProtectionState::Paused => ICON_PAUSED_BYTES,
    };
    Image::from_bytes(bytes).map(|img| img.to_owned())
}

fn should_start_minimized<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .skip(1)
        .any(|arg| arg.as_ref() == "--minimized")
}

#[cfg(windows)]
fn apply_windows_rounded_corners(window: &tauri::WebviewWindow) {
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        DWM_WINDOW_CORNER_PREFERENCE,
    };

    if let Ok(hwnd) = window.hwnd() {
        let preference: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
        // SAFETY: `hwnd.0` is a live top-level window owned by Tauri and `preference` matches
        // `DWMWA_WINDOW_CORNER_PREFERENCE`. On Windows 10 where the attribute is unsupported,
        // DWM returns an error which is safely ignored.
        let _ = unsafe {
            DwmSetWindowAttribute(
                HWND(hwnd.0),
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&preference as *const DWM_WINDOW_CORNER_PREFERENCE).cast::<c_void>(),
                size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            )
        };
    }
}

#[cfg(not(windows))]
fn apply_windows_rounded_corners(_window: &tauri::WebviewWindow) {}

#[cfg(windows)]
fn window_raw_hwnd(window: &tauri::WebviewWindow) -> isize {
    window.hwnd().map(|hwnd| hwnd.0 as isize).unwrap_or(0)
}

#[cfg(not(windows))]
fn window_raw_hwnd(_window: &tauri::WebviewWindow) -> isize {
    0
}

fn show_or_create_settings_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let builder = WebviewWindowBuilder::new(
        app,
        SETTINGS_WINDOW_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .title("InGame")
    .inner_size(300.0, 358.0)
    .resizable(false)
    .decorations(false)
    .transparent(false)
    .background_color(tauri::window::Color(0x0A, 0x0A, 0x0A, 0xFF))
    .skip_taskbar(false)
    .center()
    .visible(false);

    if let Ok(window) = builder.build() {
        apply_windows_rounded_corners(&window);

        let app_handle = app.clone();
        let settings_hwnd = window_raw_hwnd(&window);
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::Focused(true) = event {
                if let Some(coordinator) = app_handle.try_state::<ProtectionCoordinator>() {
                    coordinator.notify_non_pubg_foreground(settings_hwnd);
                }
            }
        });

        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn sync_tray_and_window(app: &AppHandle, snapshot: &AppSnapshot) {
    if let Some(handles) = app.try_state::<TrayMenuHandles>() {
        let should_update_tray = handles
            .last_rendered_state
            .lock()
            .map(|mut last| {
                if *last != Some(snapshot.state) {
                    *last = Some(snapshot.state);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(true);

        if should_update_tray {
            let _ = handles
                .status_item
                .set_text(tray_status_label(snapshot.state));
            let _ = handles
                .toggle_pause_item
                .set_text(tray_toggle_pause_label(snapshot.state));

            if let Some(tray) = app.tray_by_id(TRAY_ID) {
                if let Ok(icon) = tray_icon_image(snapshot.state) {
                    let _ = tray.set_icon(Some(icon));
                }
                let _ = tray.set_tooltip(Some(tray_tooltip(snapshot.state)));
            }
        }
    }

    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        let _ = window.emit(APP_STATE_CHANGED_EVENT, snapshot);
    }
}

fn setup_system_tray(app: &tauri::App, initial_state: ProtectionState) -> tauri::Result<()> {
    let status_item = MenuItem::with_id(
        app,
        MENU_ID_STATUS,
        tray_status_label(initial_state),
        false,
        None::<&str>,
    )?;
    let open_item = MenuItem::with_id(app, MENU_ID_OPEN, "เปิดตั้งค่า", true, None::<&str>)?;
    let toggle_pause_item = MenuItem::with_id(
        app,
        MENU_ID_TOGGLE_PAUSE,
        tray_toggle_pause_label(initial_state),
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, MENU_ID_QUIT, "ปิดโปรแกรม", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[&status_item, &open_item, &toggle_pause_item, &quit_item],
    )?;

    app.manage(TrayMenuHandles {
        status_item,
        toggle_pause_item,
        last_rendered_state: Mutex::new(Some(initial_state)),
    });

    let icon = tray_icon_image(initial_state)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(tray_tooltip(initial_state))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_ID_OPEN => {
                show_or_create_settings_window(app);
            }
            MENU_ID_TOGGLE_PAUSE => {
                let _ = app.state::<ProtectionCoordinator>().toggle_pause();
            }
            MENU_ID_QUIT => {
                app.state::<AppQuitFlag>().0.store(true, Ordering::Release);
                app.state::<ProtectionCoordinator>().shutdown();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_or_create_settings_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

#[tauri::command]
fn get_app_state(coordinator: State<'_, ProtectionCoordinator>) -> AppSnapshot {
    coordinator.snapshot()
}

#[tauri::command]
fn update_settings(
    coordinator: State<'_, ProtectionCoordinator>,
    patch: Settings,
) -> Result<AppSnapshot, String> {
    coordinator.update_settings(patch)
}

#[tauri::command]
fn toggle_pause(coordinator: State<'_, ProtectionCoordinator>) -> Result<AppSnapshot, String> {
    coordinator.toggle_pause()
}

#[tauri::command]
fn set_escape_key(
    coordinator: State<'_, ProtectionCoordinator>,
    key: EscapeKey,
) -> Result<AppSnapshot, String> {
    coordinator.set_escape_key(key)
}

#[tauri::command]
fn close_settings_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        let _ = window.close();
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                show_or_create_settings_window(&handle);
            });
        }))
        .manage(AppQuitFlag(AtomicBool::new(false)))
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            update_settings,
            toggle_pause,
            set_escape_key,
            close_settings_window,
        ])
        .setup(|app| {
            let app_handle = app.handle().clone();
            let coordinator = ProtectionCoordinator::start(move |snapshot| {
                let handle = app_handle.clone();
                let _ = app_handle.run_on_main_thread(move || {
                    sync_tray_and_window(&handle, &snapshot);
                });
            })
            .map_err(std::io::Error::other)?;

            let initial_snapshot = coordinator.snapshot();
            app.manage(coordinator);

            setup_system_tray(app, initial_snapshot.state)?;

            // Re-sync once after tray registration in case state transitioned during setup.
            let current_snapshot = app.state::<ProtectionCoordinator>().snapshot();
            sync_tray_and_window(app.handle(), &current_snapshot);

            if !should_start_minimized(std::env::args()) {
                show_or_create_settings_window(app.handle());
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| match event {
        RunEvent::ExitRequested { api, code, .. } => {
            if code.is_none() && !app_handle.state::<AppQuitFlag>().0.load(Ordering::Acquire) {
                api.prevent_exit();
            }
        }
        RunEvent::Exit => {
            if let Some(coordinator) = app_handle.try_state::<ProtectionCoordinator>() {
                coordinator.shutdown();
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_snapshot_serializes_with_expected_wire_keys() {
        let snapshot = AppSnapshot {
            state: ProtectionState::Protected,
            settings: Settings::default(),
            has_english_layout: true,
            elevation_warning: false,
        };
        let value = serde_json::to_value(&snapshot).expect("serialize AppSnapshot");
        let obj = value.as_object().expect("AppSnapshot object");

        assert_eq!(obj.get("state").and_then(|v| v.as_str()), Some("Protected"));
        assert_eq!(
            obj.get("has_english_layout").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            obj.get("elevation_warning").and_then(|v| v.as_bool()),
            Some(false)
        );
        let settings = obj
            .get("settings")
            .and_then(|v| v.as_object())
            .expect("settings object");
        assert!(settings.contains_key("enabled"));
        assert!(settings.contains_key("lock_language"));
        assert!(settings.contains_key("block_minimize"));
        assert!(settings.contains_key("block_caps_lock"));
        assert!(settings.contains_key("escape_key"));
        assert!(settings.contains_key("launch_at_startup"));
    }

    #[test]
    fn tray_labels_and_embedded_icons_cover_all_states() {
        for state in [
            ProtectionState::Standby,
            ProtectionState::Protected,
            ProtectionState::Paused,
        ] {
            let icon = tray_icon_image(state).expect("decode embedded tray icon PNG");
            assert!(icon.width() > 0 && icon.height() > 0);
            assert!(!tray_tooltip(state).is_empty());
            assert!(!tray_status_label(state).is_empty());
            assert!(!tray_toggle_pause_label(state).is_empty());
        }

        assert_eq!(
            tray_tooltip(ProtectionState::Protected),
            "InGame — กำลังป้องกัน"
        );
        assert_eq!(
            tray_tooltip(ProtectionState::Standby),
            "InGame — รอเปิด PUBG"
        );
        assert_eq!(tray_tooltip(ProtectionState::Paused), "InGame — พักการทำงาน");
        assert_eq!(
            tray_toggle_pause_label(ProtectionState::Paused),
            "เปิดการป้องกันต่อ"
        );
        assert_eq!(
            tray_toggle_pause_label(ProtectionState::Standby),
            "พักการป้องกันชั่วคราว"
        );
        assert_eq!(
            tray_toggle_pause_label(ProtectionState::Protected),
            "พักการป้องกันชั่วคราว"
        );
    }

    #[test]
    fn should_start_minimized_detects_cli_flag() {
        assert!(!should_start_minimized(["ingame.exe"]));
        assert!(should_start_minimized(["ingame.exe", "--minimized"]));
        assert!(should_start_minimized([
            "ingame.exe",
            "--other",
            "--minimized"
        ]));
        assert!(!should_start_minimized(["--minimized"]));
    }
}
