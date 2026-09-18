//! H.264 → NV12 using the Windows Media Foundation decoder MFT (hardware-agnostic, all profiles).
use std::mem::ManuallyDrop;

use windows::core::{Interface, Result as WinResult};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

/// A decoded NV12 picture borrowed from the decoder's output buffer.
pub struct Nv12<'a> {
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// Byte offset of the interleaved UV plane.
    pub uv_offset: usize,
}

pub struct Decoder {
    mft: IMFTransform,
    width: u32,
    height: u32,
    stride: u32,
    buffer_size: u32,
    provides_samples: bool,
}

impl Decoder {
    pub fn new() -> WinResult<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
            if let Ok(attrs) = mft.GetAttributes() {
                let _ = attrs.SetUINT32(&CODECAPI_AVLowLatencyMode, 1);
            }
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            mft.SetInputType(0, &input, 0)?;
            let mut dec = Decoder { mft, width: 0, height: 0, stride: 0, buffer_size: 0, provides_samples: false };
            dec.configure_output()?;
            dec.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            dec.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(dec)
        }
    }

    unsafe fn configure_output(&mut self) -> WinResult<()> {
        let mut index = 0;
        loop {
            let t = self.mft.GetOutputAvailableType(0, index)?;
            if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                self.mft.SetOutputType(0, &t, 0)?;
                let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                self.width = (size >> 32) as u32;
                self.height = size as u32;
                // Coded size is padded to macroblocks; the aperture is the visible picture.
                let mut area = [0u8; 16];
                if t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut area, None).is_ok() {
                    let w = i32::from_le_bytes(area[8..12].try_into().unwrap());
                    let h = i32::from_le_bytes(area[12..16].try_into().unwrap());
                    if w > 0 && h > 0 && (w as u32) <= self.width && (h as u32) <= self.height {
                        self.width = w as u32;
                        self.height = h as u32;
                    }
                }
                self.stride = t.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| (s as i32).unsigned_abs()).unwrap_or(self.width);
                if self.stride == 0 {
                    self.stride = self.width;
                }
                let info = self.mft.GetOutputStreamInfo(0)?;
                self.buffer_size = info.cbSize.max(self.stride * self.height.max(1) * 3 / 2);
                self.provides_samples = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
                return Ok(());
            }
            index += 1;
        }
    }

    /// Feed one access unit (Annex B) and invoke `on_frame` for every picture that comes out.
    pub fn decode(&mut self, au: &[u8], pts_us: i64, on_frame: &mut dyn FnMut(&Nv12)) -> WinResult<()> {
        unsafe {
            let sample = MFCreateSample()?;
            let buffer = MFCreateMemoryBuffer(au.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(au.as_ptr(), ptr, au.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(au.len() as u32)?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts_us * 10)?;

            if let Err(e) = self.mft.ProcessInput(0, &sample, 0) {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(e);
                }
                self.drain(on_frame)?;
                self.mft.ProcessInput(0, &sample, 0)?;
            }
            self.drain(on_frame)
        }
    }

    unsafe fn drain(&mut self, on_frame: &mut dyn FnMut(&Nv12)) -> WinResult<()> {
        loop {
            let out_sample = if self.provides_samples {
                None
            } else {
                let s = MFCreateSample()?;
                s.AddBuffer(&MFCreateMemoryBuffer(self.buffer_size)?)?;
                Some(s)
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(out_sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let result = self.mft.ProcessOutput(0, &mut buffers, &mut status);
            let sample = ManuallyDrop::take(&mut buffers[0].pSample);
            drop(ManuallyDrop::take(&mut buffers[0].pEvents));

            match result {
                Ok(()) => {
                    if let Some(sample) = sample {
                        self.emit(&sample, on_frame)?;
                    }
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.configure_output()?,
                Err(e) => return Err(e),
            }
        }
    }

    unsafe fn emit(&self, sample: &IMFSample, on_frame: &mut dyn FnMut(&Nv12)) -> WinResult<()> {
        let buffer = sample.ConvertToContiguousBuffer()?;
        // Prefer the 2D buffer's real stride when the decoder exposes one.
        let mut stride = self.stride;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let twod = buffer.cast::<IMF2DBuffer>().ok();
        if let Some(b2) = &twod {
            let mut scan0 = std::ptr::null_mut();
            let mut pitch = 0i32;
            if b2.Lock2D(&mut scan0, &mut pitch).is_ok() {
                stride = pitch.unsigned_abs();
                len = b2.GetContiguousLength().unwrap_or(0);
                ptr = scan0;
            }
        }
        let locked_2d = !ptr.is_null();
        if !locked_2d {
            buffer.Lock(&mut ptr, None, Some(&mut len))?;
        }
        if stride > 0 && self.width > 0 && len as usize >= (stride * self.height) as usize {
            // Plane height may be padded (e.g. 1088 for 1080p): derive it from the buffer length.
            let plane_h = (len as usize * 2 / 3) / stride as usize;
            let data = std::slice::from_raw_parts(ptr, len as usize);
            let frame = Nv12 { data, width: self.width, height: self.height, stride, uv_offset: stride as usize * plane_h };
            on_frame(&frame);
        }
        if locked_2d {
            twod.unwrap().Unlock2D()?;
        } else {
            buffer.Unlock()?;
        }
        Ok(())
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = MFShutdown();
        }
    }
}
