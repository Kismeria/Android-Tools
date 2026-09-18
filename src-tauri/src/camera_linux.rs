//! Linux camera: scrcpy streams straight into a v4l2loopback device named
//! "Android Tools Camera" (`--v4l2-sink`), which every Linux app sees as a webcam.
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::util::{hidden, Res};
use crate::{adb, tools};

pub const CAMERA_NAME: &str = "Android Tools Camera";

#[derive(Deserialize, Clone)]
pub struct Settings {
    pub serial: String,
    pub sdk: u32,
    pub facing: String,
    pub camera_id: String,
    pub quality: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub mode: String,
    pub torch: bool,
    pub live: Live,
}

#[derive(Deserialize, Serialize, Clone, Copy, Default)]
pub struct Live {
    pub rotation: u32,
    pub mirror: bool,
    pub fill: bool,
    pub preview: bool,
}

#[derive(Serialize, Clone)]
struct Status {
    state: &'static str,
    text: String,
    fps: f32,
    native: bool,
}

/// `/dev/videoN` of the loopback device created for Android Tools.
pub fn find_device() -> Option<PathBuf> {
    let entries = std::fs::read_dir("/sys/class/video4linux").ok()?;
    for e in entries.flatten() {
        let name = std::fs::read_to_string(e.path().join("name")).unwrap_or_default();
        if name.trim() == CAMERA_NAME {
            return Some(PathBuf::from("/dev").join(e.file_name()));
        }
    }
    None
}

pub struct Camera {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Camera {
    pub fn start(app: AppHandle, s: Settings) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let (stop2, child2) = (stop.clone(), child.clone());
        let thread = std::thread::spawn(move || run(app, s, stop2, child2));
        Camera { stop, child, thread: Some(thread) }
    }

    /// Orientation changes need a restart on Linux; the UI does that.
    pub fn update_live(&self, _live: Live) {}

    pub fn finished(&self) -> bool {
        self.thread.as_ref().map(|t| t.is_finished()).unwrap_or(true)
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(c) = self.child.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn orientation(live: &Live) -> Option<String> {
    let rot = live.rotation % 360;
    if rot == 0 && !live.mirror {
        return None;
    }
    Some(format!("{}{rot}", if live.mirror { "flip" } else { "" }))
}

fn run(app: AppHandle, s: Settings, stop: Arc<AtomicBool>, child_slot: Arc<Mutex<Option<Child>>>) {
    let native = s.mode == "native" || (s.mode == "auto" && s.sdk >= 31);
    let status = |state: &'static str, text: &str| {
        let _ = app.emit("camera-status", Status { state, text: text.into(), fps: 0.0, native });
    };
    let error = |text: String| {
        let _ = app.emit("camera-error", text);
    };

    let result: Res<()> = (|| {
        status("connecting", "Подключение…");
        tools::ensure()?;
        let device = find_device().ok_or("Камера не установлена")?;
        let exe = tools::scrcpy_exe().ok_or("scrcpy не найден")?;
        if !native {
            let _ = adb::keyevent(&s.serial, "wake");
            std::thread::sleep(Duration::from_millis(400));
            if adb::is_locked(&s.serial) {
                error("Разблокируйте телефон — камера откроется поверх экрана блокировки".into());
            }
            adb::open_camera_app(&s.serial, s.facing == "front")?;
            std::thread::sleep(Duration::from_millis(1200));
        }

        let long_side = match s.quality {
            480 => 848,
            1080 => 1920,
            _ => 1280,
        };
        let mut args = vec![
            "-s".to_string(),
            s.serial.clone(),
            format!("--v4l2-sink={}", device.display()),
            "--no-playback".into(),
            "--no-audio".into(),
            "--no-control".into(),
            "--video-codec=h264".into(),
            format!("--video-bit-rate={}M", s.bitrate.max(1)),
            format!("--max-fps={}", s.fps),
            format!("--max-size={long_side}"),
        ];
        if native {
            args.push("--video-source=camera".into());
            if s.camera_id.is_empty() {
                args.push(format!("--camera-facing={}", s.facing));
            } else {
                args.push(format!("--camera-id={}", s.camera_id));
            }
            args.push(format!("--camera-fps={}", s.fps));
            if s.torch {
                args.push("--camera-torch".into());
            }
        }
        if let Some(o) = orientation(&s.live) {
            args.push(format!("--capture-orientation={o}"));
        }

        let mut child = hidden(&exe)
            .args(&args)
            .env("ADB", tools::adb_exe())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
                     child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)]
            .into_iter()
            .flatten()
        {
            let log = log.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    let mut l = log.lock().unwrap();
                    l.push(line);
                    let n = l.len();
                    if n > 40 {
                        l.drain(..n - 40);
                    }
                }
            });
        }
        *child_slot.lock().unwrap() = Some(child);

        let started = Instant::now();
        let mut announced = false;
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            let exited = matches!(child_slot.lock().unwrap().as_mut().map(|c| c.try_wait()), Some(Ok(Some(_))));
            if exited {
                std::thread::sleep(Duration::from_millis(200));
                let l = log.lock().unwrap();
                let msg = l
                    .iter()
                    .rev()
                    .find(|x| x.contains("ERROR"))
                    .map(|x| {
                        if x.contains("not supported before Android 12") {
                            "Прямой доступ к камере — только Android 12+. Выберите режим «Совм.»".to_string()
                        } else {
                            x.split("ERROR:").last().unwrap_or(x).trim().to_string()
                        }
                    })
                    .unwrap_or_else(|| "Поток прерван".into());
                return Err(msg);
            }
            if !announced && started.elapsed() > Duration::from_secs(2) {
                announced = true;
                status("running", "Трансляция");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    })();

    if let Err(e) = result {
        if !stop.load(Ordering::SeqCst) {
            error(e);
        }
    }
    if let Some(mut c) = child_slot.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    status("stopped", "");
}

pub mod install {
    //! v4l2loopback setup through polkit (`pkexec`), once.
    use serde::Serialize;

    use super::{find_device, CAMERA_NAME};
    use crate::util::{hidden, Res};

    #[derive(Serialize)]
    pub struct CameraStatus {
        pub kind: &'static str,
        pub supported: bool,
        pub registered: bool,
        pub device: bool,
        pub service_disabled: bool,
    }

    fn module_available() -> bool {
        std::path::Path::new("/sys/module/v4l2loopback").exists()
            || hidden("modinfo").arg("v4l2loopback").output().map(|o| o.status.success()).unwrap_or(false)
    }

    pub fn status() -> CameraStatus {
        let device = find_device().is_some();
        CameraStatus { kind: "v4l2", supported: module_available(), registered: device, device, service_disabled: false }
    }

    #[allow(dead_code)]
    pub fn handle_cli(_args: &[String]) -> Option<i32> {
        None
    }

    fn install_script() -> String {
        let options = format!("devices=1 exclusive_caps=1 card_label=\"{CAMERA_NAME}\"");
        format!(
            r#"set -e
modinfo v4l2loopback >/dev/null 2>&1 || exit 3
if lsmod | grep -q '^v4l2loopback'; then
  v4l2loopback-ctl add -n "{CAMERA_NAME}" -x 1 >/dev/null 2>&1 || {{ modprobe -r v4l2loopback && modprobe v4l2loopback {options}; }}
else
  modprobe v4l2loopback {options}
fi
echo v4l2loopback > /etc/modules-load.d/android-tools-gui.conf
if ! grep -rqs v4l2loopback /etc/modprobe.d/ ; then
  echo 'options v4l2loopback {options}' > /etc/modprobe.d/android-tools-gui.conf
fi"#
        )
    }

    fn remove_script() -> String {
        format!(
            r#"for d in /sys/class/video4linux/*; do
  if [ "$(cat "$d/name" 2>/dev/null)" = "{CAMERA_NAME}" ]; then
    v4l2loopback-ctl delete "/dev/$(basename "$d")" >/dev/null 2>&1 || modprobe -r v4l2loopback || true
  fi
done
rm -f /etc/modules-load.d/android-tools-gui.conf /etc/modprobe.d/android-tools-gui.conf"#
        )
    }

    pub fn run_elevated(flag: &str) -> Res<()> {
        let script = if flag == "--camera-remove" { remove_script() } else { install_script() };
        let out = hidden("pkexec").args(["sh", "-c", &script]).output().map_err(|e| format!("pkexec: {e}"))?;
        match out.status.code() {
            Some(0) => Ok(()),
            Some(3) => Err("Нужен пакет v4l2loopback-dkms".into()),
            Some(126) | Some(127) => Err("Отменено".into()),
            _ => {
                let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(if msg.is_empty() { format!("Код ошибки {}", out.status.code().unwrap_or(-1)) } else { msg })
            }
        }
    }
}
