//! adb (SDK Platform Tools) and scrcpy: embedded in the exe, unpacked on first run, updatable.
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use crate::util::{data_dir, err, hidden, Res};

static PLATFORM_TOOLS_ZIP: &[u8] = include_bytes!("../embed/platform-tools.zip");
static SCRCPY_ZIP: &[u8] = include_bytes!("../embed/scrcpy.zip");
static UNPACK_LOCK: Mutex<()> = Mutex::new(());

pub fn bin_dir() -> PathBuf {
    data_dir().join("bin")
}

fn extract(zip: impl Read + std::io::Seek, dest: &Path) -> Res<()> {
    let mut archive = zip::ZipArchive::new(zip).map_err(err)?;
    archive.extract(dest).map_err(err)
}

/// Unpack the embedded tools if this exe carries a build that is not on disk yet.
pub fn ensure() -> Res<()> {
    let _guard = UNPACK_LOCK.lock().unwrap();
    let dir = bin_dir();
    fs::create_dir_all(&dir).map_err(err)?;
    let stamp = dir.join(".embedded");
    let want = format!("{}:{}", PLATFORM_TOOLS_ZIP.len(), SCRCPY_ZIP.len());
    let have = fs::read_to_string(&stamp).unwrap_or_default();
    let adb_ok = dir.join("platform-tools").join("adb.exe").is_file();
    if have == want && adb_ok && scrcpy_exe().is_some() {
        return Ok(());
    }
    if !adb_ok || have != want {
        kill_adb_server();
        let _ = fs::remove_dir_all(dir.join("platform-tools"));
        extract(Cursor::new(PLATFORM_TOOLS_ZIP), &dir)?;
    }
    if scrcpy_exe().is_none() || have != want {
        remove_scrcpy_dirs(&dir);
        extract(Cursor::new(SCRCPY_ZIP), &dir)?;
    }
    fs::write(stamp, want).map_err(err)
}

fn remove_scrcpy_dirs(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("scrcpy") && e.path().is_dir() {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

pub fn adb_exe() -> PathBuf {
    bin_dir().join("platform-tools").join("adb.exe")
}

pub fn scrcpy_exe() -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(bin_dir())
        .ok()?
        .flatten()
        .map(|e| e.path().join("scrcpy.exe"))
        .filter(|p| p.is_file() && p.parent().unwrap().file_name().unwrap().to_string_lossy().starts_with("scrcpy"))
        .collect();
    found.sort();
    found.pop()
}

pub fn scrcpy_server() -> Option<PathBuf> {
    scrcpy_exe().map(|p| p.with_file_name("scrcpy-server")).filter(|p| p.is_file())
}

fn first_line(mut cmd: std::process::Command) -> String {
    cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

pub fn adb_version() -> String {
    let mut cmd = hidden(adb_exe());
    cmd.arg("version");
    first_line(cmd)
        .lines()
        .find(|l| l.starts_with("Version"))
        .and_then(|l| l.split_whitespace().nth(1))
        .map(|v| v.split('-').next().unwrap_or(v).to_string())
        .unwrap_or_default()
}

pub fn scrcpy_version() -> String {
    let Some(exe) = scrcpy_exe() else { return String::new() };
    let mut cmd = hidden(exe);
    cmd.arg("--version");
    first_line(cmd)
        .lines()
        .next()
        .filter(|l| l.starts_with("scrcpy"))
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or_default()
        .to_string()
}

pub fn kill_adb_server() {
    let adb = adb_exe();
    if adb.is_file() {
        let _ = hidden(adb).arg("kill-server").output();
    }
}

#[derive(Serialize)]
pub struct ToolsInfo {
    adb_version: String,
    adb_path: String,
    scrcpy_version: String,
    scrcpy_path: String,
    bin_dir: String,
    managed: bool,
}

pub fn info() -> ToolsInfo {
    ToolsInfo {
        adb_version: adb_version(),
        adb_path: adb_exe().display().to_string(),
        scrcpy_version: scrcpy_version(),
        scrcpy_path: scrcpy_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        bin_dir: bin_dir().display().to_string(),
        managed: false,
    }
}

fn download(url: &str) -> Res<Vec<u8>> {
    let resp = ureq::get(url).set("User-Agent", "AndroidTools").call().map_err(err)?;
    let mut buf = Vec::new();
    resp.into_reader().take(200 * 1024 * 1024).read_to_end(&mut buf).map_err(err)?;
    Ok(buf)
}

pub fn update_platform_tools() -> Res<String> {
    let data = download("https://dl.google.com/android/repository/platform-tools-latest-windows.zip")?;
    kill_adb_server();
    let dir = bin_dir();
    let _ = fs::remove_dir_all(dir.join("platform-tools"));
    extract(Cursor::new(data), &dir)?;
    Ok(adb_version())
}

pub fn update_scrcpy() -> Res<String> {
    let release: serde_json::Value = serde_json::from_reader(
        ureq::get("https://api.github.com/repos/Genymobile/scrcpy/releases/latest")
            .set("User-Agent", "AndroidTools")
            .call()
            .map_err(err)?
            .into_reader(),
    )
    .map_err(err)?;
    let url = release["assets"]
        .as_array()
        .and_then(|a| {
            a.iter().find(|x| {
                let n = x["name"].as_str().unwrap_or("");
                n.contains("win64") && n.ends_with(".zip")
            })
        })
        .and_then(|x| x["browser_download_url"].as_str())
        .ok_or("Не найден архив scrcpy для Windows")?
        .to_string();
    let data = download(&url)?;
    let dir = bin_dir();
    remove_scrcpy_dirs(&dir);
    extract(Cursor::new(data), &dir)?;
    Ok(scrcpy_version())
}
