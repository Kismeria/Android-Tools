//! Sound of the screen mirror on Android 10. scrcpy forwards audio only on Android 11+, so on
//! Android 10 the sndcpy helper app (github.com/rom1v/sndcpy, by the scrcpy author) captures
//! the playback with AudioPlaybackCapture and streams raw PCM (s16le, 48 kHz, stereo) over an
//! adb forward; the PC plays it on the default output device.
use std::io::{Cursor, Read};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::adb;
use crate::util::{data_dir, err, Res};

const ZIP_URL: &str = "https://github.com/rom1v/sndcpy/releases/download/v1.1/sndcpy-v1.1.zip";
const PACKAGE: &str = "com.rom1v.sndcpy";

pub struct Sndcpy {
    serial: String,
    port: u16,
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn apk() -> Res<PathBuf> {
    let path = data_dir().join("sndcpy.apk");
    if path.is_file() {
        return Ok(path);
    }
    let mut zip = Vec::new();
    ureq::get(ZIP_URL)
        .set("User-Agent", "AndroidTools")
        .call()
        .map_err(|e| format!("Загрузка sndcpy: {e}"))?
        .into_reader()
        .take(20 * 1024 * 1024)
        .read_to_end(&mut zip)
        .map_err(err)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(zip)).map_err(err)?;
    let mut entry = archive.by_name("sndcpy.apk").map_err(|_| "В архиве нет sndcpy.apk".to_string())?;
    let mut data = Vec::new();
    entry.read_to_end(&mut data).map_err(err)?;
    std::fs::create_dir_all(data_dir()).map_err(err)?;
    std::fs::write(&path, data).map_err(err)?;
    Ok(path)
}

fn free_port() -> Res<u16> {
    TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).map_err(err)
}

impl Sndcpy {
    /// Installs the helper if needed, starts capture and plays it in a background thread.
    pub fn start(serial: &str) -> Res<Self> {
        let installed = adb::shell_lenient(serial, &format!("pm path {PACKAGE}")).unwrap_or_default().contains("package:");
        if !installed {
            let apk = apk()?;
            adb::run(Some(serial), &["install", "-t", "-r", "-g", &apk.display().to_string()], 120)
                .map_err(|e| format!("Установка sndcpy: {e}"))?;
        }
        // Allow screen-audio capture without the confirmation dialog.
        let _ = adb::shell_lenient(serial, &format!("appops set {PACKAGE} PROJECT_MEDIA allow"));
        let port = free_port()?;
        adb::run(Some(serial), &["forward", &format!("tcp:{port}"), "localabstract:sndcpy"], 10)?;
        adb::shell_lenient(serial, &format!("am start {PACKAGE}/.MainActivity"))?;

        let stop = Arc::new(AtomicBool::new(false));
        let socket: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));
        let (stop2, socket2) = (stop.clone(), socket.clone());
        let thread = std::thread::spawn(move || play(port, stop2, socket2));
        Ok(Sndcpy { serial: serial.to_string(), port, stop, socket, thread: Some(thread) })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(s) = self.socket.lock().unwrap().as_ref() {
            let _ = s.shutdown(Shutdown::Both);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = adb::run_lenient(Some(&self.serial), &["forward", "--remove", &format!("tcp:{}", self.port)], 5);
        let _ = adb::shell_lenient(&self.serial, &format!("am force-stop {PACKAGE}"));
    }
}

fn play(port: u16, stop: Arc<AtomicBool>, socket: Arc<Mutex<Option<TcpStream>>>) {
    // The app needs a moment to start its capture service; the forward accepts connections
    // before that and closes them, so retry until audio actually flows.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buf = vec![0u8; 16 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    let mut samples: Vec<i16> = Vec::new();
    while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
            std::thread::sleep(Duration::from_millis(300));
            continue;
        };
        stream.set_nodelay(true).ok();
        *socket.lock().unwrap() = stream.try_clone().ok();
        let mut out: Option<crate::mic::sys::Monitor> = None;
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 || stop.load(Ordering::SeqCst) {
                break;
            }
            if out.is_none() {
                match crate::mic::sys::Monitor::open(120) {
                    Ok(m) => out = Some(m),
                    Err(_) => return,
                }
            }
            // Whole stereo frames (4 bytes) only; the remainder waits for the next read.
            pending.extend_from_slice(&buf[..n]);
            let whole = pending.len() / 4 * 4;
            samples.clear();
            samples.extend(pending[..whole].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
            pending.drain(..whole);
            if let Some(m) = out.as_mut() {
                m.write(&samples);
            }
        }
        if stop.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}
