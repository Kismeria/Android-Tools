use std::ffi::OsStr;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub type Res<T> = Result<T, String>;

/// A child process that never flashes a console window.
pub fn hidden(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("AndroidTools")
}

pub fn program_data_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("AndroidTools")
}


pub fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}
