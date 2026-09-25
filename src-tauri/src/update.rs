//! Updates from GitHub releases. Windows replaces the running exe; Arch Linux installs the
//! new package with `pacman -U` through polkit.
#[cfg(windows)]
use std::io::Read;

use serde::Serialize;
use serde_json::Value;

use crate::util::{err, Res};

const REPO: &str = "Kismeria/Android-Tools";
#[cfg(windows)]
const ASSET: &str = "Android-Tools.exe";
#[cfg(not(windows))]
const ASSET: &str = "android-tools-gui-x86_64.pkg.tar.zst";

#[derive(Serialize)]
pub struct UpdateInfo {
    current: String,
    latest: String,
    newer: bool,
    notes: String,
    page: String,
    /// "exe" (Windows), "pacman" (Arch package) or "manual".
    method: &'static str,
    asset: String,
}

fn version(v: &str) -> Vec<u32> {
    v.trim_start_matches('v').split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect()
}

fn get(url: &str) -> Res<ureq::Response> {
    ureq::get(url)
        .set("User-Agent", "AndroidTools")
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .map_err(|e| format!("GitHub: {e}"))
}

pub fn check() -> Res<UpdateInfo> {
    let release: Value =
        serde_json::from_reader(get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?.into_reader()).map_err(err)?;
    let latest = release["tag_name"].as_str().unwrap_or("").trim_start_matches('v').to_string();
    let current = env!("CARGO_PKG_VERSION").to_string();
    let asset = release["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(ASSET)))
        .and_then(|x| x["browser_download_url"].as_str())
        .unwrap_or("")
        .to_string();
    Ok(UpdateInfo {
        newer: !latest.is_empty() && version(&latest) > version(&current),
        current,
        latest,
        notes: release["body"].as_str().unwrap_or("").chars().take(1500).collect(),
        page: release["html_url"].as_str().unwrap_or("").to_string(),
        method: method(),
        asset,
    })
}

#[cfg(windows)]
fn method() -> &'static str {
    "exe"
}

#[cfg(not(windows))]
fn method() -> &'static str {
    let exe = std::env::current_exe().unwrap_or_default();
    let owned = crate::util::hidden("pacman")
        .args(["-Qqo", &exe.display().to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if owned { "pacman" } else { "manual" }
}

/// Installs `url` and restarts the app.
#[cfg(windows)]
pub fn apply(app: &tauri::AppHandle, url: &str) -> Res<()> {
    let mut data = Vec::new();
    get(url)?.into_reader().take(300 * 1024 * 1024).read_to_end(&mut data).map_err(err)?;
    if data.len() < 1024 * 1024 || &data[..2] != b"MZ" {
        return Err("Скачанный файл повреждён".into());
    }
    let exe = std::env::current_exe().map_err(err)?;
    let new = exe.with_extension("new");
    let old = exe.with_extension("old");
    std::fs::write(&new, &data).map_err(|e| format!("{}: {e}", new.display()))?;
    // A running exe cannot be overwritten, but it can be renamed.
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(err)?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(e.to_string());
    }
    std::process::Command::new(&exe).spawn().map_err(err)?;
    app.exit(0);
    Ok(())
}

#[cfg(not(windows))]
pub fn apply(app: &tauri::AppHandle, url: &str) -> Res<()> {
    if method() != "pacman" {
        return Err("Эта копия установлена не через pacman. Обновите командой из README.".into());
    }
    let out = crate::util::hidden("pkexec")
        .args(["pacman", "-U", "--noconfirm", url])
        .output()
        .map_err(|e| format!("pkexec: {e}"))?;
    match out.status.code() {
        Some(0) => {
            app.restart();
        }
        Some(126) | Some(127) => Err("Отменено".into()),
        _ => {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if msg.is_empty() { "pacman: ошибка".into() } else { msg })
        }
    }
}

/// Removes the previous exe left by an update (Windows).
pub fn cleanup() {
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old"));
    }
}
