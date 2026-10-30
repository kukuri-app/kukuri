//! libvpx による VP8 の圧縮・復号と、RGBA から I420 への変換（ADR 0070 §8.3、#1982）。libvpx は `KUKURI_LIBVPX_DIR` の
//! build を静的に link する（feature `vp8`。`sdk/unity/build-libvpx.ps1`）。

use std::ffi::c_void;

unsafe extern "C" {
    fn kvpx_encoder_new(width: u32, height: u32, kbps: u32, threads: u32) -> *mut c_void;
    fn kvpx_encode(encoder: *mut c_void, i420: *const u8, pts_us: i64, keyframe: i32) -> i32;
    fn kvpx_encoder_packet(encoder: *mut c_void, len: *mut usize, keyframe: *mut i32) -> *const u8;
    fn kvpx_encoder_free(encoder: *mut c_void);
    fn kvpx_decoder_new(threads: u32) -> *mut c_void;
    fn kvpx_decode(decoder: *mut c_void, data: *const u8, len: usize) -> i32;
    fn kvpx_decoder_next(decoder: *mut c_void, width: *mut u32, height: *mut u32) -> i32;
    fn kvpx_decoder_copy(decoder: *mut c_void, out: *mut u8);
    fn kvpx_decoder_free(decoder: *mut c_void);
}

/// 圧縮・復号の thread 数（ADR 0070 §8.3）。
const THREADS: u32 = 4;

/// 詰めた I420（Y・U・V の順）の長さ。
pub fn i420_len(width: u32, height: u32) -> usize {
    let (width, height) = (width as usize, height as usize);
    width * height + 2 * width.div_ceil(2) * height.div_ceil(2)
}

/// 下の行から並んだ RGBA（Unity の readback の並び）を、上の行から並んだ I420（BT.601 の limited range）にする。
pub fn rgba_bottom_up_to_i420(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; i420_len(width, height)];
    let (luma, chroma) = out.split_at_mut(w * h);
    let (cb, cr) = chroma.split_at_mut(cw * ch);
    let pixel = |x: usize, y: usize| {
        let at = ((h - 1 - y.min(h - 1)) * w + x.min(w - 1)) * 4;
        [
            i32::from(rgba[at]),
            i32::from(rgba[at + 1]),
            i32::from(rgba[at + 2]),
        ]
    };
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = pixel(x, y);
            luma[y * w + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
        }
    }
    for y in 0..ch {
        for x in 0..cw {
            let mut sum = [0; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let rgb = pixel(x * 2 + dx, y * 2 + dy);
                (0..3).for_each(|index| sum[index] += rgb[index]);
            }
            let [r, g, b] = sum.map(|value| value / 4);
            cb[y * cw + x] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            cr[y * cw + x] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
    out
}

pub struct Encoder {
    raw: *mut c_void,
    width: u32,
    height: u32,
}

// libvpx の context は 1 つの thread から順に使う（Encoder は `&mut` でだけ使う）。
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(width: u32, height: u32, kbps: u32) -> Option<Self> {
        let raw = unsafe { kvpx_encoder_new(width, height, kbps, THREADS) };
        (!raw.is_null()).then_some(Self { raw, width, height })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 詰めた I420 を圧縮し、出た frame（中身と keyframe か）を返す。
    pub fn encode(&mut self, i420: &[u8], pts_us: i64, keyframe: bool) -> Vec<(Vec<u8>, bool)> {
        assert_eq!(i420.len(), i420_len(self.width, self.height));
        if unsafe { kvpx_encode(self.raw, i420.as_ptr(), pts_us, i32::from(keyframe)) } != 0 {
            return Vec::new();
        }
        let mut frames = Vec::new();
        loop {
            let (mut len, mut key) = (0usize, 0i32);
            let data = unsafe { kvpx_encoder_packet(self.raw, &mut len, &mut key) };
            if data.is_null() {
                return frames;
            }
            frames.push((
                unsafe { std::slice::from_raw_parts(data, len) }.to_vec(),
                key != 0,
            ));
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { kvpx_encoder_free(self.raw) }
    }
}

/// 復号した frame（詰めた I420）。
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub i420: Vec<u8>,
}

pub struct Decoder {
    raw: *mut c_void,
}

unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Option<Self> {
        let raw = unsafe { kvpx_decoder_new(THREADS) };
        (!raw.is_null()).then_some(Self { raw })
    }

    /// chunk を復号し、出た最後の frame を返す（壊れた chunk は None）。
    pub fn decode(&mut self, chunk: &[u8]) -> Option<Frame> {
        if unsafe { kvpx_decode(self.raw, chunk.as_ptr(), chunk.len()) } != 0 {
            return None;
        }
        let mut last = None;
        let (mut width, mut height) = (0u32, 0u32);
        while unsafe { kvpx_decoder_next(self.raw, &mut width, &mut height) } != 0 {
            let mut i420 = vec![0u8; i420_len(width, height)];
            unsafe { kvpx_decoder_copy(self.raw, i420.as_mut_ptr()) };
            last = Some(Frame {
                width,
                height,
                i420,
            });
        }
        last
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { kvpx_decoder_free(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 下の行から並んだ RGBA で、上半分が赤・下半分が青の画像。
    fn red_over_blue(width: u32, height: u32) -> Vec<u8> {
        (0..height)
            .rev()
            .flat_map(|y| {
                let color = if y < height / 2 {
                    [230, 20, 20, 255]
                } else {
                    [20, 20, 230, 255]
                };
                (0..width).flat_map(move |_| color)
            })
            .collect()
    }

    /// 既存の desktop・Web の配信の圧縮（Chromium の WebCodecs の VP8。上が赤・下が青の 320×240、realtime・1.5 Mbps）の
    /// 出力を libvpx で復号できる（#1982。desktop・Web の配信を Unity で見る向き）。
    #[test]
    fn chunks_from_webcodecs_decode_with_the_top_row_first() {
        let mut decoder = Decoder::new().unwrap();
        let mut decoded = None;
        for line in include_str!("../../tests/fixtures/webcodecs-vp8-red-over-blue.txt").lines() {
            let (_, hex) = line.split_once(' ').unwrap();
            let chunk: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
                .collect();
            decoded = Some(decoder.decode(&chunk).expect("every chunk decodes"));
        }
        let frame = decoded.unwrap();
        assert_eq!((frame.width, frame.height), (320, 240));
        let cr = |y: u32| frame.i420[(320 * 240 + 320 * 240 / 4 + y / 2 * 160 + 80) as usize];
        assert!(cr(10) > 200 && cr(230) < 125, "{} {}", cr(10), cr(230));
    }

    #[test]
    fn frames_round_trip_through_vp8_with_the_top_row_first() {
        let (width, height) = (320, 240);
        let i420 = rgba_bottom_up_to_i420(&red_over_blue(width, height), width, height);
        let mut encoder = Encoder::new(width, height, 1_500).unwrap();
        let mut decoder = Decoder::new().unwrap();
        let mut decoded = None;
        for index in 0..5 {
            for (chunk, keyframe) in encoder.encode(&i420, index * 33_333, index == 0) {
                assert_eq!(keyframe, index == 0, "only the first frame is a keyframe");
                decoded = decoder.decode(&chunk).or(decoded);
            }
        }
        let frame = decoded.expect("decoded");
        assert_eq!((frame.width, frame.height), (width, height));
        let luma = |y: u32| frame.i420[(y * width + width / 2) as usize];
        let cr =
            |y: u32| frame.i420[(width * height + width * height / 4 + y / 2 * width / 2) as usize];
        // 上の行は赤（V が大きい）、下の行は青（V が小さい）。
        assert!(
            cr(10) > 200 && cr(height - 10) < 120,
            "{} {}",
            cr(10),
            cr(height - 10)
        );
        assert!(luma(10).abs_diff(luma(height - 10)) > 10);
    }
}
