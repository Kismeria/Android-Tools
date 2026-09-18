//! Windows 10 camera: a DirectShow source filter (softcam, see `softcam/`).
//! Windows 10 has no MFCreateVirtualCamera, but DirectShow apps (Zoom, Discord, Teams, OBS,
//! Chrome, Edge, Telegram…) list registered video-input filters as cameras.
use std::fs;
use std::path::PathBuf;

use windows::core::{s, HSTRING, PCSTR};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};

use crate::util::{hidden, program_data_dir, Res};

static SOFTCAM64: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/softcam64.dll"));
static SOFTCAM32: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/softcam32.dll"));

const VIDEO_INPUT_CATEGORY: &str = "{860BB310-5D01-11d0-BD3B-00A0C911CE86}";

fn dir() -> PathBuf {
    program_data_dir().join("Camera").join("dshow")
}

fn dll_path(bits: u32) -> PathBuf {
    dir().join(format!("AndroidToolsCam{bits}.dll"))
}

fn system_dir(bits: u32) -> PathBuf {
    let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    windir.join(if bits == 32 { "SysWOW64" } else { "System32" })
}

/// A loaded DLL cannot be overwritten, but it can be renamed away first.
fn write(target: &PathBuf, data: &[u8]) -> Res<()> {
    if fs::read(target).map(|d| d == data).unwrap_or(false) {
        return Ok(());
    }
    if fs::write(target, data).is_err() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        fs::rename(target, target.with_extension(format!("{stamp}.old"))).map_err(|e| e.to_string())?;
        fs::write(target, data).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn regsvr32(bits: u32, dll: &PathBuf, unregister: bool) -> Res<()> {
    let exe = system_dir(bits).join("regsvr32.exe");
    if !exe.is_file() {
        return Ok(()); // 32-bit subsystem absent: nothing to register
    }
    let mut cmd = hidden(exe);
    cmd.arg("/s");
    if unregister {
        cmd.arg("/u");
    }
    let status = cmd.arg(dll).status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("regsvr32 ({bits}-бит): код {}", status.code().unwrap_or(-1)))
    }
}

/// Elevated: copy both builds and register them (64-bit for modern apps, 32-bit for older ones).
pub fn install() -> Res<()> {
    fs::create_dir_all(dir()).map_err(|e| format!("{}: {e}", dir().display()))?;
    for (bits, data) in [(64, SOFTCAM64), (32, SOFTCAM32)] {
        let path = dll_path(bits);
        write(&path, data)?;
        regsvr32(bits, &path, false)?;
    }
    if let Ok(entries) = fs::read_dir(dir()) {
        for e in entries.flatten() {
            if e.path().extension().map(|x| x == "old").unwrap_or(false) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

pub fn remove() -> Res<()> {
    for bits in [64, 32] {
        let path = dll_path(bits);
        if path.is_file() {
            let _ = regsvr32(bits, &path, true);
        }
    }
    Ok(())
}

pub fn registered() -> bool {
    // softcam names the category instance key after the filter, not after its CLSID.
    let key = HSTRING::from(format!("SOFTWARE\\Classes\\CLSID\\{VIDEO_INPUT_CATEGORY}\\Instance\\Android Tools Camera"));
    unsafe {
        let mut h = HKEY::default();
        let ok = RegOpenKeyExW(HKEY_LOCAL_MACHINE, &key, None, KEY_READ | KEY_WOW64_64KEY, &mut h).is_ok();
        if ok {
            let _ = RegCloseKey(h);
        }
        ok
    }
}

// ── sender ──

type CreateFn = unsafe extern "C" fn(i32, i32, f32) -> *mut core::ffi::c_void;
type DeleteFn = unsafe extern "C" fn(*mut core::ffi::c_void);
type SendFn = unsafe extern "C" fn(*mut core::ffi::c_void, *const u8);

/// Publishes BGR frames to the DirectShow camera through the installed 64-bit DLL.
pub struct Sender {
    _lib: HMODULE,
    camera: *mut core::ffi::c_void,
    delete: DeleteFn,
    send: SendFn,
    pub width: u32,
    pub height: u32,
}

unsafe impl Send for Sender {}

impl Sender {
    pub fn open(width: u32, height: u32, fps: u32) -> Res<Self> {
        let path = dll_path(64);
        if !path.is_file() {
            return Err("Камера не установлена".into());
        }
        unsafe {
            let lib = LoadLibraryW(&HSTRING::from(path.as_os_str())).map_err(|e| e.message())?;
            let get = |name: PCSTR| GetProcAddress(lib, name).ok_or("softcam: нет функции");
            let create: CreateFn = std::mem::transmute(get(s!("scCreateCamera"))?);
            let delete: DeleteFn = std::mem::transmute(get(s!("scDeleteCamera"))?);
            let send: SendFn = std::mem::transmute(get(s!("scSendFrame"))?);
            // Fixed rate: scSendFrame paces delivery, and a static picture keeps flowing.
            let camera = create(width as i32, height as i32, fps.max(1) as f32);
            if camera.is_null() {
                return Err("Камера занята другой программой Android Tools".into());
            }
            Ok(Sender { _lib: lib, camera, delete, send, width, height })
        }
    }

    /// `bgr` is top-down, `width * height * 3` bytes.
    pub fn send(&self, bgr: &[u8]) {
        if bgr.len() >= (self.width * self.height * 3) as usize {
            unsafe { (self.send)(self.camera, bgr.as_ptr()) }
        }
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        unsafe { (self.delete)(self.camera) }
    }
}
