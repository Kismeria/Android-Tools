//! Writer side of the shared-memory frame contract read by AndroidToolsCam.dll (vcam/shared.h).
use std::sync::atomic::{AtomicU64, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, INVALID_HANDLE_VALUE};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::*;
use windows::Win32::System::SystemInformation::GetTickCount64;

const MAGIC: u32 = 0x4D43_5441;
const HEADER: usize = 64;
const MAX_W: u32 = 1920;
const MAX_H: u32 = 1080;
const MAP_SIZE: usize = HEADER + (MAX_W * MAX_H * 3 / 2) as usize;

pub struct SharedFrame {
    map: HANDLE,
    view: *mut u8,
    counter: AtomicU64,
}

unsafe impl Send for SharedFrame {}
unsafe impl Sync for SharedFrame {}

impl SharedFrame {
    /// Create the mapping (or open the one the camera DLL created). Fails until one side exists.
    pub fn open() -> Option<Self> {
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let have_sd = ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:(A;;GA;;;SY)(A;;GA;;;LS)(A;;GA;;;BA)(A;;GRGW;;;AU)"),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .is_ok();
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            let name = w!("Global\\AndroidToolsCamFrame");
            let created = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                if have_sd { Some(&sa) } else { None },
                PAGE_READWRITE,
                0,
                MAP_SIZE as u32,
                name,
            );
            if have_sd {
                let _ = LocalFree(Some(HLOCAL(sd.0)));
            }
            let map = match created {
                Ok(h) => h,
                Err(_) => OpenFileMappingW((FILE_MAP_READ | FILE_MAP_WRITE).0, false, name).ok()?,
            };
            let view = MapViewOfFile(map, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, MAP_SIZE);
            if view.Value.is_null() {
                let _ = CloseHandle(map);
                return None;
            }
            let ptr = view.Value as *mut u8;
            let counter = std::ptr::read_unaligned(ptr.add(16) as *const u64) & !1;
            Some(SharedFrame { map, view: ptr, counter: AtomicU64::new(counter) })
        }
    }

    pub fn write(&self, width: u32, height: u32, nv12: &[u8]) {
        if width > MAX_W || height > MAX_H || nv12.len() < (width * height * 3 / 2) as usize {
            return;
        }
        unsafe {
            let c = self.counter.fetch_add(2, Ordering::SeqCst) + 1;
            std::ptr::write_unaligned(self.view.add(16) as *mut u64, c); // odd: writing
            std::ptr::write_unaligned(self.view as *mut u32, MAGIC);
            std::ptr::write_unaligned(self.view.add(4) as *mut u32, 1);
            std::ptr::write_unaligned(self.view.add(8) as *mut u32, width);
            std::ptr::write_unaligned(self.view.add(12) as *mut u32, height);
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), self.view.add(HEADER), (width * height * 3 / 2) as usize);
            std::ptr::write_unaligned(self.view.add(16) as *mut u64, c + 1);
            self.touch();
        }
    }

    /// Keep the frame "live" for the DLL while the picture is static.
    pub fn touch(&self) {
        unsafe { std::ptr::write_unaligned(self.view.add(24) as *mut u64, GetTickCount64()) }
    }

    pub fn max_size() -> (u32, u32) {
        (MAX_W, MAX_H)
    }
}

impl Drop for SharedFrame {
    fn drop(&mut self) {
        unsafe {
            std::ptr::write_unaligned(self.view.add(24) as *mut u64, 0);
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as _ });
            let _ = CloseHandle(self.map);
        }
    }
}
