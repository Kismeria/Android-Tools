//! Phone microphone → "Android Tools Microphone".
//! scrcpy-server captures the microphone as raw PCM (16-bit LE, 48 kHz, stereo) over an adb
//! reverse tunnel; the samples get gain, noise gate and limiter, then go to the system
//! microphone device (Windows: VB-CABLE, Linux: PipeWire/PulseAudio pipe source) and,
//! optionally, to the PC speakers.
#[cfg_attr(windows, path = "win.rs")]
#[cfg_attr(not(windows), path = "linux.rs")]
pub mod sys;

use std::io::{BufRead, BufReader, Read};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::util::{hidden, Res};
use crate::{adb, tools};

const REMOTE_SERVER: &str = "/data/local/tmp/atools-mic-server.jar";
pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
const CODEC_RAW: u32 = 0x0072_6177;

#[derive(Deserialize, Clone)]
pub struct Settings {
    pub serial: String,
    pub sdk: u32,
    /// scrcpy audio source: mic, mic-unprocessed, mic-voice-communication, mic-camcorder, mic-voice-recognition
    pub source: String,
    /// Target latency of the output device in milliseconds.
    pub buffer: u32,
    pub live: Live,
}

/// Settings applied without restarting the stream.
#[derive(Deserialize, Serialize, Clone, Copy)]
pub struct Live {
    /// Percent, 100 = unchanged.
    pub gain: u32,
    pub mute: bool,
    pub gate: bool,
    /// Gate threshold in dBFS (negative).
    pub gate_db: i32,
    pub limiter: bool,
    pub mono: bool,
    pub monitor: bool,
    /// Percent.
    pub monitor_volume: u32,
}

#[derive(Serialize, Clone)]
struct Status {
    state: &'static str, // connecting | running | stopped
    text: String,
    device: bool,
}

#[derive(Serialize, Clone)]
struct Level {
    peak: f32,
    rms: f32,
    open: bool,
}

pub struct Mic {
    stop: Arc<AtomicBool>,
    live: Arc<Mutex<Live>>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    server: Arc<Mutex<Option<Child>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Mic {
    pub fn start(app: AppHandle, s: Settings) -> Self {
        let mic = Mic {
            stop: Arc::new(AtomicBool::new(false)),
            live: Arc::new(Mutex::new(s.live)),
            socket: Arc::new(Mutex::new(None)),
            server: Arc::new(Mutex::new(None)),
            thread: None,
        };
        let ctx = Ctx {
            app,
            stop: mic.stop.clone(),
            live: mic.live.clone(),
            socket: mic.socket.clone(),
            server: mic.server.clone(),
            s,
        };
        let handle = std::thread::spawn(move || ctx.run());
        Mic { thread: Some(handle), ..mic }
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
    stop: Arc<AtomicBool>,
    live: Arc<Mutex<Live>>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    server: Arc<Mutex<Option<Child>>>,
}

impl Ctx {
    fn status(&self, state: &'static str, text: impl Into<String>, device: bool) {
        let _ = self.app.emit("mic-status", Status { state, text: text.into(), device });
    }

    fn error(&self, text: impl Into<String>) {
        let _ = self.app.emit("mic-error", text.into());
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn run(self) {
        let reverse = format!("localabstract:scrcpy_{:08x}", rand::random::<u32>() & 0x7fff_ffff);
        if let Err(e) = self.pipeline(&reverse) {
            if !self.stopped() {
                self.error(e);
            }
        }
        if let Some(mut child) = self.server.lock().unwrap().take() {
            let _ = child.kill();
        }
        let _ = adb::run_lenient(Some(&self.s.serial), &["reverse", "--remove", &reverse], 5);
        self.status("stopped", "", false);
    }

    fn server_args(&self, scid: &str) -> Vec<String> {
        vec![
            tools::scrcpy_version(),
            format!("scid={scid}"),
            "log_level=info".into(),
            "tunnel_forward=false".into(),
            "video=false".into(),
            "audio=true".into(),
            "control=false".into(),
            "cleanup=true".into(),
            "send_device_meta=false".into(),
            "send_dummy_byte=false".into(),
            "send_stream_meta=true".into(),
            "send_frame_meta=true".into(),
            "audio_codec=raw".into(),
            format!("audio_source={}", self.s.source),
        ]
    }

    fn pipeline(&self, reverse: &str) -> Res<()> {
        let serial = self.s.serial.clone();
        self.status("connecting", "Подключение…", false);
        if self.s.sdk != 0 && self.s.sdk < 30 {
            return Err("Микрофон телефона доступен с Android 11".into());
        }
        tools::ensure()?;
        let server = tools::scrcpy_server().ok_or("scrcpy не найден")?;
        adb::run(Some(&serial), &["push", &server.display().to_string(), REMOTE_SERVER], 30)?;
        if self.s.sdk == 30 {
            // Android 11 starts microphone capture only while the screen is unlocked.
            let _ = adb::keyevent(&serial, "wake");
            if adb::is_locked(&serial) {
                self.error("Разблокируйте телефон — Android 11 даёт доступ к микрофону только на разблокированном экране");
            }
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
            l.iter()
                .rev()
                .find(|x| x.contains("ERROR") || x.contains("Exception"))
                .map(|x| x.split("ERROR:").last().unwrap_or(x).trim().to_string())
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
                        return Err(server_error().unwrap_or_else(|| "Телефон не ответил".into()));
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
        let mut codec = [0u8; 4];
        stream.read_exact(&mut codec).map_err(|e| e.to_string())?;
        match u32::from_be_bytes(codec) {
            CODEC_RAW => {}
            0 => return Err("Телефон не разрешил запись с микрофона (нужен Android 11+)".into()),
            1 => return Err("Не удалось запустить запись на телефоне".into()),
            _ => return Err("Неожиданный кодек".into()),
        }

        let mut output = match sys::Output::open(self.s.buffer) {
            Ok(o) => Some(o),
            Err(e) => {
                self.error(e);
                None
            }
        };
        let has_device = output.is_some();
        let mut monitor: Option<sys::Monitor> = None;
        let mut monitor_failed = false;
        self.status("running", "Трансляция", has_device);

        let mut dsp = Dsp::default();
        let mut header = [0u8; 12];
        let mut packet = Vec::new();
        let mut samples: Vec<i16> = Vec::new();
        let mut monitor_buf: Vec<i16> = Vec::new();
        let mut meter = Meter::default();
        let mut last_level = Instant::now();
        while !self.stopped() {
            stream.read_exact(&mut header).map_err(|e| e.to_string())?;
            let first = u64::from_be_bytes(header[..8].try_into().unwrap());
            let len = u32::from_be_bytes(header[8..].try_into().unwrap()) as usize;
            packet.resize(len, 0);
            stream.read_exact(&mut packet).map_err(|e| e.to_string())?;
            if first >> 63 == 1 || first & (1 << 62) != 0 {
                continue; // session or config packet: raw PCM needs neither
            }
            samples.clear();
            samples.extend(packet.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));

            let live = *self.live.lock().unwrap();
            dsp.process(&mut samples, &live);
            meter.feed(&samples);

            if let Some(out) = output.as_mut() {
                out.write(&samples);
            }
            if live.monitor && !monitor_failed {
                if monitor.is_none() {
                    match sys::Monitor::open(60) {
                        Ok(m) => monitor = Some(m),
                        Err(e) => {
                            monitor_failed = true;
                            self.error(format!("Прослушивание: {e}"));
                        }
                    }
                }
                if let Some(m) = monitor.as_mut() {
                    let k = live.monitor_volume.min(200) as f32 / 100.0;
                    monitor_buf.clear();
                    monitor_buf.extend(samples.iter().map(|&x| (x as f32 * k).clamp(-32768.0, 32767.0) as i16));
                    m.write(&monitor_buf);
                }
            } else if !live.monitor {
                monitor = None;
                monitor_failed = false;
            }

            if last_level.elapsed() >= Duration::from_millis(50) {
                last_level = Instant::now();
                let (peak, rms) = meter.take();
                let _ = self.app.emit("mic-level", Level { peak, rms, open: dsp.gate_open });
            }
        }
        Ok(())
    }
}

/// Gain → noise gate → limiter, on interleaved stereo 16-bit samples.
struct Dsp {
    env: f32,
    hold: u32,
    gate_open: bool,
}

impl Default for Dsp {
    fn default() -> Self {
        Dsp { env: 1.0, hold: 0, gate_open: true }
    }
}

impl Dsp {
    fn process(&mut self, buf: &mut [i16], live: &Live) {
        if live.mute {
            buf.fill(0);
            self.gate_open = false;
            return;
        }
        let gain = live.gain.min(1000) as f32 / 100.0;
        let frames = buf.len() / CHANNELS as usize;
        if frames == 0 {
            return;
        }

        // Gate decision per packet (≈10–20 ms) on the input level, with a short hold.
        let target = if live.gate {
            let sum: f64 = buf.iter().map(|&x| (x as f64 / 32768.0).powi(2)).sum();
            let rms = (sum / buf.len() as f64).sqrt() as f32 * gain;
            let db = 20.0 * rms.max(1e-9).log10();
            if db >= live.gate_db as f32 {
                self.hold = RATE / 5; // 200 ms
            } else {
                self.hold = self.hold.saturating_sub(frames as u32);
            }
            if self.hold > 0 { 1.0 } else { 0.0 }
        } else {
            1.0
        };
        self.gate_open = target > 0.5;
        let attack = 1.0 - (-1.0 / (0.004 * RATE as f32)).exp();
        let release = 1.0 - (-1.0 / (0.120 * RATE as f32)).exp();

        for frame in buf.chunks_exact_mut(CHANNELS as usize) {
            let coef = if target > self.env { attack } else { release };
            self.env += (target - self.env) * coef;
            let (mut l, mut r) = (frame[0] as f32 / 32768.0, frame[1] as f32 / 32768.0);
            if live.mono {
                let m = (l + r) * 0.5;
                l = m;
                r = m;
            }
            let k = gain * self.env;
            frame[0] = to_i16(shape(l * k, live.limiter));
            frame[1] = to_i16(shape(r * k, live.limiter));
        }
    }
}

/// Soft knee above 0.8 instead of hard clipping.
fn shape(x: f32, limiter: bool) -> f32 {
    if !limiter {
        return x.clamp(-1.0, 1.0);
    }
    const T: f32 = 0.8;
    let a = x.abs();
    if a <= T {
        x
    } else {
        x.signum() * (T + (1.0 - T) * ((a - T) / (1.0 - T)).tanh())
    }
}

fn to_i16(x: f32) -> i16 {
    (x * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}

#[derive(Default)]
struct Meter {
    peak: f32,
    sum: f64,
    count: usize,
}

impl Meter {
    fn feed(&mut self, buf: &[i16]) {
        for &x in buf {
            let v = x as f32 / 32768.0;
            self.peak = self.peak.max(v.abs());
            self.sum += (v as f64) * (v as f64);
        }
        self.count += buf.len();
    }

    fn take(&mut self) -> (f32, f32) {
        let rms = if self.count > 0 { (self.sum / self.count as f64).sqrt() as f32 } else { 0.0 };
        let peak = self.peak;
        *self = Meter::default();
        (peak, rms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> Live {
        Live { gain: 100, mute: false, gate: false, gate_db: -50, limiter: false, mono: false, monitor: false, monitor_volume: 100 }
    }

    #[test]
    fn gain_scales_samples() {
        let mut buf = vec![1000i16, -1000, 2000, -2000];
        Dsp::default().process(&mut buf, &Live { gain: 200, ..live() });
        assert_eq!(buf, vec![2000, -2000, 4000, -4000]);
    }

    #[test]
    fn mute_outputs_silence() {
        let mut buf = vec![12345i16; 64];
        Dsp::default().process(&mut buf, &Live { mute: true, ..live() });
        assert!(buf.iter().all(|&x| x == 0));
    }

    #[test]
    fn limiter_rounds_off_instead_of_clipping() {
        // 16000 × 250 % ≈ 1.22 full scale: clipped flat without the limiter, just under full scale with it.
        let mut clipped = vec![16000i16, -16000];
        Dsp::default().process(&mut clipped, &Live { gain: 250, ..live() });
        assert_eq!(clipped, vec![32767, -32767]);
        let mut soft = vec![16000i16, -16000];
        Dsp::default().process(&mut soft, &Live { gain: 250, limiter: true, ..live() });
        assert!(soft[0] > 32000 && soft[0] < 32767, "{}", soft[0]);
        assert_eq!(soft[1], -soft[0]);
    }

    #[test]
    fn gate_silences_quiet_input_after_hold() {
        let mut dsp = Dsp::default();
        let l = Live { gate: true, gate_db: -30, ..live() };
        // -60 dBFS noise for 1 s: the gate closes and fades the signal out.
        let mut last = Vec::new();
        for _ in 0..100 {
            let mut buf = vec![32i16; 960];
            dsp.process(&mut buf, &l);
            last = buf;
        }
        assert!(!dsp.gate_open);
        assert!(last.iter().all(|&x| x == 0));
        // Speech at -6 dBFS opens it again within one 10 ms packet (4 ms attack).
        let mut buf = vec![16000i16; 960];
        dsp.process(&mut buf, &l);
        assert!(dsp.gate_open);
        assert!(buf[958] > 14000, "{}", buf[958]);
    }

    #[test]
    fn mono_averages_channels() {
        let mut buf = vec![1000i16, 3000];
        Dsp::default().process(&mut buf, &Live { mono: true, ..live() });
        assert_eq!(buf, vec![2000, 2000]);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "plays 0.2 s of silence on the default output device"]
    fn wasapi_monitor_and_status() {
        let st = sys::status();
        println!("kind={} device={} name={}", st.kind, st.device, st.name);
        let mut m = sys::Monitor::open(60).expect("monitor");
        let silence = vec![0i16; 960 * 2];
        for _ in 0..20 {
            m.write(&silence);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
