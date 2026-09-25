//! Windows: the microphone device is VB-CABLE (a signed virtual audio cable by VB-Audio,
//! downloaded from vb-audio.com on install). Audio played into its render endpoint comes out of
//! its capture endpoint, which is renamed to "Android Tools Microphone".
//! Install/remove need admin rights: the exe relaunches itself elevated with a CLI flag.
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::core::{Interface, GUID, HSTRING, IUnknown, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::{PKEY_DeviceInterface_FriendlyName, PKEY_Device_DeviceDesc, PKEY_Device_FriendlyName};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ, STGM_READWRITE};

use super::{CHANNELS, RATE};
use crate::util::{data_dir, err, program_data_dir, Res};

const CABLE: &str = "VB-Audio Virtual Cable";
const PACK_URL: &str = "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip";
const SETUP: &str = "VBCABLE_Setup_x64.exe";
pub const MIC_NAME: &str = "Android Tools Microphone";
const FEED_NAME: &str = "Android Tools Mic Feed";

fn com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn enumerator() -> Res<IMMDeviceEnumerator> {
    com();
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("Звук Windows: {e}")) }
}

fn prop(device: &IMMDevice, key: &windows::Win32::Foundation::PROPERTYKEY) -> String {
    unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .and_then(|store| store.GetValue(key))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}

fn device_id(device: &IMMDevice) -> String {
    unsafe {
        match device.GetId() {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as _));
                s
            }
            Err(_) => String::new(),
        }
    }
}

/// The VB-CABLE endpoint of the given direction (render = feed, capture = microphone).
fn cable(flow: EDataFlow) -> Option<IMMDevice> {
    let en = enumerator().ok()?;
    unsafe {
        let list = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE).ok()?;
        for i in 0..list.GetCount().ok()? {
            if let Ok(d) = list.Item(i) {
                if prop(&d, &PKEY_DeviceInterface_FriendlyName) == CABLE {
                    return Some(d);
                }
            }
        }
    }
    None
}

// ── playback into an endpoint (WASAPI shared mode) ──

struct Player {
    client: IAudioClient,
    render: IAudioRenderClient,
    frames: u32,
    target: u32,
    started: bool,
}

impl Player {
    fn open(device: &IMMDevice, buffer_ms: u32) -> Res<Self> {
        let buffer_ms = buffer_ms.clamp(10, 500);
        unsafe {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(err)?;
            let wfx = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM as u16,
                nChannels: CHANNELS as u16,
                nSamplesPerSec: RATE,
                nAvgBytesPerSec: RATE * CHANNELS * 2,
                nBlockAlign: (CHANNELS * 2) as u16,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            // Room for twice the target latency; the converter adapts the format to the device.
            let hns = buffer_ms as i64 * 2 * 10_000;
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    hns,
                    0,
                    &wfx,
                    None,
                )
                .map_err(err)?;
            let frames = client.GetBufferSize().map_err(err)?;
            let render: IAudioRenderClient = client.GetService().map_err(err)?;
            let target = (RATE * buffer_ms / 1000).min(frames);
            Ok(Player { client, render, frames, target, started: false })
        }
    }

    fn write(&mut self, samples: &[i16]) {
        let n = (samples.len() / CHANNELS as usize) as u32;
        if n == 0 {
            return;
        }
        unsafe {
            let padding = self.client.GetCurrentPadding().unwrap_or(0);
            // Too much queued (the phone ran ahead of the PC clock): drop to keep latency low.
            if padding + n > self.frames || padding > self.target {
                return;
            }
            if let Ok(ptr) = self.render.GetBuffer(n) {
                std::ptr::copy_nonoverlapping(samples.as_ptr() as *const u8, ptr, (n * CHANNELS * 2) as usize);
                let _ = self.render.ReleaseBuffer(n, 0);
            }
            if !self.started {
                self.started = self.client.Start().is_ok();
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// The system microphone device.
pub struct Output(Player);

impl Output {
    pub fn open(buffer_ms: u32) -> Res<Self> {
        com();
        let dev = cable(eRender).ok_or("Микрофон Android Tools не установлен — работает только прослушивание")?;
        Player::open(&dev, buffer_ms).map(Output).map_err(|e| format!("Микрофон Android Tools: {e}"))
    }

    pub fn write(&mut self, samples: &[i16]) {
        self.0.write(samples);
    }
}

/// Listening on the PC speakers (default playback device).
pub struct Monitor(Player);

impl Monitor {
    pub fn open(buffer_ms: u32) -> Res<Self> {
        let en = enumerator()?;
        let dev = unsafe { en.GetDefaultAudioEndpoint(eRender, eConsole).map_err(err)? };
        Player::open(&dev, buffer_ms).map(Monitor)
    }

    pub fn write(&mut self, samples: &[i16]) {
        self.0.write(samples);
    }
}

// ── status ──

#[derive(Serialize)]
pub struct MicStatus {
    /// "vbcable" on Windows, "pulse" on Linux.
    pub kind: &'static str,
    pub supported: bool,
    pub device: bool,
    /// Name of the microphone as apps show it.
    pub name: String,
}

pub fn status() -> MicStatus {
    let name = cable(eCapture).map(|d| prop(&d, &PKEY_Device_FriendlyName));
    MicStatus { kind: "vbcable", supported: true, device: name.is_some(), name: name.unwrap_or_default() }
}

// ── install / remove (non-elevated side) ──

fn pack_dir() -> PathBuf {
    data_dir().join("VBCABLE")
}

fn download_pack() -> Res<PathBuf> {
    let dir = pack_dir();
    if dir.join(SETUP).is_file() {
        return Ok(dir);
    }
    let data = crate::tools::download(PACK_URL).map_err(|e| format!("Загрузка VB-CABLE: {e}"))?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(err)?;
    zip::ZipArchive::new(std::io::Cursor::new(data)).and_then(|mut z| z.extract(&dir)).map_err(err)?;
    if !dir.join(SETUP).is_file() {
        return Err(format!("В архиве нет {SETUP}"));
    }
    Ok(dir)
}

pub fn install() -> Res<()> {
    let dir = if cable(eCapture).is_some() { pack_dir() } else { download_pack()? };
    crate::camera::install::run_elevated(&format!("--mic-install \"{}\"", dir.display()))
}

pub fn remove() -> Res<()> {
    let dir = if marker().is_file() { download_pack()? } else { pack_dir() };
    crate::camera::install::run_elevated(&format!("--mic-remove \"{}\"", dir.display()))
}

// ── elevated side ──

fn marker() -> PathBuf {
    program_data_dir().join("Mic").join("installed-vbcable")
}

/// Handles `--mic-install <dir>` / `--mic-remove <dir>`. Returns an exit code.
pub fn handle_cli(args: &[String]) -> Option<i32> {
    let pos = args.iter().position(|a| a == "--mic-install" || a == "--mic-remove")?;
    let dir = PathBuf::from(args.get(pos + 1).cloned().unwrap_or_default());
    com();
    let defaults = Defaults::save();
    let result = if args[pos] == "--mic-install" { elevated_install(&dir) } else { elevated_remove(&dir) };
    defaults.restore();
    crate::camera::install::save_result(&result);
    Some(if result.is_ok() { 0 } else { 1 })
}

fn run_setup(dir: &Path, flag: &str) -> Res<()> {
    let exe = dir.join(SETUP);
    if !exe.is_file() {
        return Err(format!("Нет {}", exe.display()));
    }
    let mut child = std::process::Command::new(&exe).args([flag, "-h"]).current_dir(dir).spawn().map_err(err)?;
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            return Ok(());
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err("Установщик VB-CABLE не ответил".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn wait_for(present: bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if cable(eCapture).is_some() == present {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

fn elevated_install(dir: &Path) -> Res<()> {
    if cable(eCapture).is_none() {
        run_setup(dir, "-i")?;
        if !wait_for(true) {
            return Err("VB-CABLE не появился в системе. Перезагрузите компьютер и нажмите «Установить» ещё раз.".into());
        }
        let m = marker();
        let _ = std::fs::create_dir_all(m.parent().unwrap());
        let _ = std::fs::write(&m, "1");
    }
    rename(eCapture, MIC_NAME)?;
    let _ = rename(eRender, FEED_NAME);
    Ok(())
}

fn elevated_remove(dir: &Path) -> Res<()> {
    if marker().is_file() {
        run_setup(dir, "-u")?;
        wait_for(false);
        let _ = std::fs::remove_file(marker());
    } else {
        let _ = rename(eCapture, "CABLE Output");
        let _ = rename(eRender, "CABLE Input");
    }
    Ok(())
}

fn rename(flow: EDataFlow, name: &str) -> Res<()> {
    let dev = cable(flow).ok_or("VB-CABLE не найден")?;
    unsafe {
        let store = dev.OpenPropertyStore(STGM_READWRITE).map_err(|e| format!("Имя устройства: {e}"))?;
        store.SetValue(&PKEY_Device_DeviceDesc, &PROPVARIANT::from(name)).map_err(|e| format!("Имя устройства: {e}"))?;
        store.Commit().map_err(|e| format!("Имя устройства: {e}"))?;
    }
    Ok(())
}

/// Default playback/recording devices. Installing a new audio driver may make it the default;
/// the user's devices are put back afterwards.
struct Defaults(Vec<(EDataFlow, ERole, String)>);

impl Defaults {
    fn save() -> Self {
        let mut v = Vec::new();
        if let Ok(en) = enumerator() {
            for flow in [eRender, eCapture] {
                for role in [eConsole, eMultimedia, eCommunications] {
                    if let Ok(d) = unsafe { en.GetDefaultAudioEndpoint(flow, role) } {
                        v.push((flow, role, device_id(&d)));
                    }
                }
            }
        }
        Defaults(v)
    }

    fn restore(&self) {
        let Ok(en) = enumerator() else { return };
        for (flow, role, id) in &self.0 {
            let current = unsafe { en.GetDefaultAudioEndpoint(*flow, *role) }.map(|d| device_id(&d)).unwrap_or_default();
            if &current != id && !id.is_empty() {
                let _ = set_default_endpoint(id, *role);
            }
        }
    }
}

/// IPolicyConfig::SetDefaultEndpoint — the interface the Sound control panel uses.
fn set_default_endpoint(id: &str, role: ERole) -> Res<()> {
    const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    type SetDefault = unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> windows::core::HRESULT;
    unsafe {
        let unk: IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL).map_err(err)?;
        let mut raw = std::ptr::null_mut();
        unk.query(&IID_POLICY_CONFIG, &mut raw).ok().map_err(err)?;
        let policy = IUnknown::from_raw(raw);
        let vtable = *(raw as *const *const usize);
        let f: SetDefault = std::mem::transmute(*vtable.add(13));
        let wide = HSTRING::from(id);
        let hr = f(raw, PCWSTR(wide.as_ptr()), role.0);
        drop(policy);
        hr.ok().map_err(err)
    }
}
