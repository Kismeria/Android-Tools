//! Tauri commands. Blocking work runs on the async runtime's blocking pool.
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};
use tauri::{AppHandle, State};

use crate::camera::{self, Camera};
use crate::mic::{self, Mic};
use crate::mirror::{self, Mirror, MirrorSettings};
use crate::util::{err, hidden, Res};
use crate::{adb, tools};

#[derive(Default)]
pub struct AppState {
    pub mirror: Mutex<Mirror>,
    pub camera: Mutex<Option<Camera>>,
    pub mic: Mutex<Option<Mic>>,
}

impl AppState {
    pub fn shutdown(&self) {
        self.mirror.lock().unwrap().stop();
        if let Some(cam) = self.camera.lock().unwrap().take() {
            cam.stop();
        }
        if let Some(mic) = self.mic.lock().unwrap().take() {
            mic.stop();
        }
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Res<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

use crate::util::{default_save_dir, folder};

// ── app / tools ──

#[tauri::command]
pub async fn app_info() -> Res<Value> {
    blocking(|| {
        tools::ensure()?;
        Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "save_dir": default_save_dir().display().to_string(),
            "tools": tools::info(),
        }))
    })
    .await
}

#[tauri::command]
pub fn set_language(lang: String) {
    crate::util::set_english(lang == "en");
}

/// Window caption colors follow the UI theme (`#rrggbb`).
#[tauri::command]
pub fn set_window_theme(window: tauri::WebviewWindow, dark: bool, caption: String, border: String, text: String) {
    crate::paint_titlebar(&window, dark, &caption, &border, &text);
}

/// Interface scale (browser-style zoom of the whole page).
#[tauri::command]
pub fn set_zoom(window: tauri::WebviewWindow, scale: f64) {
    let _ = window.set_zoom(scale.clamp(0.5, 2.0));
}

#[tauri::command]
pub async fn update_check() -> Res<crate::update::UpdateInfo> {
    blocking(crate::update::check).await
}

#[tauri::command]
pub async fn update_apply(app: AppHandle, url: String) -> Res<()> {
    blocking(move || crate::update::apply(&app, &url)).await
}

#[tauri::command]
pub async fn tools_info() -> Res<tools::ToolsInfo> {
    blocking(|| Ok(tools::info())).await
}

#[tauri::command]
pub async fn update_tool(name: String) -> Res<String> {
    blocking(move || match name.as_str() {
        "adb" => tools::update_platform_tools(),
        _ => tools::update_scrcpy(),
    })
    .await
}

#[tauri::command]
pub async fn open_path(path: String) -> Res<()> {
    blocking(move || {
        let p = PathBuf::from(&path);
        if !p.exists() {
            std::fs::create_dir_all(&p).map_err(err)?;
        }
        let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
        hidden(opener).arg(&p).spawn().map_err(err)?;
        Ok(())
    })
    .await
}

/// Opens a web link in the PC browser.
#[tauri::command]
pub async fn open_external(url: String) -> Res<()> {
    if !url.starts_with("https://") {
        return Err("Неверная ссылка".into());
    }
    blocking(move || {
        #[cfg(windows)]
        hidden("cmd").args(["/c", "start", "", &url]).spawn().map_err(err)?;
        #[cfg(not(windows))]
        hidden("xdg-open").arg(&url).spawn().map_err(err)?;
        Ok(())
    })
    .await
}

// ── devices ──

#[tauri::command]
pub async fn devices() -> Res<Vec<adb::Device>> {
    blocking(adb::devices).await
}

#[tauri::command]
pub async fn device_info(serial: String) -> Res<Value> {
    blocking(move || adb::device_info(&serial)).await
}

#[tauri::command]
pub async fn wifi_connect(address: String) -> Res<String> {
    blocking(move || adb::connect(&address)).await
}

#[tauri::command]
pub async fn wifi_disconnect(address: String) -> Res<String> {
    blocking(move || adb::disconnect(&address)).await
}

#[tauri::command]
pub async fn wifi_pair(address: String, code: String) -> Res<String> {
    blocking(move || adb::pair(&address, &code)).await
}

#[tauri::command]
pub async fn wifi_switch(serial: String) -> Res<String> {
    blocking(move || adb::to_wifi(&serial)).await
}

#[tauri::command]
pub async fn wifi_scan() -> Res<Vec<String>> {
    blocking(adb::mdns).await
}

// ── mirror ──

#[tauri::command]
pub async fn mirror_start(
    state: State<'_, AppState>,
    serial: String,
    title: String,
    save_dir: String,
    settings: MirrorSettings,
) -> Res<()> {
    let record_dir = PathBuf::from(save_dir).join(folder("Записи", "Recordings")).display().to_string();
    let args = mirror::build_args(&serial, &settings, &title, &record_dir);
    let mut m = Mirror::default();
    let m = blocking(move || m.start(args).map(|_| m)).await?;
    *state.mirror.lock().unwrap() = m;
    Ok(())
}

#[tauri::command]
pub fn mirror_stop(state: State<'_, AppState>) {
    state.mirror.lock().unwrap().stop();
}

#[tauri::command]
pub fn mirror_state(state: State<'_, AppState>) -> Value {
    let mut m = state.mirror.lock().unwrap();
    json!({ "running": m.running(), "input_blocked": m.input_blocked() })
}

// ── camera ──

#[tauri::command]
pub async fn camera_start(app: AppHandle, state: State<'_, AppState>, settings: camera::Settings) -> Res<()> {
    let old = state.camera.lock().unwrap().take();
    if let Some(old) = old {
        blocking(move || {
            old.stop();
            Ok(())
        })
        .await?;
    }
    *state.camera.lock().unwrap() = Some(Camera::start(app, settings));
    Ok(())
}

#[tauri::command]
pub async fn camera_stop(state: State<'_, AppState>) -> Res<()> {
    let cam = state.camera.lock().unwrap().take();
    if let Some(cam) = cam {
        blocking(move || {
            cam.stop();
            Ok(())
        })
        .await?;
    }
    Ok(())
}

#[tauri::command]
pub fn camera_live(state: State<'_, AppState>, live: camera::Live) {
    if let Some(cam) = state.camera.lock().unwrap().as_ref() {
        cam.update_live(live);
    }
}

#[tauri::command]
pub fn camera_running(state: State<'_, AppState>) -> bool {
    let mut g = state.camera.lock().unwrap();
    if g.as_ref().map(|c| c.finished()).unwrap_or(false) {
        *g = None;
    }
    g.is_some()
}

#[tauri::command]
pub async fn camera_list(serial: String) -> Res<Value> {
    blocking(move || adb::list_cameras(&serial)).await
}

#[tauri::command]
pub async fn camera_status() -> Res<camera::install::CameraStatus> {
    blocking(|| Ok(camera::install::status())).await
}

#[tauri::command]
pub async fn camera_install() -> Res<()> {
    blocking(|| camera::install::run_elevated("--camera-install")).await
}

#[tauri::command]
pub async fn camera_remove(state: State<'_, AppState>) -> Res<()> {
    let cam = state.camera.lock().unwrap().take();
    blocking(move || {
        if let Some(cam) = cam {
            cam.stop();
        }
        camera::install::run_elevated("--camera-remove")
    })
    .await
}

// ── microphone ──

#[tauri::command]
pub async fn mic_start(app: AppHandle, state: State<'_, AppState>, settings: mic::Settings) -> Res<()> {
    let old = state.mic.lock().unwrap().take();
    if let Some(old) = old {
        blocking(move || {
            old.stop();
            Ok(())
        })
        .await?;
    }
    *state.mic.lock().unwrap() = Some(Mic::start(app, settings));
    Ok(())
}

#[tauri::command]
pub async fn mic_stop(state: State<'_, AppState>) -> Res<()> {
    let mic = state.mic.lock().unwrap().take();
    if let Some(mic) = mic {
        blocking(move || {
            mic.stop();
            Ok(())
        })
        .await?;
    }
    Ok(())
}

#[tauri::command]
pub fn mic_live(state: State<'_, AppState>, live: mic::Live) {
    if let Some(mic) = state.mic.lock().unwrap().as_ref() {
        mic.update_live(live);
    }
}

#[tauri::command]
pub fn mic_running(state: State<'_, AppState>) -> bool {
    let mut g = state.mic.lock().unwrap();
    if g.as_ref().map(|m| m.finished()).unwrap_or(false) {
        *g = None;
    }
    g.is_some()
}

#[tauri::command]
pub async fn mic_status() -> Res<mic::sys::MicStatus> {
    blocking(|| Ok(mic::sys::status())).await
}

#[tauri::command]
pub async fn mic_install() -> Res<()> {
    blocking(mic::sys::install).await
}

#[tauri::command]
pub async fn mic_remove(state: State<'_, AppState>) -> Res<()> {
    let running = state.mic.lock().unwrap().take();
    blocking(move || {
        if let Some(m) = running {
            m.stop();
        }
        mic::sys::remove()
    })
    .await
}

// ── apps ──

#[tauri::command]
pub async fn apps_list(serial: String, kind: String) -> Res<Value> {
    blocking(move || adb::packages(&serial, &kind)).await
}

#[tauri::command]
pub async fn app_details(serial: String, package: String) -> Res<Value> {
    blocking(move || adb::package_details(&serial, &package)).await
}

#[tauri::command]
pub async fn app_action(serial: String, packages: Vec<String>, action: String, save_dir: String) -> Res<String> {
    blocking(move || {
        let mut done = Vec::new();
        for p in &packages {
            match action.as_str() {
                "launch" => adb::launch(&serial, p)?,
                "stop" => adb::force_stop(&serial, p)?,
                "clear" => adb::clear_data(&serial, p)?,
                "enable" => adb::set_enabled(&serial, p, true)?,
                "disable" => adb::set_enabled(&serial, p, false)?,
                "uninstall" => adb::uninstall(&serial, p)?,
                "extract" => {
                    let dir = PathBuf::from(&save_dir).join("APK");
                    done.push(adb::extract_apk(&serial, p, &dir)?.display().to_string());
                }
                _ => return Err("Неизвестное действие".into()),
            }
        }
        Ok(done.join("\n"))
    })
    .await
}

#[tauri::command]
pub async fn app_install(serial: String, path: String) -> Res<String> {
    blocking(move || adb::install(&serial, &path)).await
}

// ── files ──

#[tauri::command]
pub async fn files_list(serial: String, path: String) -> Res<Value> {
    blocking(move || adb::list_dir(&serial, &path)).await
}

#[tauri::command]
pub async fn files_push(serial: String, files: Vec<String>, dest: String) -> Res<()> {
    blocking(move || {
        for f in &files {
            adb::push(&serial, f, &dest)?;
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn files_pull(serial: String, remote: Vec<String>, dest: String) -> Res<String> {
    blocking(move || {
        std::fs::create_dir_all(&dest).map_err(err)?;
        for r in &remote {
            adb::pull(&serial, r, &dest)?;
        }
        Ok(dest)
    })
    .await
}

#[tauri::command]
pub async fn files_op(serial: String, op: String, path: String, target: Option<String>) -> Res<()> {
    blocking(move || {
        let cmd = match op.as_str() {
            "mkdir" => format!("mkdir -p {}", adb::q(&path)),
            "delete" => format!("rm -rf {}", adb::q(&path)),
            "rename" => format!("mv {} {}", adb::q(&path), adb::q(target.as_deref().unwrap_or(""))),
            _ => return Err("Неизвестная операция".into()),
        };
        adb::shell(&serial, &cmd).map(|_| ())
    })
    .await
}

// ── utilities ──

#[tauri::command]
pub async fn key_event(serial: String, key: String) -> Res<()> {
    blocking(move || adb::keyevent(&serial, &key)).await
}

#[tauri::command]
pub async fn screenshot(serial: String, save_dir: String, clipboard: bool) -> Res<Value> {
    blocking(move || {
        let png = adb::screenshot(&serial)?;
        let dir = PathBuf::from(save_dir).join(folder("Скриншоты", "Screenshots"));
        std::fs::create_dir_all(&dir).map_err(err)?;
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let path = dir.join(format!("screen_{secs}.png"));
        std::fs::write(&path, &png).map_err(err)?;
        let mut copied = false;
        if clipboard {
            copied = copy_png(&png).is_ok();
        }
        use base64::Engine;
        Ok(json!({
            "path": path.display().to_string(),
            "copied": copied,
            "data": format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png)),
        }))
    })
    .await
}

fn copy_png(png: &[u8]) -> Res<()> {
    // Decode PNG → RGBA with the Windows Imaging-free path: use arboard's image type.
    let img = decode_png(png)?;
    let mut cb = arboard::Clipboard::new().map_err(err)?;
    cb.set_image(img).map_err(err)
}

fn decode_png(png: &[u8]) -> Res<arboard::ImageData<'static>> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().map_err(err)?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(err)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
        png::ColorType::Rgb => buf[..w * h * 3].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        _ => return Err("Формат PNG не поддерживается".into()),
    };
    Ok(arboard::ImageData { width: w, height: h, bytes: rgba.into() })
}

#[tauri::command]
pub async fn reboot(serial: String, mode: String) -> Res<()> {
    blocking(move || {
        match mode.as_str() {
            "off" => adb::shell_lenient(&serial, "reboot -p").map(|_| ()),
            "" => adb::run_lenient(Some(&serial), &["reboot"], 20).map(|_| ()),
            m => adb::run_lenient(Some(&serial), &["reboot", m], 20).map(|_| ()),
        }
    })
    .await
}

#[tauri::command]
pub async fn send_text(serial: String, text: String) -> Res<()> {
    blocking(move || adb::send_text(&serial, &text)).await
}

#[tauri::command]
pub async fn open_url(serial: String, url: String) -> Res<()> {
    blocking(move || adb::open_url(&serial, &url)).await
}

#[tauri::command]
pub async fn shell(serial: String, command: String) -> Res<String> {
    blocking(move || adb::shell_lenient(&serial, &command)).await
}
