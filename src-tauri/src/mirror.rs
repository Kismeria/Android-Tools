//! scrcpy window launcher.
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::tools;
use crate::util::{err, hidden, Res};

#[derive(Deserialize)]
pub struct MirrorSettings {
    pub max_size: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub codec: String,
    pub audio: bool,
    pub turn_off: bool,
    pub stay_awake: bool,
    pub touches: bool,
    pub on_top: bool,
    pub borderless: bool,
    pub fullscreen: bool,
    pub view_only: bool,
    pub uhid: bool,
    #[serde(default)]
    pub uhid_mouse: bool,
    pub record: bool,
    /// output (the whole phone sound) or mic.
    #[serde(default)]
    pub audio_source: String,
    /// Lock the video orientation: 0, 90, 180, 270; empty follows the phone.
    #[serde(default)]
    pub orientation: String,
    #[serde(default)]
    pub power_off_on_close: bool,
    /// Android SDK level: 29 (Android 10) gets its sound through sndcpy.
    #[serde(default)]
    pub sdk: u32,
}

/// scrcpy captures audio only on Android 11+; Android 10 plays through the sndcpy helper.
pub fn needs_sndcpy(s: &MirrorSettings) -> bool {
    s.audio && s.sdk == 29
}

pub fn build_args(serial: &str, s: &MirrorSettings, title: &str, record_dir: &str) -> Vec<String> {
    let mut a = vec![
        "-s".into(),
        serial.into(),
        format!("--window-title={title}"),
        format!("--video-codec={}", s.codec),
        format!("--video-bit-rate={}M", s.bitrate.max(1)),
        format!("--max-fps={}", s.fps.max(1)),
    ];
    if s.max_size > 0 {
        a.push(format!("--max-size={}", s.max_size));
    }
    if !s.audio || needs_sndcpy(s) {
        a.push("--no-audio".into());
    } else if s.audio_source == "mic" {
        a.push("--audio-source=mic".into());
    }
    if !s.orientation.is_empty() {
        a.push(format!("--capture-orientation=@{}", s.orientation));
    }
    if s.view_only {
        a.push("--no-control".into());
    } else {
        if s.turn_off {
            a.push("--turn-screen-off".into());
        }
        if s.stay_awake {
            a.push("--stay-awake".into());
        }
        if s.uhid {
            a.push("--keyboard=uhid".into());
        }
        if s.uhid_mouse {
            a.push("--mouse=uhid".into());
        }
        if s.power_off_on_close {
            a.push("--power-off-on-close".into());
        }
    }
    for (on, flag) in [
        (s.touches, "--show-touches"),
        (s.on_top, "--always-on-top"),
        (s.borderless, "--window-borderless"),
        (s.fullscreen, "--fullscreen"),
    ] {
        if on {
            a.push(flag.into());
        }
    }
    if s.record && !record_dir.is_empty() {
        let dir = PathBuf::from(record_dir);
        let _ = std::fs::create_dir_all(&dir);
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        a.push(format!("--record={}", dir.join(format!("screen_{secs}.mp4")).display()));
    }
    a
}

#[derive(Default)]
pub struct Mirror {
    child: Option<Child>,
    blocked: Arc<AtomicBool>,
    sound: Option<crate::sndcpy::Sndcpy>,
}

impl Mirror {
    pub fn running(&mut self) -> bool {
        let alive = match &mut self.child {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        };
        if !alive {
            if let Some(s) = self.sound.take() {
                s.stop();
            }
        }
        alive
    }

    /// Android 10 sound; a failure leaves the picture running and returns the reason.
    pub fn start_sound(&mut self, serial: &str) -> Res<()> {
        self.sound = Some(crate::sndcpy::Sndcpy::start(serial)?);
        Ok(())
    }

    pub fn start(&mut self, args: Vec<String>) -> Res<()> {
        tools::ensure()?;
        let exe = tools::scrcpy_exe().ok_or("scrcpy не найден")?;
        self.stop();
        let mut child = hidden(&exe)
            .args(&args)
            .current_dir(exe.parent().unwrap())
            .env("ADB", tools::adb_exe())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(err)?;

        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let blocked = Arc::new(AtomicBool::new(false));
        self.blocked = blocked.clone();
        for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
                     child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>)]
            .into_iter()
            .flatten()
        {
            let lines = lines.clone();
            let blocked = blocked.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if line.contains("INJECT_EVENTS") {
                        blocked.store(true, Ordering::Relaxed);
                    }
                    let mut l = lines.lock().unwrap();
                    l.push(line);
                    let n = l.len();
                    if n > 60 {
                        l.drain(..n - 60);
                    }
                }
            });
        }

        // scrcpy exits quickly on bad options or a missing device: surface that error.
        let deadline = Instant::now() + Duration::from_millis(2200);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                std::thread::sleep(Duration::from_millis(150));
                let l = lines.lock().unwrap();
                let msg = l
                    .iter()
                    .rev()
                    .find(|x| x.starts_with("ERROR"))
                    .map(|x| x.trim_start_matches("ERROR:").trim().to_string())
                    .unwrap_or_else(|| "scrcpy завершился".into());
                return Err(msg);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.child = Some(child);
        Ok(())
    }

    /// Some firmwares (MIUI/HyperOS, ColorOS…) forbid adb input injection: the phone shows
    /// but ignores mouse and keyboard. scrcpy logs a SecurityException about INJECT_EVENTS.
    pub fn input_blocked(&self) -> bool {
        self.blocked.load(Ordering::Relaxed)
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.sound.take() {
            s.stop();
        }
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
