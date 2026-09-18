//! Phone camera → "Android Tools Camera".
//! scrcpy-server streams H.264 over an adb reverse tunnel; frames are decoded with Media
//! Foundation, transformed, and published to shared memory for the camera DLL.
pub mod decoder;
pub mod install;
pub mod output;
pub mod transform;

use std::io::{BufRead, BufReader, Read};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::util::{hidden, Res};
use crate::{adb, tools};
use output::SharedFrame;
use transform::{Canvas, Options};

const REMOTE_SERVER: &str = "/data/local/tmp/atools-scrcpy-server.jar";

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
    state: &'static str, // connecting | running | stopped
    text: String,
    fps: f32,
    native: bool,
}

pub struct Camera {
    stop: Arc<AtomicBool>,
    live: Arc<Mutex<Live>>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    server: Arc<Mutex<Option<Child>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Camera {
    pub fn start(app: AppHandle, s: Settings) -> Self {
        let cam = Camera {
            stop: Arc::new(AtomicBool::new(false)),
            live: Arc::new(Mutex::new(s.live)),
            socket: Arc::new(Mutex::new(None)),
            server: Arc::new(Mutex::new(None)),
            thread: None,
        };
        let ctx = Ctx {
            app,
            stop: cam.stop.clone(),
            live: cam.live.clone(),
            socket: cam.socket.clone(),
            server: cam.server.clone(),
            native: s.mode == "native" || (s.mode == "auto" && s.sdk >= 31),
            s,
        };
        let handle = std::thread::spawn(move || ctx.run());
        Camera { thread: Some(handle), ..cam }
    }

    pub fn update_live(&self, live: Live) {
        *self.live.lock().unwrap() = live;
    }

    pub fn finished(&self) -> bool {
        self.thread.as_ref().map(|t| t.is_finished()).unwrap_or(true)
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(sock) = self.socket.lock().unwrap().as_ref() {
            let _ = sock.shutdown(Shutdown::Both);
        }
        if let Some(child) = self.server.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Ctx {
    app: AppHandle,
    s: Settings,
    native: bool,
    stop: Arc<AtomicBool>,
    live: Arc<Mutex<Live>>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    server: Arc<Mutex<Option<Child>>>,
}

impl Ctx {
    fn status(&self, state: &'static str, text: impl Into<String>, fps: f32) {
        let _ = self.app.emit("camera-status", Status { state, text: text.into(), fps, native: self.native });
    }

    fn error(&self, text: impl Into<String>) {
        let _ = self.app.emit("camera-error", text.into());
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn run(self) {
        let reverse = format!("localabstract:scrcpy_{:08x}", rand::random::<u32>() & 0x7fff_ffff);
        let result = self.pipeline(&reverse);
        if let Err(e) = result {
            if !self.stopped() {
                self.error(e);
            }
        }
        if let Some(mut child) = self.server.lock().unwrap().take() {
            let _ = child.kill();
        }
        let _ = adb::run_lenient(Some(&self.s.serial), &["reverse", "--remove", &reverse], 5);
        self.status("stopped", "", 0.0);
    }

    fn size(&self) -> (u32, u32) {
        match self.s.quality {
            480 => (854, 480),
            1080 => (1920, 1080),
            _ => (1280, 720),
        }
    }

    fn server_args(&self, scid: &str) -> Vec<String> {
        let (w, h) = self.size();
        let mut a = vec![
            tools::scrcpy_version(),
            format!("scid={scid}"),
            "log_level=info".into(),
            "tunnel_forward=false".into(),
            "audio=false".into(),
            "control=false".into(),
            "cleanup=true".into(),
            "send_device_meta=false".into(),
            "send_dummy_byte=false".into(),
            "send_stream_meta=true".into(),
            "send_frame_meta=true".into(),
            "video_codec=h264".into(),
            format!("video_bit_rate={}", self.s.bitrate.max(1) * 1_000_000),
            format!("max_fps={}", self.s.fps),
            format!("max_size={}", w.max(h)),
        ];
        if self.native {
            a.push("video_source=camera".into());
            if self.s.camera_id.is_empty() {
                a.push(format!("camera_facing={}", self.s.facing));
            } else {
                a.push(format!("camera_id={}", self.s.camera_id));
            }
            a.push(format!("camera_fps={}", self.s.fps));
            if self.s.torch {
                a.push("camera_torch=true".into());
            }
        }
        a
    }

    fn pipeline(&self, reverse: &str) -> Res<()> {
        let serial = self.s.serial.clone();
        self.status("connecting", "Подключение…", 0.0);
        tools::ensure()?;
        let server = tools::scrcpy_server().ok_or("scrcpy не найден")?;
        adb::run(Some(&serial), &["push", &server.display().to_string(), REMOTE_SERVER], 30)?;

        if !self.native {
            let _ = adb::keyevent(&serial, "wake");
            std::thread::sleep(Duration::from_millis(400));
            if adb::is_locked(&serial) {
                self.error("Разблокируйте телефон — камера откроется поверх экрана блокировки");
            }
            adb::open_camera_app(&serial, self.s.facing == "front")?;
            std::thread::sleep(Duration::from_millis(1200));
        }

        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().unwrap().port();
        let scid = &reverse[reverse.len() - 8..];
        adb::run(Some(&serial), &["reverse", reverse, &format!("tcp:{port}")], 10)?;

        let mut cmd = hidden(tools::adb_exe());
        cmd.args(["-s", &serial, "shell", &format!("CLASSPATH={REMOTE_SERVER}"), "app_process", "/",
                  "com.genymobile.scrcpy.Server"])
            .args(self.server_args(scid))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
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
        *self.server.lock().unwrap() = Some(child);

        let server_error = || {
            let l = log.lock().unwrap();
            l.iter().rev().find(|x| x.contains("ERROR") || x.contains("Exception")).map(|x| {
                if x.contains("not supported before Android 12") {
                    "Прямой доступ к камере — только Android 12+. Выберите режим «Совм.»".to_string()
                } else {
                    x.split("ERROR:").last().unwrap_or(x).trim().to_string()
                }
            })
        };

        let deadline = Instant::now() + Duration::from_secs(15);
        let stream = loop {
            if self.stopped() {
                return Ok(());
            }
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    let dead = matches!(self.server.lock().unwrap().as_mut().map(|c| c.try_wait()), Some(Ok(Some(_))));
                    if dead || Instant::now() > deadline {
                        std::thread::sleep(Duration::from_millis(300));
                        return Err(server_error().unwrap_or_else(|| "Сервер камеры не ответил".into()));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.to_string()),
            }
        };
        stream.set_nonblocking(false).ok();
        stream.set_nodelay(true).ok();
        *self.socket.lock().unwrap() = Some(stream.try_clone().map_err(|e| e.to_string())?);

        let result = self.receive(stream);
        if self.stopped() {
            return Ok(());
        }
        result.and_then(|_| Err(server_error().unwrap_or_else(|| "Поток прерван".into())))
    }

    fn receive(&self, mut stream: TcpStream) -> Res<()> {
        let (out_w, out_h) = self.size();
        let (max_w, max_h) = SharedFrame::max_size();
        let (out_w, out_h) = (out_w.min(max_w), out_h.min(max_h));

        // Heartbeat + lazy (re)open of the shared frame, independent of frame arrival.
        let shared: Arc<Mutex<Option<SharedFrame>>> = Arc::new(Mutex::new(SharedFrame::open()));
        let decoded = Arc::new(AtomicU32::new(0));
        {
            let (shared, stop, decoded) = (shared.clone(), self.stop.clone(), decoded.clone());
            let app = self.app.clone();
            let native = self.native;
            std::thread::spawn(move || {
                let mut last = decoded.load(Ordering::Relaxed);
                let mut tick = Instant::now();
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                    {
                        let mut g = shared.lock().unwrap();
                        match g.as_ref() {
                            Some(f) => f.touch(),
                            None => *g = SharedFrame::open(),
                        }
                    }
                    if tick.elapsed() >= Duration::from_secs(1) {
                        let now = decoded.load(Ordering::Relaxed);
                        let fps = (now - last) as f32 / tick.elapsed().as_secs_f32();
                        last = now;
                        tick = Instant::now();
                        let _ = app.emit("camera-status", Status { state: "running", text: "Трансляция".into(), fps, native });
                    }
                }
            });
        }

        let mut decoder = decoder::Decoder::new().map_err(|e| format!("Декодер H.264: {e}"))?;
        let mut canvas = Canvas::new(out_w, out_h);
        let mut codec = [0u8; 4];
        stream.read_exact(&mut codec).map_err(|e| e.to_string())?;
        if &codec != b"h264" {
            return Err("Неожиданный кодек".into());
        }
        self.status("running", "Трансляция", 0.0);

        let mut header = [0u8; 12];
        let mut config: Vec<u8> = Vec::new();
        let mut packet = Vec::new();
        let mut last_preview = Instant::now() - Duration::from_secs(1);
        while !self.stopped() {
            stream.read_exact(&mut header).map_err(|e| e.to_string())?;
            let first = u64::from_be_bytes(header[..8].try_into().unwrap());
            if first >> 63 == 1 {
                continue; // session packet: resolution change, the decoder adapts on its own
            }
            let len = u32::from_be_bytes(header[8..].try_into().unwrap()) as usize;
            packet.resize(len, 0);
            stream.read_exact(&mut packet).map_err(|e| e.to_string())?;
            if first & (1 << 62) != 0 {
                config = packet.clone(); // SPS/PPS, prepended to the next picture
                continue;
            }
            let pts = (first & ((1u64 << 61) - 1)) as i64;
            let mut au = std::mem::take(&mut config);
            au.extend_from_slice(&packet);

            let live = *self.live.lock().unwrap();
            let opts = Options { rotation: live.rotation, mirror: live.mirror, fill: live.fill };
            let mut produced = false;
            decoder
                .decode(&au, pts, &mut |frame| {
                    canvas.draw(frame, opts);
                    produced = true;
                })
                .map_err(|e| format!("Декодирование: {e}"))?;
            if !produced {
                continue;
            }
            decoded.fetch_add(1, Ordering::Relaxed);
            if let Some(f) = shared.lock().unwrap().as_ref() {
                f.write(out_w, out_h, &canvas.data);
            }
            if live.preview && last_preview.elapsed() >= Duration::from_millis(80) {
                last_preview = Instant::now();
                let (rgb, w, h) = canvas.preview_rgb(640);
                let mut jpeg = Vec::new();
                let enc = jpeg_encoder::Encoder::new(&mut jpeg, 72);
                if enc.encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb).is_ok() {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);
                    let _ = self.app.emit("camera-frame", format!("data:image/jpeg;base64,{b64}"));
                }
            }
        }
        Ok(())
    }
}
