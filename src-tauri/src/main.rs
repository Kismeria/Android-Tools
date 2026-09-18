#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;
mod camera;
mod commands;
mod mirror;
mod tools;
mod util;

use tauri::Manager;

/// Pure-black caption bar on Windows 11 to match the OLED UI.
fn black_titlebar(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
    if let Ok(hwnd) = window.hwnd() {
        let hwnd = HWND(hwnd.0 as _);
        for (attr, value) in [(20u32, 1u32), (35, 0x000000), (34, 0x1B1B1B), (36, 0xF2F2F2)] {
            unsafe {
                let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(attr as i32), &value as *const u32 as _, 4);
            }
        }
    }
}

/// Windows 10 may lack the WebView2 runtime that renders the UI: explain and offer the installer.
fn ensure_webview2() -> bool {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONWARNING, MB_YESNO};
    if tauri::webview_version().is_ok() {
        return true;
    }
    let answer = unsafe {
        MessageBoxW(
            None,
            w!("Для работы Android Tools нужен компонент Microsoft Edge WebView2 Runtime.

Открыть страницу загрузки? После установки запустите программу снова."),
            w!("Android Tools"),
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if answer == IDYES {
        let _ = util::hidden("cmd")
            .args(["/c", "start", "", "https://go.microsoft.com/fwlink/p/?LinkId=2124703"])
            .spawn();
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = camera::install::handle_cli(&args) {
        std::process::exit(code);
    }
    if !ensure_webview2() {
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::default())
        .setup(|app| {
            if let Some(w) = app.get_webview_window("main") {
                black_titlebar(&w);
            }
            std::thread::spawn(|| {
                let _ = tools::ensure();
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                window.state::<commands::AppState>().shutdown();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::tools_info,
            commands::update_tool,
            commands::open_path,
            commands::devices,
            commands::device_info,
            commands::wifi_connect,
            commands::wifi_disconnect,
            commands::wifi_pair,
            commands::wifi_switch,
            commands::wifi_scan,
            commands::mirror_start,
            commands::mirror_stop,
            commands::mirror_state,
            commands::camera_start,
            commands::camera_stop,
            commands::camera_live,
            commands::camera_running,
            commands::camera_list,
            commands::camera_status,
            commands::camera_install,
            commands::camera_remove,
            commands::apps_list,
            commands::app_details,
            commands::app_action,
            commands::app_install,
            commands::files_list,
            commands::files_push,
            commands::files_pull,
            commands::files_op,
            commands::key_event,
            commands::screenshot,
            commands::reboot,
            commands::send_text,
            commands::open_url,
            commands::shell,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Android Tools");
}
