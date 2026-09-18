//! Registering "Android Tools Camera" as a Windows camera device (MFCreateVirtualCamera).
//! Install/remove need admin rights: the exe relaunches itself elevated with a CLI flag.
use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use windows::core::{s, w, Interface, HSTRING, PCWSTR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, ERROR_CANCELLED};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Services::*;
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

use crate::util::{program_data_dir, Res};

static CAMERA_DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/AndroidToolsCam.dll"));

const CLSID_STR: &str = "{6B1F0C2A-8E4D-4F7B-A3C5-2D9E7F10B4A6}";
const CAMERA_NAME: &str = "Android Tools Camera";
const SERVICES: [&str; 2] = ["FrameServer", "FrameServerMonitor"];

fn camera_dir() -> PathBuf {
    program_data_dir().join("Camera")
}

fn log_path() -> PathBuf {
    camera_dir().join("last-result.txt")
}

fn clsid_key() -> HSTRING {
    HSTRING::from(format!("SOFTWARE\\Classes\\CLSID\\{CLSID_STR}"))
}

/// Handles `--camera-install` / `--camera-remove` when started elevated. Returns an exit code.
pub fn handle_cli(args: &[String]) -> Option<i32> {
    let action = args.iter().find(|a| a.starts_with("--camera-"))?;
    let result = match action.as_str() {
        "--camera-install" => install(),
        "--camera-remove" => remove(),
        _ => return None,
    };
    let _ = fs::create_dir_all(camera_dir());
    let _ = fs::write(log_path(), result.as_ref().err().cloned().unwrap_or_default());
    Some(if result.is_ok() { 0 } else { 1 })
}

fn install() -> Res<()> {
    if !supported() {
        return Err(UNSUPPORTED.into());
    }
    enable_services()?;
    let dll = write_dll()?;
    register_clsid(&dll)?;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| format!("MFStartup: {e}"))?;
        let cam = create_virtual_camera()?;
        let started = cam.Start(None);
        let _ = MFShutdown();
        started.map_err(|e| format!("Запуск камеры: {e}"))?;
    }
    Ok(())
}

fn remove() -> Res<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| e.to_string())?;
        if let Ok(cam) = create_virtual_camera() {
            let _ = cam.Remove();
        }
        let _ = MFShutdown();
        let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, &clsid_key());
    }
    Ok(())
}

fn enable_services() -> Res<()> {
    unsafe {
        let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
            .map_err(|e| format!("Службы: {e}"))?;
        for name in SERVICES {
            if let Ok(svc) = OpenServiceW(scm, &HSTRING::from(name), SERVICE_CHANGE_CONFIG | SERVICE_QUERY_CONFIG) {
                if service_start_type(svc) == Some(SERVICE_DISABLED.0) {
                    ChangeServiceConfigW(
                        svc,
                        ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                        SERVICE_DEMAND_START,
                        SERVICE_ERROR(SERVICE_NO_CHANGE),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        None,
                        PCWSTR::null(),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        PCWSTR::null(),
                    )
                    .map_err(|e| format!("Не удалось включить {name}: {e}"))?;
                }
                let _ = CloseServiceHandle(svc);
            }
        }
        let _ = CloseServiceHandle(scm);
    }
    Ok(())
}

unsafe fn service_start_type(svc: SC_HANDLE) -> Option<u32> {
    let mut needed = 0u32;
    let _ = QueryServiceConfigW(svc, None, 0, &mut needed);
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    let cfg = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
    QueryServiceConfigW(svc, Some(cfg), needed, &mut needed).ok()?;
    Some((*cfg).dwStartType.0)
}

/// Copies the DLL where the Frame Server (LocalService) can read it. A loaded DLL cannot be
/// overwritten, but it can be renamed away first.
fn write_dll() -> Res<PathBuf> {
    let dir = camera_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let target = dir.join("AndroidToolsCam.dll");
    if fs::read(&target).map(|d| d == CAMERA_DLL).unwrap_or(false) {
        return Ok(target);
    }
    if fs::write(&target, CAMERA_DLL).is_err() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        fs::rename(&target, dir.join(format!("AndroidToolsCam.{stamp}.old"))).map_err(|e| e.to_string())?;
        fs::write(&target, CAMERA_DLL).map_err(|e| e.to_string())?;
    }
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.path().extension().map(|x| x == "old").unwrap_or(false) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    Ok(target)
}

fn register_clsid(dll: &std::path::Path) -> Res<()> {
    unsafe {
        let set = |key: &HSTRING, name: PCWSTR, value: &str| -> Res<()> {
            let mut h = HKEY::default();
            let r = RegCreateKeyExW(HKEY_LOCAL_MACHINE, key, None, PCWSTR::null(), REG_OPTION_NON_VOLATILE,
                                    KEY_WRITE, None, &mut h, None);
            if r.is_err() {
                return Err(format!("Реестр: {}", r.to_hresult().message()));
            }
            let data: Vec<u8> = value.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect();
            let r = RegSetValueExW(h, name, None, REG_SZ, Some(&data));
            let _ = RegCloseKey(h);
            if r.is_err() { Err(format!("Реестр: {}", r.to_hresult().message())) } else { Ok(()) }
        };
        let base = format!("SOFTWARE\\Classes\\CLSID\\{CLSID_STR}");
        set(&HSTRING::from(base.as_str()), PCWSTR::null(), CAMERA_NAME)?;
        let inproc = HSTRING::from(format!("{base}\\InprocServer32"));
        set(&inproc, PCWSTR::null(), &dll.display().to_string())?;
        set(&inproc, w!("ThreadingModel"), "Both")?;
    }
    Ok(())
}

/// MFCreateVirtualCamera exists only on Windows 11 (build 22000+). It is resolved at run time:
/// a static import would stop the whole exe from starting on Windows 10.
type CreateVirtualCameraFn = unsafe extern "system" fn(
    i32, i32, i32, PCWSTR, PCWSTR, *const windows::core::GUID, u32, *mut *mut core::ffi::c_void,
) -> windows::core::HRESULT;

fn create_virtual_camera_fn() -> Option<CreateVirtualCameraFn> {
    unsafe {
        let lib = LoadLibraryW(w!("mfsensorgroup.dll")).ok()?;
        let proc = GetProcAddress(lib, s!("MFCreateVirtualCamera"))?;
        Some(std::mem::transmute::<_, CreateVirtualCameraFn>(proc))
    }
}

pub fn supported() -> bool {
    create_virtual_camera_fn().is_some()
}

fn create_virtual_camera() -> Res<IMFVirtualCamera> {
    let create = create_virtual_camera_fn().ok_or(UNSUPPORTED)?;
    let name = HSTRING::from(CAMERA_NAME);
    let id = HSTRING::from(CLSID_STR);
    unsafe {
        let mut raw = std::ptr::null_mut();
        create(
            MFVirtualCameraType_SoftwareCameraSource.0,
            MFVirtualCameraLifetime_System.0,
            MFVirtualCameraAccess_AllUsers.0,
            PCWSTR(name.as_ptr()),
            PCWSTR(id.as_ptr()),
            std::ptr::null(),
            0,
            &mut raw,
        )
        .ok()
        .map_err(|e| format!("Создание камеры: {e}"))?;
        Ok(IMFVirtualCamera::from_raw(raw))
    }
}

const UNSUPPORTED: &str = "Системная камера доступна только в Windows 11";

// ── non-elevated side ──

#[derive(Serialize)]
pub struct CameraStatus {
    pub supported: bool,
    pub registered: bool,
    pub device: bool,
    pub service_disabled: bool,
}

pub fn status() -> CameraStatus {
    unsafe {
        let mut h = HKEY::default();
        let key = HSTRING::from(format!("SOFTWARE\\Classes\\CLSID\\{CLSID_STR}\\InprocServer32"));
        let registered = RegOpenKeyExW(HKEY_LOCAL_MACHINE, &key, None, KEY_READ, &mut h).is_ok();
        if registered {
            let _ = RegCloseKey(h);
        }
        let mut service_disabled = false;
        if let Ok(scm) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) {
            if let Ok(svc) = OpenServiceW(scm, w!("FrameServer"), SERVICE_QUERY_CONFIG) {
                service_disabled = service_start_type(svc) == Some(SERVICE_DISABLED.0);
                let _ = CloseServiceHandle(svc);
            }
            let _ = CloseServiceHandle(scm);
        }
        CameraStatus { supported: supported(), registered, device: registered && device_present(), service_disabled }
    }
}

fn device_present() -> bool {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).is_err() {
            return false;
        }
        let mut found = false;
        let mut attrs = None;
        if MFCreateAttributes(&mut attrs, 1).is_ok() {
            let attrs = attrs.unwrap();
            let _ = attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID);
            let mut list = std::ptr::null_mut();
            let mut count = 0u32;
            if MFEnumDeviceSources(&attrs, &mut list, &mut count).is_ok() && !list.is_null() {
                for i in 0..count as usize {
                    if let Some(act) = (*list.add(i)).take() {
                        let mut len = 0u32;
                        let mut pwstr = windows::core::PWSTR::null();
                        if act.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut pwstr, &mut len).is_ok() {
                            if pwstr.to_string().unwrap_or_default().contains(CAMERA_NAME) {
                                found = true;
                            }
                            CoTaskMemFree(Some(pwstr.0 as _));
                        }
                    }
                }
                CoTaskMemFree(Some(list as _));
            }
        }
        let _ = MFShutdown();
        found
    }
}

/// Relaunch this exe elevated with `flag`; waits and returns the error text on failure.
pub fn run_elevated(flag: &str) -> Res<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(flag);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == ERROR_CANCELLED.to_hresult() {
                return Err("Отменено".into());
            }
            return Err(e.message());
        }
        if info.hProcess.is_invalid() {
            return Err("Не удалось запустить установщик".into());
        }
        if WaitForSingleObject(info.hProcess, INFINITE) != WAIT_OBJECT_0 {
            let _ = CloseHandle(info.hProcess);
            return Err("Установщик не ответил".into());
        }
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if code == 0 {
            Ok(())
        } else {
            let msg = fs::read_to_string(log_path()).unwrap_or_default();
            Err(if msg.trim().is_empty() { format!("Код ошибки {code}") } else { msg })
        }
    }
}
