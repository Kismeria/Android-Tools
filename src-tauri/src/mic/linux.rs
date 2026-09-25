//! Linux: the microphone device is a PipeWire/PulseAudio pipe source named
//! "Android Tools Microphone". Samples are written into its FIFO; every app sees it as a
//! microphone. No root needed: the source lives in the user's sound server.
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Stdio};

use serde::Serialize;

use super::{CHANNELS, RATE};
use crate::util::{err, hidden, Res};

pub const MIC_NAME: &str = "Android Tools Microphone";
const SOURCE: &str = "android_tools_mic";
const O_NONBLOCK: i32 = 0o4000;
/// Writes up to PIPE_BUF bytes to a FIFO are atomic: a frame is never split.
const PIPE_BUF: usize = 4096;

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

fn fifo() -> PathBuf {
    runtime_dir().join("android-tools-mic")
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

fn pactl(args: &[&str]) -> Res<String> {
    let out = hidden("pactl").args(args).output().map_err(|_| "Нужна утилита pactl (пакет libpulse)".to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn loaded() -> bool {
    pactl(&["list", "short", "sources"])
        .map(|s| s.lines().any(|l| l.split_whitespace().nth(1) == Some(SOURCE)))
        .unwrap_or(false)
}

fn pipewire() -> bool {
    pactl(&["info"]).map(|s| s.contains("PipeWire")).unwrap_or(false)
}

/// The system microphone device: its FIFO, opened without blocking.
pub struct Output {
    file: std::fs::File,
    buf: Vec<u8>,
}

impl Output {
    pub fn open(_buffer_ms: u32) -> Res<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        if !loaded() {
            return Err("Микрофон Android Tools не установлен — работает только прослушивание".into());
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(fifo())
            .map_err(|e| format!("Микрофон Android Tools: {e}"))?;
        Ok(Output { file, buf: Vec::new() })
    }

    pub fn write(&mut self, samples: &[i16]) {
        self.buf.clear();
        self.buf.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        for chunk in self.buf.chunks(PIPE_BUF) {
            // Full pipe (nobody records right now): drop instead of piling up latency.
            if self.file.write(chunk).is_err() {
                break;
            }
        }
    }
}

/// Listening on the PC speakers through `pacat`.
pub struct Monitor {
    child: Child,
    stdin: ChildStdin,
    buf: Vec<u8>,
}

impl Monitor {
    pub fn open(buffer_ms: u32) -> Res<Self> {
        let mut child = hidden("pacat")
            .args([
                "--playback",
                "--raw",
                "--format=s16le",
                &format!("--rate={RATE}"),
                &format!("--channels={CHANNELS}"),
                &format!("--latency-msec={buffer_ms}"),
                "--client-name=Android Tools",
                "--stream-name=Microphone monitor",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Нужна утилита pacat (пакет libpulse)".to_string())?;
        let stdin = child.stdin.take().ok_or("pacat")?;
        Ok(Monitor { child, stdin, buf: Vec::new() })
    }

    pub fn write(&mut self, samples: &[i16]) {
        self.buf.clear();
        self.buf.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        let _ = self.stdin.write_all(&self.buf);
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ── status / install ──

#[derive(Serialize)]
pub struct MicStatus {
    pub kind: &'static str,
    pub supported: bool,
    pub device: bool,
    pub name: String,
}

pub fn status() -> MicStatus {
    let supported = crate::util::which("pactl").is_some();
    let device = supported && loaded();
    MicStatus { kind: "pulse", supported, device, name: if device { MIC_NAME.into() } else { String::new() } }
}

#[allow(dead_code)]
pub fn handle_cli(_args: &[String]) -> Option<i32> {
    None
}

fn pipewire_conf() -> PathBuf {
    config_dir().join("pipewire/pipewire.conf.d/android-tools-mic.conf")
}

fn pulse_conf() -> PathBuf {
    config_dir().join("pulse/default.pa")
}

const PULSE_MARK: &str = "# android-tools-gui microphone";

fn module_args() -> Vec<String> {
    vec![
        "module-pipe-source".into(),
        format!("source_name={SOURCE}"),
        format!("file={}", fifo().display()),
        "format=s16le".into(),
        format!("rate={RATE}"),
        format!("channels={CHANNELS}"),
        format!("source_properties='device.description=\"{MIC_NAME}\"'"),
    ]
}

/// Loads the source now and makes it permanent for the next logins.
pub fn install() -> Res<()> {
    if !loaded() {
        let args = module_args();
        let mut a = vec!["load-module"];
        a.extend(args.iter().map(String::as_str));
        pactl(&a).map_err(|e| format!("Не удалось создать микрофон: {e}"))?;
    }
    if pipewire() {
        let conf = format!(
            r#"# Android Tools: phone microphone as a system microphone.
context.modules = [
  {{ name = libpipewire-module-pipe-tunnel
    args = {{
      tunnel.mode = source
      pipe.filename = "{fifo}"
      audio.format = S16LE
      audio.rate = {RATE}
      audio.channels = {CHANNELS}
      audio.position = [ FL FR ]
      stream.props = {{
        node.name = "{SOURCE}"
        node.description = "{MIC_NAME}"
        media.class = Audio/Source
      }}
    }}
  }}
]
"#,
            fifo = fifo().display()
        );
        let path = pipewire_conf();
        std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
        std::fs::write(&path, conf).map_err(err)?;
    } else {
        let path = pulse_conf();
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        if !text.contains(PULSE_MARK) {
            if text.is_empty() {
                text.push_str(".include /etc/pulse/default.pa\n");
            }
            text.push_str(&format!("{PULSE_MARK}\nload-module {}\n", module_args().join(" ")));
            std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
            std::fs::write(&path, text).map_err(err)?;
        }
    }
    Ok(())
}

pub fn remove() -> Res<()> {
    if let Ok(list) = pactl(&["list", "short", "modules"]) {
        for line in list.lines() {
            let mut cols = line.split('\t');
            let (Some(id), Some(name), Some(args)) = (cols.next(), cols.next(), cols.next()) else { continue };
            if name == "module-pipe-source" && args.contains(SOURCE) {
                let _ = pactl(&["unload-module", id]);
            }
        }
    }
    let _ = std::fs::remove_file(pipewire_conf());
    let path = pulse_conf();
    if let Ok(text) = std::fs::read_to_string(&path) {
        let mut skip = false;
        let kept: Vec<&str> = text
            .lines()
            .filter(|l| {
                if *l == PULSE_MARK {
                    skip = true;
                    return false;
                }
                if skip {
                    skip = false;
                    return false;
                }
                true
            })
            .collect();
        let _ = std::fs::write(&path, kept.join("\n") + "\n");
    }
    Ok(())
}
