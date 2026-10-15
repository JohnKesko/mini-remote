//! Shared helpers: fast conversion from server pixels to framebuffer RGBA,
//! reading and inflating blocks of data, a small reader over decompressed
//! bytes, and writing into the framebuffer without marking anything as
//! changed (the decoders mark the whole rectangle once at the end).

use flate2::{Decompress, FlushDecompress, Status};

use super::DecodeError;
use crate::{Framebuffer, PixelFormat, Rect, RfbReader};

/// Shorthand for a "bad data" error.
pub(crate) fn invalid(msg: impl Into<String>) -> DecodeError {
    DecodeError::Invalid(msg.into())
}

/// Turns pixels in one pixel format into RGBA. Channels of up to 8 bits use
/// lookup tables, so no division is needed per pixel.
pub(crate) struct Converter {
    pf: PixelFormat,
    bpp: usize,
    rgbx: bool,
    /// Per channel: shift and mask (mask 0 when the shift is out of range).
    shift: [u32; 3],
    mask: [u32; 3],
    /// Channel value to 0..=255, used when every channel max is <= 255.
    tables: Option<Box<[[u8; 256]; 3]>>,
}

impl Converter {
    pub fn new(pf: &PixelFormat) -> Self {
        let maxes = [pf.red_max, pf.green_max, pf.blue_max];
        let shifts = [pf.red_shift, pf.green_shift, pf.blue_shift];
        let mut shift = [0u32; 3];
        let mut mask = [0u32; 3];
        for i in 0..3 {
            if (shifts[i] as u32) < 32 {
                shift[i] = shifts[i] as u32;
                mask[i] = maxes[i] as u32;
            }
        }
        let tables = maxes.iter().all(|&m| m <= 255).then(|| {
            let mut t = Box::new([[0u8; 256]; 3]);
            for (c, table) in t.iter_mut().enumerate() {
                let max = maxes[c] as u32;
                if max == 0 {
                    continue;
                }
                for (v, out) in table.iter_mut().enumerate().take(max as usize + 1) {
                    *out = ((v as u32 * 255 + max / 2) / max) as u8;
                }
            }
            t
        });
        Converter { pf: *pf, bpp: pf.bytes_per_pixel(), rgbx: pf.is_rgbx(), shift, mask, tables }
    }

    pub fn format(&self) -> &PixelFormat {
        &self.pf
    }

    pub fn bpp(&self) -> usize {
        self.bpp
    }

    /// Scale one channel value (already shifted and masked) to 0..=255.
    #[inline]
    pub fn channel(&self, c: usize, v: u32) -> u8 {
        match &self.tables {
            Some(t) => t[c][(v & 0xff) as usize],
            None => {
                let max = self.mask[c];
                (v.min(max) * 255 + max / 2).checked_div(max).unwrap_or(0) as u8
            }
        }
    }

    /// Split a packed pixel value into its three channel values.
    #[inline]
    pub fn split(&self, v: u32) -> [u32; 3] {
        [(v >> self.shift[0]) & self.mask[0], (v >> self.shift[1]) & self.mask[1], (v >> self.shift[2]) & self.mask[2]]
    }

    /// Largest value of each channel.
    pub fn maxes(&self) -> [u32; 3] {
        self.mask
    }

    /// Convert a packed pixel value into RGBA.
    #[inline]
    pub fn value(&self, v: u32) -> [u8; 4] {
        let c = self.split(v);
        [self.channel(0, c[0]), self.channel(1, c[1]), self.channel(2, c[2]), 255]
    }

    /// Read one pixel (`bpp` bytes) and convert it into RGBA.
    #[inline]
    pub fn pixel(&self, b: &[u8]) -> [u8; 4] {
        if self.rgbx {
            return [b[0], b[1], b[2], 255];
        }
        self.value(self.raw(b))
    }

    /// Read one pixel as a packed integer.
    #[inline]
    pub fn raw(&self, b: &[u8]) -> u32 {
        match (self.bpp, self.pf.big_endian) {
            (1, _) => b[0] as u32,
            (2, false) => u16::from_le_bytes([b[0], b[1]]) as u32,
            (2, true) => u16::from_be_bytes([b[0], b[1]]) as u32,
            (_, false) => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            (_, true) => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        }
    }

    /// Convert a run of pixels. Converts as many as fit in both slices.
    pub fn row(&self, src: &[u8], dst: &mut [u8]) {
        let dst = dst.chunks_exact_mut(4);
        if self.rgbx {
            for (d, s) in dst.zip(src.chunks_exact(4)) {
                d.copy_from_slice(&[s[0], s[1], s[2], 255]);
            }
            return;
        }
        match (self.bpp, self.pf.big_endian) {
            (1, _) => {
                for (d, &s) in dst.zip(src) {
                    d.copy_from_slice(&self.value(s as u32));
                }
            }
            (2, false) => {
                for (d, s) in dst.zip(src.chunks_exact(2)) {
                    d.copy_from_slice(&self.value(u16::from_le_bytes([s[0], s[1]]) as u32));
                }
            }
            (2, true) => {
                for (d, s) in dst.zip(src.chunks_exact(2)) {
                    d.copy_from_slice(&self.value(u16::from_be_bytes([s[0], s[1]]) as u32));
                }
            }
            (_, false) => {
                for (d, s) in dst.zip(src.chunks_exact(4)) {
                    d.copy_from_slice(&self.value(u32::from_le_bytes([s[0], s[1], s[2], s[3]])));
                }
            }
            (_, true) => {
                for (d, s) in dst.zip(src.chunks_exact(4)) {
                    d.copy_from_slice(&self.value(u32::from_be_bytes([s[0], s[1], s[2], s[3]])));
                }
            }
        }
    }
}

/// Reader over a block of decompressed bytes.
pub(crate) struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Bytes<'a> {
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Bytes { data, pos: 0, what }
    }

    #[inline]
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        let b = *self.data.get(self.pos).ok_or_else(|| self.short())?;
        self.pos += 1;
        Ok(b)
    }

    #[inline]
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or_else(|| self.short())?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn short(&self) -> DecodeError {
        invalid(format!("{} data ended too early", self.what))
    }
}

/// Read `len` bytes into `buf`, replacing its contents. The buffer grows
/// only as the bytes arrive, so a false length cannot make us reserve a
/// huge block of memory up front.
pub(crate) async fn read_into(r: &mut RfbReader, buf: &mut Vec<u8>, len: usize) -> Result<(), DecodeError> {
    const STEP: usize = 1 << 20;
    buf.clear();
    while buf.len() < len {
        let start = buf.len();
        buf.resize(start + (len - start).min(STEP), 0);
        r.read_exact(&mut buf[start..]).await?;
    }
    Ok(())
}

/// Output grows in steps of this size while inflating.
const INFLATE_STEP: usize = 1 << 20;

/// Inflate all of `input` with a zlib stream that lives for the whole
/// connection, replacing the contents of `out`. Fails when the output would
/// be larger than `limit` bytes.
pub(crate) fn inflate(z: &mut Decompress, input: &[u8], out: &mut Vec<u8>, limit: usize) -> Result<(), DecodeError> {
    out.clear();
    let mut pos = 0;
    loop {
        if out.len() == out.capacity() {
            if out.len() > limit {
                break;
            }
            out.reserve((limit + 1 - out.len()).clamp(1, INFLATE_STEP));
        }
        let (in_before, out_before) = (z.total_in(), z.total_out());
        let status = z
            .decompress_vec(&input[pos..], out, FlushDecompress::Sync)
            .map_err(|e| invalid(format!("compressed data is damaged: {e}")))?;
        let consumed = (z.total_in() - in_before) as usize;
        let produced = (z.total_out() - out_before) as usize;
        pos += consumed;
        let room_left = out.len() < out.capacity();
        if status == Status::StreamEnd || (pos >= input.len() && room_left) {
            break;
        }
        if consumed == 0 && produced == 0 && room_left {
            // No progress although there is room: the input is cut short.
            break;
        }
    }
    if out.len() > limit {
        return Err(invalid("compressed data unpacks to more than the rectangle can hold"));
    }
    Ok(())
}

/// Copy tightly packed RGBA for `r` into the framebuffer, clipped to its
/// edges. Does not mark anything as changed.
pub(crate) fn blit(fb: &mut Framebuffer, r: Rect, rgba: &[u8]) {
    let stride = r.w as usize * 4;
    for row in 0..r.h {
        let Some(y) = r.y.checked_add(row) else { break };
        let dst = fb.row_mut(r.x, y, r.w);
        if dst.is_empty() {
            break;
        }
        let off = row as usize * stride;
        let Some(src) = rgba.get(off..off + dst.len()) else { break };
        dst.copy_from_slice(src);
    }
}

/// Fill a rectangle of the framebuffer with one colour, clipped to its
/// edges. Does not mark anything as changed.
pub(crate) fn fill(fb: &mut Framebuffer, r: Rect, rgba: [u8; 4]) {
    for row in 0..r.h {
        let Some(y) = r.y.checked_add(row) else { break };
        let dst = fb.row_mut(r.x, y, r.w);
        if dst.is_empty() {
            break;
        }
        for px in dst.chunks_exact_mut(4) {
            px.copy_from_slice(&rgba);
        }
    }
}

/// Fill part of a tightly packed RGBA buffer that is `buf_w` pixels wide.
pub(crate) fn fill_buf(buf: &mut [u8], buf_w: usize, x: usize, y: usize, w: usize, h: usize, rgba: [u8; 4]) {
    for row in y..y + h {
        let start = (row * buf_w + x) * 4;
        let Some(dst) = buf.get_mut(start..start + w * 4) else { return };
        for px in dst.chunks_exact_mut(4) {
            px.copy_from_slice(&rgba);
        }
    }
}

/// Rectangle at an offset inside `outer`, or `None` when it would not fit
/// in 16-bit coordinates (it is then outside any framebuffer anyway).
pub(crate) fn sub_rect(outer: Rect, x: usize, y: usize, w: usize, h: usize) -> Option<Rect> {
    let x = u16::try_from(outer.x as usize + x).ok()?;
    let y = u16::try_from(outer.y as usize + y).ok()?;
    Some(Rect::new(x, y, w as u16, h as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_common_formats() {
        let c = Converter::new(&PixelFormat::RGBX);
        assert_eq!(c.pixel(&[1, 2, 3, 0]), [1, 2, 3, 255]);
        let c = Converter::new(&PixelFormat::RGB565);
        // Pure red, little-endian.
        assert_eq!(c.pixel(&0xf800u16.to_le_bytes()), [255, 0, 0, 255]);
        let mut out = [0u8; 8];
        c.row(&[0x1f, 0x00, 0xe0, 0x07], &mut out);
        assert_eq!(out, [0, 0, 255, 255, 0, 255, 0, 255]);
        let c = Converter::new(&PixelFormat::BGR233);
        assert_eq!(c.pixel(&[0b11_000_111]), [255, 0, 255, 255]);
    }

    #[test]
    fn big_endian_rgbx_is_byte_identical() {
        let pf = PixelFormat { big_endian: true, red_shift: 24, green_shift: 16, blue_shift: 8, ..PixelFormat::RGBX };
        let c = Converter::new(&pf);
        let mut out = [0u8; 4];
        c.row(&[10, 20, 30, 0], &mut out);
        assert_eq!(out, [10, 20, 30, 255]);
    }

    #[test]
    fn inflate_keeps_stream_and_checks_limit() {
        use flate2::{Compress, Compression, FlushCompress};
        let mut comp = Compress::new(Compression::default(), true);
        let mut z = Decompress::new(true);
        let mut out = Vec::new();
        for part in [&b"hello hello hello"[..], &b"second part"[..]] {
            let mut packed = Vec::with_capacity(256);
            comp.compress_vec(part, &mut packed, FlushCompress::Sync).unwrap();
            inflate(&mut z, &packed, &mut out, 1000).unwrap();
            assert_eq!(out, part);
        }
        let mut packed = Vec::with_capacity(256);
        comp.compress_vec(&[7u8; 100], &mut packed, FlushCompress::Sync).unwrap();
        assert!(inflate(&mut z, &packed, &mut out, 99).is_err());
    }
}
