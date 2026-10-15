//! Tight encoding: solid fill, JPEG, or pixel data (optionally palette or
//! gradient filtered) compressed with one of four zlib streams that live
//! for the whole connection.

use flate2::Decompress;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use super::convert::{Converter, blit, fill, inflate, invalid, read_into};
use super::{DecodeError, MAX_RECT_PIXELS, Scratch};
use crate::{Framebuffer, Rect, RfbReader};

const FILTER_COPY: u8 = 0;
const FILTER_PALETTE: u8 = 1;
const FILTER_GRADIENT: u8 = 2;

/// Pixel data shorter than this is sent without compression.
const MIN_TO_COMPRESS: usize = 12;

/// Largest JPEG side we agree to decode.
const MAX_JPEG_SIDE: usize = 16384;

/// Reads Tight pixels (`TPIXEL`): 3 bytes R, G, B for 24-bit colour,
/// otherwise a whole pixel.
struct TPixels<'a> {
    conv: &'a Converter,
    rgb: bool,
    size: usize,
}

impl<'a> TPixels<'a> {
    fn new(conv: &'a Converter) -> Self {
        let pf = conv.format();
        TPixels { conv, rgb: pf.is_tight_rgb(), size: pf.bytes_per_tpixel() }
    }

    #[inline]
    fn rgba(&self, b: &[u8]) -> [u8; 4] {
        if self.rgb { [b[0], b[1], b[2], 255] } else { self.conv.pixel(b) }
    }

    fn row(&self, src: &[u8], dst: &mut [u8]) {
        if self.rgb {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
                d.copy_from_slice(&[s[0], s[1], s[2], 255]);
            }
        } else {
            self.conv.row(src, dst);
        }
    }
}

enum Filter {
    Copy,
    /// Two colours, one bit per pixel, rows padded to whole bytes.
    Mono,
    /// Up to 256 colours, one byte per pixel.
    Indexed,
    Gradient,
}

pub(crate) struct Tight {
    streams: [Decompress; 4],
    /// Previous row of channel values for the gradient filter.
    prev: Vec<i32>,
}

impl Tight {
    pub fn new() -> Self {
        Tight { streams: std::array::from_fn(|_| Decompress::new(true)), prev: Vec::new() }
    }

    pub async fn decode(
        &mut self,
        rect: Rect,
        conv: &Converter,
        r: &mut RfbReader,
        fb: &mut Framebuffer,
        s: &mut Scratch,
    ) -> Result<(), DecodeError> {
        let ctl = r.read_u8().await?;
        for (i, z) in self.streams.iter_mut().enumerate() {
            if ctl & (1 << i) != 0 {
                z.reset(true);
            }
        }
        let tp = TPixels::new(conv);
        match ctl >> 4 {
            0x8 => {
                let mut b = [0u8; 4];
                r.read_exact(&mut b[..tp.size]).await?;
                fill(fb, rect, tp.rgba(&b[..tp.size]));
                fb.mark_dirty(rect);
                Ok(())
            }
            0x9 => self.jpeg(rect, r, fb, s).await,
            0xA => self.basic(rect, &tp, None, false, r, fb, s).await,
            0xE => self.basic(rect, &tp, None, true, r, fb, s).await,
            k @ 0..=7 => self.basic(rect, &tp, Some((k & 3) as usize), k & 4 != 0, r, fb, s).await,
            k => Err(invalid(format!("unknown Tight compression type {k}"))),
        }
    }

    /// Basic compression. `stream` is `None` when the data is never zlib
    /// compressed ("Tight without zlib").
    #[allow(clippy::too_many_arguments)]
    async fn basic(
        &mut self,
        rect: Rect,
        tp: &TPixels<'_>,
        stream: Option<usize>,
        read_filter: bool,
        r: &mut RfbReader,
        fb: &mut Framebuffer,
        s: &mut Scratch,
    ) -> Result<(), DecodeError> {
        let (w, h) = (rect.w as usize, rect.h as usize);
        if w * h > MAX_RECT_PIXELS {
            return Err(invalid("Tight rectangle is too large"));
        }
        let filter = if read_filter { r.read_u8().await? } else { FILTER_COPY };
        let mut palette = [[0u8, 0, 0, 255]; 256];
        let (size, kind) = match filter {
            FILTER_COPY => (w * h * tp.size, Filter::Copy),
            FILTER_PALETTE => {
                let n = r.read_u8().await? as usize + 1;
                let mut buf = [0u8; 256 * 4];
                let len = n * tp.size;
                r.read_exact(&mut buf[..len]).await?;
                for (p, c) in palette.iter_mut().zip(buf[..len].chunks_exact(tp.size)) {
                    *p = tp.rgba(c);
                }
                if n == 2 { (w.div_ceil(8) * h, Filter::Mono) } else { (w * h, Filter::Indexed) }
            }
            FILTER_GRADIENT => (w * h * tp.size, Filter::Gradient),
            f => return Err(invalid(format!("unknown Tight filter {f}"))),
        };

        match stream {
            Some(i) if size >= MIN_TO_COMPRESS => {
                let len = r.read_compact_len().await?;
                read_into(r, &mut s.input, len).await?;
                inflate(&mut self.streams[i], &s.input, &mut s.inflated, size)?;
                if s.inflated.len() != size {
                    return Err(invalid("Tight data is shorter than its rectangle"));
                }
            }
            _ => read_into(r, &mut s.inflated, size).await?,
        }
        if rect.is_empty() {
            return Ok(());
        }

        let data = &s.inflated[..size];
        s.rgba.resize(w * h * 4, 0);
        let out = &mut s.rgba[..w * h * 4];
        match kind {
            Filter::Copy => tp.row(data, out),
            Filter::Indexed => {
                for (d, &i) in out.chunks_exact_mut(4).zip(data) {
                    d.copy_from_slice(&palette[i as usize]);
                }
            }
            Filter::Mono => {
                for (src, dst) in data.chunks_exact(w.div_ceil(8)).zip(out.chunks_exact_mut(w * 4)) {
                    for (x, d) in dst.chunks_exact_mut(4).enumerate() {
                        let bit = (src[x / 8] >> (7 - x % 8)) & 1;
                        d.copy_from_slice(&palette[bit as usize]);
                    }
                }
            }
            Filter::Gradient => self.gradient(tp, w, data, out),
        }
        blit(fb, rect, out);
        fb.mark_dirty(rect);
        Ok(())
    }

    /// Undo the gradient filter: each channel was sent as the difference
    /// from a prediction made from the pixels to the left, above and
    /// above-left.
    fn gradient(&mut self, tp: &TPixels, w: usize, data: &[u8], out: &mut [u8]) {
        self.prev.clear();
        self.prev.resize(w * 3, 0);
        let conv = tp.conv;
        let max = if tp.rgb { [255, 255, 255] } else { conv.maxes().map(|m| m as i32) };
        for (src, dst) in data.chunks_exact(w * tp.size).zip(out.chunks_exact_mut(w * 4)) {
            let mut left = [0i32; 3];
            let mut up_left = [0i32; 3];
            let pixels = src.chunks_exact(tp.size).zip(dst.chunks_exact_mut(4)).zip(self.prev.chunks_exact_mut(3));
            for ((px, d), prev) in pixels {
                let diff = if tp.rgb { [px[0] as u32, px[1] as u32, px[2] as u32] } else { conv.split(conv.raw(px)) };
                for c in 0..3 {
                    let up = prev[c];
                    let guess = (left[c] + up - up_left[c]).clamp(0, max[c]);
                    let v = (diff[c] as i32 + guess) & max[c];
                    up_left[c] = up;
                    left[c] = v;
                    prev[c] = v;
                }
                let rgba = if tp.rgb {
                    [left[0] as u8, left[1] as u8, left[2] as u8, 255]
                } else {
                    [
                        conv.channel(0, left[0] as u32),
                        conv.channel(1, left[1] as u32),
                        conv.channel(2, left[2] as u32),
                        255,
                    ]
                };
                d.copy_from_slice(&rgba);
            }
        }
    }

    async fn jpeg(
        &mut self,
        rect: Rect,
        r: &mut RfbReader,
        fb: &mut Framebuffer,
        s: &mut Scratch,
    ) -> Result<(), DecodeError> {
        let len = r.read_compact_len().await?;
        read_into(r, &mut s.input, len).await?;
        if rect.is_empty() {
            return Ok(());
        }
        // A broken picture does not upset the stream, so skip it and go on.
        let (jw, jh) = match decode_jpeg(&s.input, &mut s.rgba) {
            Ok(size) => size,
            Err(e) => {
                log::warn!("could not decode a JPEG picture: {e}");
                return Ok(());
            }
        };
        // Crop the picture to the rectangle if the sizes differ.
        let cw = jw.min(rect.w as usize);
        for row in 0..jh.min(rect.h as usize) {
            let Some(y) = rect.y.checked_add(row as u16) else { break };
            let dst = fb.row_mut(rect.x, y, cw as u16);
            if dst.is_empty() {
                break;
            }
            let start = row * jw * 4;
            dst.copy_from_slice(&s.rgba[start..start + dst.len()]);
        }
        fb.mark_dirty(rect);
        Ok(())
    }
}

/// Decode a JPEG into RGBA, returning its width and height.
fn decode_jpeg(data: &[u8], out: &mut Vec<u8>) -> Result<(usize, usize), String> {
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(MAX_JPEG_SIDE)
        .set_max_height(MAX_JPEG_SIDE);
    let mut dec = JpegDecoder::new_with_options(ZCursor::new(data), options);
    dec.decode_headers().map_err(|e| e.to_string())?;
    let (w, h) = dec.dimensions().ok_or("the picture has no size")?;
    let size = dec.output_buffer_size().ok_or("the picture is too large")?;
    if size != w * h * 4 {
        return Err("unexpected picture layout".into());
    }
    out.resize(size, 0);
    dec.decode_into(&mut out[..size]).map_err(|e| e.to_string())?;
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;
    use flate2::{Compress, Compression, FlushCompress};

    fn compact(len: usize) -> Vec<u8> {
        let mut v = vec![(len & 0x7f) as u8];
        if len > 0x7f {
            v[0] |= 0x80;
            v.push(((len >> 7) & 0x7f) as u8);
            if len > 0x3fff {
                v[1] |= 0x80;
                v.push((len >> 14) as u8);
            }
        }
        v
    }

    fn compress(z: &mut Compress, data: &[u8]) -> Vec<u8> {
        let mut packed = Vec::with_capacity(data.len() + 1024);
        z.compress_vec(data, &mut packed, FlushCompress::Sync).unwrap();
        let mut out = compact(packed.len());
        out.extend_from_slice(&packed);
        out
    }

    async fn run(t: &mut Tight, pf: &PixelFormat, rect: Rect, fb: &mut Framebuffer, wire: Vec<u8>) {
        let mut r = RfbReader::from_bytes(wire);
        let conv = Converter::new(pf);
        t.decode(rect, &conv, &mut r, fb, &mut Scratch::default()).await.unwrap();
        assert_eq!(r.buffered(), 0, "all data should be used");
    }

    fn px(fb: &Framebuffer, x: u16, y: u16) -> [u8; 4] {
        fb.read_rect(Rect::new(x, y, 1, 1)).try_into().unwrap()
    }

    #[tokio::test]
    async fn tight_fill_rgb_and_rgb565() {
        let mut t = Tight::new();
        let mut fb = Framebuffer::new(4, 4);
        run(&mut t, &PixelFormat::RGBX, Rect::new(1, 1, 2, 2), &mut fb, vec![0x80, 5, 6, 7]).await;
        assert_eq!(px(&fb, 2, 2), [5, 6, 7, 255]);
        assert_eq!(px(&fb, 0, 0), [0, 0, 0, 255]);
        let mut wire = vec![0x80];
        wire.extend_from_slice(&0x07e0u16.to_le_bytes());
        run(&mut t, &PixelFormat::RGB565, Rect::new(0, 0, 1, 1), &mut fb, wire).await;
        assert_eq!(px(&fb, 0, 0), [0, 255, 0, 255]);
    }

    #[tokio::test]
    async fn tight_copy_small_and_compressed_with_reset() {
        let mut t = Tight::new();
        let mut fb = Framebuffer::new(8, 4);
        // 3 pixels = 9 bytes: sent as is.
        run(&mut t, &PixelFormat::RGBX, Rect::new(0, 0, 3, 1), &mut fb, vec![0x00, 1, 2, 3, 4, 5, 6, 7, 8, 9]).await;
        assert_eq!(px(&fb, 2, 0), [7, 8, 9, 255]);

        // 8x2 pixels through stream 1, in two rectangles to use the same stream.
        let mut server = Compress::new(Compression::default(), true);
        for round in 0..2u8 {
            let raw: Vec<u8> = (0..16u8).flat_map(|i| [i, round, 200]).collect();
            let mut wire = vec![0x10];
            wire.extend_from_slice(&compress(&mut server, &raw));
            run(&mut t, &PixelFormat::RGBX, Rect::new(0, 2, 8, 2), &mut fb, wire).await;
            assert_eq!(px(&fb, 7, 3), [15, round, 200, 255]);
        }

        // The server starts stream 1 afresh and tells us to reset it.
        let mut server = Compress::new(Compression::default(), true);
        let raw: Vec<u8> = (0..16u8).flat_map(|i| [i, 77, 1]).collect();
        let mut wire = vec![0x12];
        wire.extend_from_slice(&compress(&mut server, &raw));
        run(&mut t, &PixelFormat::RGBX, Rect::new(0, 2, 8, 2), &mut fb, wire).await;
        assert_eq!(px(&fb, 3, 2), [3, 77, 1, 255]);
    }

    #[tokio::test]
    async fn tight_palette_two_colours_pads_rows() {
        let mut t = Tight::new();
        let mut fb = Framebuffer::new(10, 2);
        let mut server = Compress::new(Compression::default(), true);
        // Stream 2 with a filter id; 10 pixels wide means 2 bytes per row.
        let mut wire = vec![0x60, FILTER_PALETTE, 1, 0, 0, 0, 255, 255, 255];
        let bits = [0b1000_0000, 0b0100_0000, 0b0000_0001, 0b1100_0000];
        wire.extend_from_slice(&bits); // 4 bytes: below 12, so sent as is
        run(&mut t, &PixelFormat::RGBX, Rect::new(0, 0, 10, 2), &mut fb, wire).await;
        let row = |y| (0..10).map(|x| px(&fb, x, y)[0]).collect::<Vec<_>>();
        assert_eq!(row(0), vec![255, 0, 0, 0, 0, 0, 0, 0, 0, 255]);
        assert_eq!(row(1), vec![0, 0, 0, 0, 0, 0, 0, 255, 255, 255]);

        // Many rows so the data is big enough to be compressed.
        let mut fb = Framebuffer::new(10, 8);
        let mut wire = vec![0x60, FILTER_PALETTE, 1, 0, 0, 0, 255, 255, 255];
        let bits: Vec<u8> = (0..8).flat_map(|_| [0b1000_0000u8, 0b0100_0000]).collect();
        wire.extend_from_slice(&compress(&mut server, &bits));
        run(&mut t, &PixelFormat::RGBX, Rect::new(0, 0, 10, 8), &mut fb, wire).await;
        assert_eq!(px(&fb, 0, 7)[0], 255);
        assert_eq!(px(&fb, 9, 7)[0], 255);
        assert_eq!(px(&fb, 8, 7)[0], 0);
    }

    #[tokio::test]
    async fn tight_palette_many_colours() {
        let mut t = Tight::new();
        let mut fb = Framebuffer::new(4, 4);
        let mut server = Compress::new(Compression::default(), true);
        let mut wire = vec![0x40, FILTER_PALETTE, 2];
        wire.extend_from_slice(&[10, 0, 0, 20, 0, 0, 30, 0, 0]);
        let idx: Vec<u8> = (0..16).map(|i| (i % 3) as u8).collect();
        wire.extend_from_slice(&compress(&mut server, &idx));
        run(&mut t, &PixelFormat::RGBX, Rect::new(0, 0, 4, 4), &mut fb, wire).await;
        let all: Vec<u8> = (0..16).map(|i| px(&fb, i % 4, i / 4)[0]).collect();
        assert_eq!(all, idx.iter().map(|i| (i + 1) * 10).collect::<Vec<_>>());
    }

    /// Server side of the gradient filter for 3-byte pixels.
    fn gradient_encode(w: usize, rgb: &[u8]) -> Vec<u8> {
        let h = rgb.len() / (w * 3);
        let v = |x: isize, y: isize, c: usize| -> i32 {
            if x < 0 || y < 0 { 0 } else { rgb[(y as usize * w + x as usize) * 3 + c] as i32 }
        };
        let mut out = Vec::new();
        for y in 0..h as isize {
            for x in 0..w as isize {
                for c in 0..3 {
                    let p = (v(x - 1, y, c) + v(x, y - 1, c) - v(x - 1, y - 1, c)).clamp(0, 255);
                    out.push((v(x, y, c) - p) as u8);
                }
            }
        }
        out
    }

    #[tokio::test]
    async fn tight_gradient() {
        let mut t = Tight::new();
        let (w, h) = (5usize, 4usize);
        let rgb: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = ((i % w) as u8, (i / w) as u8);
                [x * 40, y * 60, 255 - x * y * 10]
            })
            .collect();
        let mut server = Compress::new(Compression::default(), true);
        let mut wire = vec![0x70, FILTER_GRADIENT];
        wire.extend_from_slice(&compress(&mut server, &gradient_encode(w, &rgb)));
        let mut fb = Framebuffer::new(w as u16, h as u16);
        run(&mut t, &PixelFormat::RGBX, Rect::new(0, 0, w as u16, h as u16), &mut fb, wire).await;
        let got: Vec<u8> = fb.read_rect(fb.bounds()).chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        assert_eq!(got, rgb);
    }

    #[tokio::test]
    async fn tight_without_zlib_and_rgb565_copy() {
        let mut t = Tight::new();
        let mut fb = Framebuffer::new(4, 1);
        let mut wire = vec![0xA0];
        for v in [0xf800u16, 0x07e0, 0x001f, 0xffff] {
            wire.extend_from_slice(&v.to_le_bytes());
        }
        run(&mut t, &PixelFormat::RGB565, Rect::new(0, 0, 4, 1), &mut fb, wire).await;
        assert_eq!(px(&fb, 0, 0), [255, 0, 0, 255]);
        assert_eq!(px(&fb, 2, 0), [0, 0, 255, 255]);
        assert_eq!(px(&fb, 3, 0), [255, 255, 255, 255]);
    }

    #[tokio::test]
    async fn tight_jpeg() {
        use jpeg_encoder::{ColorType, Encoder};
        let (w, h) = (32u16, 16u16);
        let mut rgb = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(if x < 16 { &[220, 30, 30] } else { &[20, 40, 210] });
            }
        }
        let mut jpeg = Vec::new();
        Encoder::new(&mut jpeg, 95).encode(&rgb, w, h, ColorType::Rgb).unwrap();
        let mut wire = vec![0x90];
        wire.extend_from_slice(&compact(jpeg.len()));
        wire.extend_from_slice(&jpeg);
        let mut t = Tight::new();
        let mut fb = Framebuffer::new(40, 20);
        // The rectangle is narrower than the picture, so it gets cropped.
        run(&mut t, &PixelFormat::RGBX, Rect::new(2, 2, 30, 16), &mut fb, wire).await;
        let close = |a: [u8; 4], b: [u8; 3]| (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= 12) && a[3] == 255;
        assert!(close(px(&fb, 4, 4), [220, 30, 30]), "{:?}", px(&fb, 4, 4));
        assert!(close(px(&fb, 28, 10), [20, 40, 210]), "{:?}", px(&fb, 28, 10));
        assert_eq!(px(&fb, 32, 4), [0, 0, 0, 255]);
        assert!(fb.read_rect(fb.bounds()).chunks_exact(4).all(|p| p[3] == 255));
    }

    #[tokio::test]
    async fn tight_rejects_bad_control_byte() {
        let mut t = Tight::new();
        let mut fb = Framebuffer::new(1, 1);
        let mut r = RfbReader::from_bytes(vec![0xC0]);
        let conv = Converter::new(&PixelFormat::RGBX);
        let res = t.decode(Rect::new(0, 0, 1, 1), &conv, &mut r, &mut fb, &mut Scratch::default()).await;
        assert!(matches!(res, Err(DecodeError::Invalid(_))));
    }
}
