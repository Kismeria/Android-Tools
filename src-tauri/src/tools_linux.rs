//! Linux: adb and scrcpy come from the distribution (Arch: `android-tools`, `scrcpy`).
use std::path::PathBuf;

use serde::Serialize;

use crate::util::{hidden, which, Res};

const MISSING: &str = "Установите пакеты android-tools и scrcpy";
const MANAGED: &str = "Обновляется через pacman";

pub fn ensure() -> Res<()> {
    if adb_exe().is_file() && scrcpy_exe().is_some() {
        Ok(())
    } else {
        Err(MISSING.into())
    }
}

pub fn adb_exe() -> PathBuf {
    which("adb").unwrap_or_else(|| PathBuf::from("/usr/bin/adb"))
}

pub fn scrcpy_exe() -> Option<PathBuf> {
    which("scrcpy")
}

pub fn scrcpy_server() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("SCRCPY_SERVER_PATH").map(PathBuf::from),
        Some(PathBuf::from("/usr/share/scrcpy/scrcpy-server")),
        Some(PathBuf::from("/usr/local/share/scrcpy/scrcpy-server")),
    ];
    candidates.into_iter().flatten().find(|p| p.is_file())
}

fn output(mut cmd: std::process::Command) -> String {
    cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

pub fn adb_version() -> String {
    let mut cmd = hidden(adb_exe());
    cmd.arg("version");
    output(cmd)
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
    output(cmd)
        .lines()
        .next()
        .filter(|l| l.starts_with("scrcpy"))
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or_default()
        .to_string()
}

#[allow(dead_code)]
pub fn kill_adb_server() {
    let _ = hidden(adb_exe()).arg("kill-server").output();
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
        bin_dir: "/usr/bin".into(),
        managed: true,
    }
}

pub fn update_platform_tools() -> Res<String> {
    Err(MANAGED.into())
}

pub fn update_scrcpy() -> Res<String> {
    Err(MANAGED.into())
}
