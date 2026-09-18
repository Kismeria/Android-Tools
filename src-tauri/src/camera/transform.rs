//! Rotate / mirror / fit-or-fill an NV12 picture into a fixed-size NV12 canvas.
//! Index maps are rebuilt only when geometry or options change, so per-frame work is a gather.
use super::decoder::Nv12;

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Default)]
pub struct Options {
    pub rotation: u32,
    pub mirror: bool,
    pub fill: bool,
}

pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    key: Option<(u32, u32, u32, usize, Options)>,
    ymap: Vec<u32>,
    uvmap: Vec<u32>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        let mut data = vec![16u8; (width * height * 3 / 2) as usize];
        data[(width * height) as usize..].fill(128);
        Canvas { width, height, data, key: None, ymap: Vec::new(), uvmap: Vec::new() }
    }

    pub fn draw(&mut self, src: &Nv12, opt: Options) {
        let key = (src.width, src.height, src.stride, src.uv_offset, opt);
        if self.key != Some(key) {
            self.build(src, opt);
            self.key = Some(key);
        }
        let (y_plane, uv_plane) = self.data.split_at_mut((self.width * self.height) as usize);
        let s = src.data;
        for (d, &m) in y_plane.iter_mut().zip(&self.ymap) {
            *d = if m == NONE { 16 } else { s[m as usize] };
        }
        for (d, &m) in uv_plane.chunks_exact_mut(2).zip(&self.uvmap) {
            if m == NONE {
                d[0] = 128;
                d[1] = 128;
            } else {
                d[0] = s[m as usize];
                d[1] = s[m as usize + 1];
            }
        }
    }

    fn build(&mut self, src: &Nv12, opt: Options) {
        let (sw, sh) = (src.width as f64, src.height as f64);
        let rot = opt.rotation % 360;
        let (rw, rh) = if rot == 90 || rot == 270 { (sh, sw) } else { (sw, sh) };
        let (dw, dh) = (self.width as f64, self.height as f64);
        let scale = if opt.fill { (dw / rw).max(dh / rh) } else { (dw / rw).min(dh / rh) };
        let (ox, oy) = ((dw - rw * scale) / 2.0, (dh - rh * scale) / 2.0);
        let limit = src.data.len();

        // Maps a destination pixel centre to a source pixel, or None when outside the picture.
        let map = |x: f64, y: f64| -> Option<(u32, u32)> {
            let mut u = (x - ox) / scale;
            let v = (y - oy) / scale;
            if u < 0.0 || v < 0.0 || u >= rw || v >= rh {
                return None;
            }
            if opt.mirror {
                u = rw - 1.0 - u;
            }
            let (sx, sy) = match rot {
                90 => (v, sh - 1.0 - u),
                180 => (sw - 1.0 - u, sh - 1.0 - v),
                270 => (sw - 1.0 - v, u),
                _ => (u, v),
            };
            let sx = sx.clamp(0.0, sw - 1.0) as u32;
            let sy = sy.clamp(0.0, sh - 1.0) as u32;
            Some((sx, sy))
        };

        let (w, h) = (self.width as usize, self.height as usize);
        self.ymap = vec![NONE; w * h];
        for y in 0..h {
            for x in 0..w {
                if let Some((sx, sy)) = map(x as f64 + 0.5, y as f64 + 0.5) {
                    let i = sy as usize * src.stride as usize + sx as usize;
                    if i < limit {
                        self.ymap[y * w + x] = i as u32;
                    }
                }
            }
        }
        self.uvmap = vec![NONE; (w / 2) * (h / 2)];
        for y in 0..h / 2 {
            for x in 0..w / 2 {
                if let Some((sx, sy)) = map(x as f64 * 2.0 + 1.0, y as f64 * 2.0 + 1.0) {
                    let i = src.uv_offset + (sy as usize / 2) * src.stride as usize + (sx as usize / 2) * 2;
                    if i + 1 < limit {
                        self.uvmap[y * (w / 2) + x] = i as u32;
                    }
                }
            }
        }
    }

    /// Full-size top-down BGR (DirectShow camera input).
    pub fn to_bgr(&self, out: &mut Vec<u8>) {
        let (w, h) = (self.width as usize, self.height as usize);
        out.resize(w * h * 3, 0);
        let (yp, uvp) = self.data.split_at(w * h);
        for y in 0..h {
            let row = &yp[y * w..(y + 1) * w];
            let uvrow = &uvp[(y / 2) * w..(y / 2) * w + w];
            let dst = &mut out[y * w * 3..(y + 1) * w * 3];
            for x in 0..w {
                let c = 298 * (row[x] as i32 - 16);
                let u = uvrow[x & !1] as i32 - 128;
                let v = uvrow[(x & !1) + 1] as i32 - 128;
                dst[x * 3] = ((c + 516 * u + 128) >> 8).clamp(0, 255) as u8;
                dst[x * 3 + 1] = ((c - 100 * u - 208 * v + 128) >> 8).clamp(0, 255) as u8;
                dst[x * 3 + 2] = ((c + 409 * v + 128) >> 8).clamp(0, 255) as u8;
            }
        }
    }

    /// Small RGB rendition for the UI preview.
    pub fn preview_rgb(&self, out_w: u32) -> (Vec<u8>, u32, u32) {
        let out_h = (out_w as u64 * self.height as u64 / self.width as u64) as u32 & !1;
        let mut rgb = Vec::with_capacity((out_w * out_h * 3) as usize);
        let (w, h) = (self.width as usize, self.height as usize);
        for oy in 0..out_h as usize {
            let y = oy * h / out_h as usize;
            for ox in 0..out_w as usize {
                let x = ox * w / out_w as usize;
                let yy = self.data[y * w + x] as f32 - 16.0;
                let uvi = w * h + (y / 2) * w + (x / 2) * 2;
                let u = self.data[uvi] as f32 - 128.0;
                let v = self.data[uvi + 1] as f32 - 128.0;
                let c = 1.164 * yy;
                rgb.push((c + 1.596 * v).clamp(0.0, 255.0) as u8);
                rgb.push((c - 0.392 * u - 0.813 * v).clamp(0.0, 255.0) as u8);
                rgb.push((c + 2.017 * u).clamp(0.0, 255.0) as u8);
            }
        }
        (rgb, out_w, out_h)
    }
}
