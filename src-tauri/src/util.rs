use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

pub type Res<T> = Result<T, String>;

/// A child process that never flashes a console window (Windows).
pub fn hidden(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"));
    base.join("AndroidTools")
}

#[cfg(windows)]
pub fn program_data_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("AndroidTools")
}

pub fn default_save_dir() -> PathBuf {
    home().join("Pictures").join("Android Tools")
}

pub fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ── language ──

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

/// Name of a folder inside the save directory, in the UI language.
pub fn folder(ru: &'static str, en: &'static str) -> &'static str {
    if ENGLISH.load(Ordering::Relaxed) { en } else { ru }
}

/// Locate an executable on PATH.
#[allow(dead_code)]
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}
