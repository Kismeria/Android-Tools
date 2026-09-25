//! Phone camera → "Android Tools Camera".
//! Android: scrcpy-server streams H.264 over an adb reverse tunnel; frames are decoded with
//! Media Foundation. iPhone: JPEG frames from Safari (crate::iphone). Either way the picture is
//! transformed and published to shared memory for the camera DLL (or to the DirectShow camera).
pub mod decoder;
pub mod dshow;
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
use decoder::Nv12;
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
    /// "android" (default) or "iphone".
    #[serde(default)]
    pub source: String,
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
    /// native | compat | iphone
    source: &'static str,
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
        let handle = if ctx.s.source == "iphone" {
            std::thread::spawn(move || ctx.run_iphone())
        } else {
            std::thread::spawn(move || ctx.run())
        };
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
        let source = if self.s.source == "iphone" { "iphone" } else if self.native { "native" } else { "compat" };
        let _ = self.app.emit("camera-status", Status { state, text: text.into(), fps, native: self.native, source });
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
        size_for(self.s.quality)
    }

    /// iPhone: frames arrive from the Safari page through crate::iphone.
    fn run_iphone(self) {
        self.status("connecting", "Ждём iPhone…", 0.0);
        if !crate::iphone::connected() {
            self.error("iPhone не подключён: откройте ссылку или QR-код на вкладке «Девайсы»");
            self.status("stopped", "", 0.0);
            return;
        }
        let (w, h) = self.size();
        let mut output = Output::open(self.app.clone(), self.stop.clone(), (w, h), self.s.fps, "iphone");
        let live = self.live.clone();
        let mut jpeg = Jpeg::default();
        crate::iphone::set_sink(Some(Box::new(move |data: &[u8]| {
            if let Some(frame) = jpeg.decode(data) {
                output.push(&frame, *live.lock().unwrap());
            }
        })));
        let (qw, qh) = match self.s.quality { 480 => (640, 480), 1080 => (1920, 1080), _ => (1280, 720) };
        crate::iphone::send(serde_json::json!({
            "cmd": "start", "facing": self.s.facing, "width": qw, "height": qh, "fps": self.s.fps,
            "quality": if self.s.bitrate >= 10 { 0.85 } else if self.s.bitrate >= 5 { 0.75 } else { 0.6 },
            "torch": self.s.torch,
        }));
        while !self.stopped() {
            if !crate::iphone::connected() {
                self.error("iPhone отключился");
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        crate::iphone::set_sink(None);
        crate::iphone::send(serde_json::json!({ "cmd": "stop" }));
        self.status("stopped", "", 0.0);
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
        let source = if self.native { "native" } else { "compat" };
        let mut output = Output::open(self.app.clone(), self.stop.clone(), self.size(), self.s.fps, source);
        let mut decoder = decoder::Decoder::new().map_err(|e| format!("Декодер H.264: {e}"))?;
        let mut codec = [0u8; 4];
        stream.read_exact(&mut codec).map_err(|e| e.to_string())?;
        if &codec != b"h264" {
            return Err("Неожиданный кодек".into());
        }
        self.status("running", "Трансляция", 0.0);

        let mut header = [0u8; 12];
        let mut config: Vec<u8> = Vec::new();
        let mut packet = Vec::new();
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
            decoder
                .decode(&au, pts, &mut |frame| output.push(frame, live))
                .map_err(|e| format!("Декодирование: {e}"))?;
        }
        Ok(())
    }
}

fn size_for(quality: u32) -> (u32, u32) {
    let (w, h) = match quality {
        480 => (848, 480), // DirectShow camera needs multiples of 4
        1080 => (1920, 1080),
        _ => (1280, 720),
    };
    let (max_w, max_h) = SharedFrame::max_size();
    (w.min(max_w), h.min(max_h))
}

/// Where transformed frames go: the system camera (Windows 11 shared memory or the Windows 10
/// DirectShow camera), the UI preview, and the fps counter.
struct Output {
    app: AppHandle,
    canvas: Canvas,
    shared: Arc<Mutex<Option<SharedFrame>>>,
    latest_bgr: Option<Arc<Mutex<Option<Vec<u8>>>>>,
    decoded: Arc<AtomicU32>,
    last_preview: Instant,
}

impl Output {
    fn open(app: AppHandle, stop: Arc<AtomicBool>, (out_w, out_h): (u32, u32), fps: u32, source: &'static str) -> Self {
        // Windows 10: DirectShow camera. Windows 11: Media Foundation camera via shared memory.
        let dshow_mode = install::use_dshow();
        // The DirectShow camera is fed from its own thread at a fixed rate with the latest
        // picture, so apps keep receiving frames while the phone sends none (static image).
        let mut latest_bgr = None;
        if dshow_mode {
            match dshow::Sender::open(out_w, out_h, fps) {
                Ok(sender) => {
                    let latest: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
                    latest_bgr = Some(latest.clone());
                    let stop = stop.clone();
                    std::thread::spawn(move || {
                        let mut frame = vec![0u8; (sender.width * sender.height * 3) as usize];
                        while !stop.load(Ordering::SeqCst) {
                            if let Some(new) = latest.lock().unwrap().take() {
                                frame = new;
                            }
                            sender.send(&frame);
                        }
                    });
                }
                Err(e) => {
                    let _ = app.emit("camera-error", format!("Камера Windows: {e}"));
                }
            }
        }

        // Heartbeat + lazy (re)open of the shared frame, independent of frame arrival.
        let shared: Arc<Mutex<Option<SharedFrame>>> =
            Arc::new(Mutex::new(if dshow_mode { None } else { SharedFrame::open() }));
        let decoded = Arc::new(AtomicU32::new(0));
        {
            let (shared, decoded, app) = (shared.clone(), decoded.clone(), app.clone());
            std::thread::spawn(move || {
                let mut last = decoded.load(Ordering::Relaxed);
                let mut tick = Instant::now();
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                    if !dshow_mode {
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
                        let _ = app.emit("camera-status", Status {
                            state: "running", text: "Трансляция".into(), fps, native: source == "native", source,
                        });
                    }
                }
            });
        }
        let _ = app.emit("camera-status", Status { state: "running", text: "Трансляция".into(), fps: 0.0, native: source == "native", source });
        Output { app, canvas: Canvas::new(out_w, out_h), shared, latest_bgr, decoded, last_preview: Instant::now() - Duration::from_secs(1) }
    }

    fn push(&mut self, frame: &Nv12, live: Live) {
        self.canvas.draw(frame, Options { rotation: live.rotation, mirror: live.mirror, fill: live.fill });
        self.decoded.fetch_add(1, Ordering::Relaxed);
        let (w, h) = (self.canvas.width, self.canvas.height);
        if let Some(f) = self.shared.lock().unwrap().as_ref() {
            f.write(w, h, &self.canvas.data);
        }
        if let Some(latest) = &self.latest_bgr {
            let mut bgr = Vec::new();
            self.canvas.to_bgr(&mut bgr);
            *latest.lock().unwrap() = Some(bgr);
        }
        if live.preview && self.last_preview.elapsed() >= Duration::from_millis(80) {
            self.last_preview = Instant::now();
            let (rgb, pw, ph) = self.canvas.preview_rgb(640);
            let mut jpeg = Vec::new();
            let enc = jpeg_encoder::Encoder::new(&mut jpeg, 72);
            if enc.encode(&rgb, pw as u16, ph as u16, jpeg_encoder::ColorType::Rgb).is_ok() {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);
                let _ = self.app.emit("camera-frame", format!("data:image/jpeg;base64,{b64}"));
            }
        }
    }
}

/// JPEG from Safari → NV12 (video range) for the canvas.
#[derive(Default)]
struct Jpeg {
    nv12: Vec<u8>,
    width: u32,
    height: u32,
}

impl Jpeg {
    fn decode(&mut self, data: &[u8]) -> Option<Nv12<'_>> {
        use zune_jpeg::zune_core::colorspace::ColorSpace;
        use zune_jpeg::zune_core::options::DecoderOptions;
        let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::YCbCr);
        let mut dec = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(data), opts);
        let ycc = dec.decode().ok()?;
        let (w, h) = dec.dimensions()?;
        let (w, h) = (w & !1, h & !1);
        if w == 0 || h == 0 {
            return None;
        }
        let stride = dec.dimensions()?.0;
        self.nv12.resize(w * h * 3 / 2, 0);
        // Full-range JPEG YCbCr → video-range NV12 (4:2:0).
        let luma = |v: u8| (16 + (v as u32 * 219 + 127) / 255) as u8;
        let chroma = |v: u32| (128 + ((v as i32 / 4 - 128) * 224 + 127) / 255) as u8;
        let (yp, uvp) = self.nv12.split_at_mut(w * h);
        for y in 0..h {
            let row = &ycc[y * stride * 3..];
            for x in 0..w {
                yp[y * w + x] = luma(row[x * 3]);
            }
        }
        for y in 0..h / 2 {
            let (r0, r1) = (&ycc[(2 * y) * stride * 3..], &ycc[(2 * y + 1) * stride * 3..]);
            for x in 0..w / 2 {
                let (a, b) = (x * 6, x * 6 + 3);
                let cb = r0[a + 1] as u32 + r0[b + 1] as u32 + r1[a + 1] as u32 + r1[b + 1] as u32;
                let cr = r0[a + 2] as u32 + r0[b + 2] as u32 + r1[a + 2] as u32 + r1[b + 2] as u32;
                uvp[y * w + x * 2] = chroma(cb);
                uvp[y * w + x * 2 + 1] = chroma(cr);
            }
        }
        self.width = w as u32;
        self.height = h as u32;
        Some(Nv12 { data: &self.nv12, width: self.width, height: self.height, stride: self.width, uv_offset: w * h })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_to_video_range_nv12() {
        // Solid red 64×48 → BT.601 video range: Y≈81, U≈90, V≈240.
        let rgb: Vec<u8> = (0..64 * 48).flat_map(|_| [255u8, 0, 0]).collect();
        let mut jpeg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg, 95).encode(&rgb, 64, 48, jpeg_encoder::ColorType::Rgb).unwrap();
        let mut dec = Jpeg::default();
        let f = dec.decode(&jpeg).expect("decoded");
        assert_eq!((f.width, f.height, f.stride, f.uv_offset), (64, 48, 64, 64 * 48));
        let (y, u, v) = (f.data[100] as i32, f.data[f.uv_offset + 10] as i32, f.data[f.uv_offset + 11] as i32);
        assert!((y - 81).abs() <= 4 && (u - 90).abs() <= 4 && (v - 240).abs() <= 4, "y={y} u={u} v={v}");
    }

    #[test]
    fn bad_jpeg_is_skipped() {
        assert!(Jpeg::default().decode(b"not a jpeg").is_none());
    }
}
