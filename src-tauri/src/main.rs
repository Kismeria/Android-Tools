#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;
#[cfg_attr(not(windows), path = "camera_linux.rs")]
mod camera;
mod commands;
mod iphone;
mod mic;
mod mirror;
#[cfg_attr(not(windows), path = "tools_linux.rs")]
mod tools;
mod update;
mod util;

use tauri::Manager;

fn colorref(hex: &str) -> Option<u32> {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).ok()?;
    // #rrggbb → 0x00bbggrr
    Some(((v & 0xFF) << 16) | (v & 0xFF00) | ((v >> 16) & 0xFF))
}

/// Caption bar in the colors of the UI theme (Windows 11); light/dark window theme elsewhere.
pub fn paint_titlebar(window: &tauri::WebviewWindow, dark: bool, caption: &str, border: &str, text: &str) {
    let _ = window.set_theme(Some(if dark { tauri::Theme::Dark } else { tauri::Theme::Light }));
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
        if let Ok(hwnd) = window.hwnd() {
            let hwnd = HWND(hwnd.0 as _);
            let mut attrs = vec![(20u32, dark as u32)];
            for (attr, hex) in [(35u32, caption), (34, border), (36, text)] {
                if let Some(c) = colorref(hex) {
                    attrs.push((attr, c));
                }
            }
            for (attr, value) in attrs {
                unsafe {
                    let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(attr as i32), &value as *const u32 as _, 4);
                }
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (caption, border, text, colorref);
}

/// Windows 10 may lack the WebView2 runtime that renders the UI: explain and offer the installer.
#[cfg(windows)]
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

#[cfg(not(windows))]
fn ensure_webview2() -> bool {
    // WebKitGTK's DMA-BUF renderer shows a blank window on some GPU/driver setups.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    true
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = mic::sys::handle_cli(&args) {
        std::process::exit(code);
    }
    if let Some(code) = camera::install::handle_cli(&args) {
        std::process::exit(code);
    }
    update::cleanup();
    if !ensure_webview2() {
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::default())
        .setup(|app| {
            if let Some(w) = app.get_webview_window("main") {
                paint_titlebar(&w, true, "#000000", "#1b1b1b", "#f2f2f2");
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
            commands::set_language,
            commands::set_window_theme,
            commands::set_zoom,
            commands::update_check,
            commands::update_apply,
            commands::tools_info,
            commands::update_tool,
            commands::open_path,
            commands::open_external,
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
            commands::iphone_start,
            commands::iphone_stop,
            commands::iphone_status,
            commands::mic_start,
            commands::mic_stop,
            commands::mic_live,
            commands::mic_running,
            commands::mic_status,
            commands::mic_install,
            commands::mic_remove,
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
