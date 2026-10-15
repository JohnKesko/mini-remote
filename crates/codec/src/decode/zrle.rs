//! ZRLE encoding: zlib-compressed 64x64 tiles, each raw, solid, packed
//! palette or run-length encoded. One zlib stream lives for the whole
//! connection.

use flate2::Decompress;

use super::convert::{Bytes, Converter, blit, fill, inflate, invalid, read_into, sub_rect};
use super::{DecodeError, MAX_INPUT_BYTES, MAX_RECT_PIXELS, Scratch};
use crate::{Framebuffer, Rect, RfbReader};

const TILE: usize = 64;

/// How a compressed pixel (`CPIXEL`) is laid out.
#[derive(Clone, Copy)]
enum Layout {
    /// Three bytes: red, green, blue.
    Rgb,
    /// Three bytes holding the low or high part of a 32-bit pixel.
    Three { shift: u32, big_endian: bool },
    /// A whole pixel.
    Whole,
}

struct CPixels<'a> {
    conv: &'a Converter,
    layout: Layout,
    size: usize,
}

impl<'a> CPixels<'a> {
    fn new(conv: &'a Converter) -> Self {
        let pf = conv.format();
        let layout = match pf.cpixel_shift() {
            // Both byte orders of RGBX put red, green, blue first on the wire.
            Some(_) if pf.is_rgbx() => Layout::Rgb,
            Some(shift) => Layout::Three { shift, big_endian: pf.big_endian },
            None => Layout::Whole,
        };
        CPixels { conv, layout, size: pf.bytes_per_cpixel() }
    }

    #[inline]
    fn rgba(&self, b: &[u8]) -> [u8; 4] {
        match self.layout {
            Layout::Rgb => [b[0], b[1], b[2], 255],
            Layout::Three { shift, big_endian } => self.conv.value(three(b, big_endian) << shift),
            Layout::Whole => self.conv.pixel(b),
        }
    }

    /// Convert a run of `CPIXEL`s.
    fn row(&self, src: &[u8], dst: &mut [u8]) {
        match self.layout {
            Layout::Rgb => {
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
                    d.copy_from_slice(&[s[0], s[1], s[2], 255]);
                }
            }
            Layout::Three { shift, big_endian } => {
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
                    d.copy_from_slice(&self.conv.value(three(s, big_endian) << shift));
                }
            }
            Layout::Whole => self.conv.row(src, dst),
        }
    }
}

#[inline]
fn three(b: &[u8], big_endian: bool) -> u32 {
    if big_endian {
        (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32
    } else {
        b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16
    }
}

/// Read a run length: 1 plus the sum of bytes up to the first one below 255.
#[inline]
fn run_length(cur: &mut Bytes) -> Result<usize, DecodeError> {
    let mut len = 1usize;
    loop {
        let b = cur.u8()?;
        len += b as usize;
        if b != 255 {
            return Ok(len);
        }
    }
}

/// What a decoded tile turned out to be.
enum Tile {
    Solid([u8; 4]),
    Pixels,
}

fn decode_tile(cur: &mut Bytes, cp: &CPixels, tw: usize, th: usize, out: &mut [u8]) -> Result<Tile, DecodeError> {
    let sub = cur.u8()?;
    let total = tw * th;
    let mut palette = [[0u8, 0, 0, 255]; 128];
    let mut read_palette = |cur: &mut Bytes, n: usize| -> Result<(), DecodeError> {
        let data = cur.take(n * cp.size)?;
        for (p, s) in palette.iter_mut().zip(data.chunks_exact(cp.size)) {
            *p = cp.rgba(s);
        }
        Ok(())
    };
    match sub {
        0 => {
            let data = cur.take(total * cp.size)?;
            cp.row(data, out);
        }
        1 => return Ok(Tile::Solid(cp.rgba(cur.take(cp.size)?))),
        2..=16 => {
            let n = sub as usize;
            read_palette(cur, n)?;
            let bits = match n {
                2 => 1,
                3 | 4 => 2,
                _ => 4,
            };
            let row_bytes = (tw * bits).div_ceil(8);
            let data = cur.take(row_bytes * th)?;
            let mask = (1u8 << bits) - 1;
            for (src, dst) in data.chunks_exact(row_bytes).zip(out.chunks_exact_mut(tw * 4)) {
                let mut px = dst.chunks_exact_mut(4);
                'row: for &byte in src {
                    let mut shift = 8 - bits as i32;
                    while shift >= 0 {
                        let Some(p) = px.next() else { break 'row };
                        p.copy_from_slice(&palette[((byte >> shift) & mask) as usize]);
                        shift -= bits as i32;
                    }
                }
            }
        }
        128 => {
            let mut i = 0;
            while i < total {
                let colour = cp.rgba(cur.take(cp.size)?);
                let run = run_length(cur)?;
                if run > total - i {
                    return Err(invalid("a ZRLE run goes past the end of its tile"));
                }
                for p in out[i * 4..(i + run) * 4].chunks_exact_mut(4) {
                    p.copy_from_slice(&colour);
                }
                i += run;
            }
        }
        130..=255 => {
            read_palette(cur, sub as usize - 128)?;
            let mut i = 0;
            while i < total {
                let index = cur.u8()?;
                let run = if index & 0x80 != 0 { run_length(cur)? } else { 1 };
                if run > total - i {
                    return Err(invalid("a ZRLE run goes past the end of its tile"));
                }
                let colour = palette[(index & 0x7f) as usize];
                for p in out[i * 4..(i + run) * 4].chunks_exact_mut(4) {
                    p.copy_from_slice(&colour);
                }
                i += run;
            }
        }
        _ => return Err(invalid(format!("unknown ZRLE tile type {sub}"))),
    }
    Ok(Tile::Pixels)
}

pub(crate) struct Zrle {
    z: Decompress,
}

impl Zrle {
    pub fn new() -> Self {
        Zrle { z: Decompress::new(true) }
    }

    pub async fn decode(
        &mut self,
        rect: Rect,
        conv: &Converter,
        r: &mut RfbReader,
        fb: &mut Framebuffer,
        s: &mut Scratch,
    ) -> Result<(), DecodeError> {
        let len = r.read_u32().await? as usize;
        let (w, h) = (rect.w as usize, rect.h as usize);
        if w * h > MAX_RECT_PIXELS {
            return Err(invalid("ZRLE rectangle is too large"));
        }
        let cp = CPixels::new(conv);
        // Worst case: every pixel is a run of one, plus a full palette per tile.
        let tiles = w.div_ceil(TILE) * h.div_ceil(TILE);
        let limit = w * h * (cp.size + 1) + tiles * (1 + 127 * cp.size) + 16;
        // zlib never grows data by more than a few bytes per block.
        if len > MAX_INPUT_BYTES.min(limit + limit / 8 + 4096) {
            return Err(invalid(format!("ZRLE rectangle of {len} bytes is too large")));
        }
        read_into(r, &mut s.input, len).await?;
        inflate(&mut self.z, &s.input, &mut s.inflated, limit)?;
        if rect.is_empty() {
            return Ok(());
        }

        let mut cur = Bytes::new(&s.inflated, "ZRLE");
        s.rgba.resize(TILE * TILE * 4, 0);
        for ty in (0..h).step_by(TILE) {
            let th = (h - ty).min(TILE);
            for tx in (0..w).step_by(TILE) {
                let tw = (w - tx).min(TILE);
                let out = &mut s.rgba[..tw * th * 4];
                let tile = decode_tile(&mut cur, &cp, tw, th, out)?;
                let Some(target) = sub_rect(rect, tx, ty, tw, th) else { continue };
                match tile {
                    Tile::Solid(c) => fill(fb, target, c),
                    Tile::Pixels => blit(fb, target, out),
                }
            }
        }
        fb.mark_dirty(rect);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;
    use flate2::{Compress, Compression, FlushCompress};

    /// Server side of the connection-wide zlib stream.
    struct Server(Compress);

    impl Server {
        fn new() -> Self {
            Server(Compress::new(Compression::default(), true))
        }

        fn rect(&mut self, raw: &[u8]) -> Vec<u8> {
            let mut packed = Vec::with_capacity(raw.len() + 1024);
            self.0.compress_vec(raw, &mut packed, FlushCompress::Sync).unwrap();
            let mut out = (packed.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(&packed);
            out
        }
    }

    async fn run(pf: &PixelFormat, rect: Rect, fb: &mut Framebuffer, z: &mut Zrle, wire: Vec<u8>) {
        let mut r = RfbReader::from_bytes(wire);
        let conv = Converter::new(pf);
        let mut s = Scratch::default();
        z.decode(rect, &conv, &mut r, fb, &mut s).await.unwrap();
        assert_eq!(r.buffered(), 0);
    }

    fn px(fb: &Framebuffer, x: u16, y: u16) -> [u8; 3] {
        let v = fb.read_rect(Rect::new(x, y, 1, 1));
        [v[0], v[1], v[2]]
    }

    #[tokio::test]
    async fn zrle_all_tile_types_over_one_stream() {
        let pf = PixelFormat::RGBX;
        let mut server = Server::new();
        let mut z = Zrle::new();
        let mut fb = Framebuffer::new(70, 8);

        // Rectangle 1 (70x2): two tiles, a raw 64x2 tile and a solid 6x2 tile.
        let mut t = vec![0u8];
        for i in 0..128u32 {
            t.extend_from_slice(&[i as u8, 1, 2]);
        }
        t.push(1);
        t.extend_from_slice(&[9, 8, 7]);
        run(&pf, Rect::new(0, 0, 70, 2), &mut fb, &mut z, server.rect(&t)).await;
        assert_eq!(px(&fb, 0, 0), [0, 1, 2]);
        assert_eq!(px(&fb, 63, 1), [127, 1, 2]);
        assert_eq!(px(&fb, 64, 0), [9, 8, 7]);
        assert_eq!(px(&fb, 69, 1), [9, 8, 7]);

        // Rectangle 2 (5x2): packed palette with 2 colours (1 bit per pixel).
        let mut t = vec![2u8, 10, 0, 0, 20, 0, 0];
        t.extend_from_slice(&[0b1010_1000, 0b0111_1000]);
        run(&pf, Rect::new(0, 2, 5, 2), &mut fb, &mut z, server.rect(&t)).await;
        let row: Vec<u8> = (0..5).map(|x| px(&fb, x, 2)[0]).collect();
        assert_eq!(row, vec![20, 10, 20, 10, 20]);
        let row: Vec<u8> = (0..5).map(|x| px(&fb, x, 3)[0]).collect();
        assert_eq!(row, vec![10, 20, 20, 20, 20]);

        // Rectangle 3 (3x1): packed palette with 3 colours (2 bits per pixel).
        let t = vec![3u8, 1, 0, 0, 2, 0, 0, 3, 0, 0, 0b10_01_00_00];
        run(&pf, Rect::new(0, 4, 3, 1), &mut fb, &mut z, server.rect(&t)).await;
        let row: Vec<u8> = (0..3).map(|x| px(&fb, x, 4)[0]).collect();
        assert_eq!(row, vec![3, 2, 1]);

        // Rectangle 4 (3x1): packed palette with 5 colours (4 bits per pixel).
        let mut t = vec![5u8];
        for c in 0..5u8 {
            t.extend_from_slice(&[c * 10, 0, 0]);
        }
        t.extend_from_slice(&[0x42, 0x10]);
        run(&pf, Rect::new(0, 5, 3, 1), &mut fb, &mut z, server.rect(&t)).await;
        let row: Vec<u8> = (0..3).map(|x| px(&fb, x, 5)[0]).collect();
        assert_eq!(row, vec![40, 20, 10]);

        // Rectangle 5 (300 pixels as 60x5): plain RLE, runs of 256 and 44.
        let t = vec![128u8, 50, 0, 0, 255, 0, 60, 0, 0, 43];
        run(&pf, Rect::new(0, 0, 60, 5), &mut fb, &mut z, server.rect(&t)).await;
        assert_eq!(px(&fb, 0, 0), [50, 0, 0]);
        assert_eq!(px(&fb, 15, 4), [50, 0, 0]); // pixel 255
        assert_eq!(px(&fb, 16, 4), [60, 0, 0]); // pixel 256
        assert_eq!(px(&fb, 59, 4), [60, 0, 0]);

        // Rectangle 6 (4x2): palette RLE with single pixels and a run.
        let t = vec![130u8, 1, 1, 1, 2, 2, 2, 0, 1, 0x80, 5];
        run(&pf, Rect::new(0, 6, 4, 2), &mut fb, &mut z, server.rect(&t)).await;
        let all: Vec<u8> = (0..8).map(|i| px(&fb, i % 4, 6 + i / 4)[0]).collect();
        assert_eq!(all, vec![1, 2, 1, 1, 1, 1, 1, 1]);
    }

    #[tokio::test]
    async fn zrle_rejects_overlong_run() {
        let mut server = Server::new();
        let mut z = Zrle::new();
        let mut fb = Framebuffer::new(4, 4);
        let wire = server.rect(&[128, 1, 2, 3, 20]);
        let mut r = RfbReader::from_bytes(wire);
        let conv = Converter::new(&PixelFormat::RGBX);
        let res = z.decode(Rect::new(0, 0, 4, 4), &conv, &mut r, &mut fb, &mut Scratch::default()).await;
        assert!(matches!(res, Err(DecodeError::Invalid(_))));
    }

    /// 32 bits per pixel with the colours in the high three bytes.
    fn high_bytes(big_endian: bool) -> PixelFormat {
        PixelFormat { big_endian, red_shift: 8, green_shift: 16, blue_shift: 24, ..PixelFormat::RGBX }
    }

    #[tokio::test]
    async fn zrle_cpixel_layouts() {
        assert_eq!(PixelFormat::RGBX.cpixel_shift(), Some(0));
        assert_eq!(high_bytes(false).cpixel_shift(), Some(8));
        assert_eq!(PixelFormat { depth: 32, ..PixelFormat::RGBX }.bytes_per_cpixel(), 4);
        assert_eq!(PixelFormat::RGB565.bytes_per_cpixel(), 2);

        // Value 0xBBGGRR00 with R=1, G=2, B=3.
        let cases = [
            (high_bytes(false), [1u8, 2, 3]), // little-endian: low byte first, top three bytes sent
            (high_bytes(true), [3u8, 2, 1]),  // big-endian: high byte first
            // Low three bytes, big-endian: value 0x00BBGGRR sent as BB GG RR.
            (PixelFormat { big_endian: true, ..PixelFormat::RGBX }, [3u8, 2, 1]),
            // Big-endian RGBX (red in the top byte) still sends R, G, B.
            (
                PixelFormat { big_endian: true, red_shift: 24, green_shift: 16, blue_shift: 8, ..PixelFormat::RGBX },
                [1, 2, 3],
            ),
        ];
        for (pf, cpixel) in cases {
            let mut server = Server::new();
            let mut z = Zrle::new();
            let mut fb = Framebuffer::new(1, 1);
            let mut t = vec![1u8];
            t.extend_from_slice(&cpixel);
            run(&pf, Rect::new(0, 0, 1, 1), &mut fb, &mut z, server.rect(&t)).await;
            assert_eq!(px(&fb, 0, 0), [1, 2, 3], "{pf:?}");
        }
    }

    #[tokio::test]
    async fn zrle_rgb565_raw_tile() {
        let mut server = Server::new();
        let mut z = Zrle::new();
        let mut fb = Framebuffer::new(2, 1);
        let mut t = vec![0u8];
        t.extend_from_slice(&0xf800u16.to_le_bytes());
        t.extend_from_slice(&0x001fu16.to_le_bytes());
        run(&PixelFormat::RGB565, Rect::new(0, 0, 2, 1), &mut fb, &mut z, server.rect(&t)).await;
        assert_eq!(px(&fb, 0, 0), [255, 0, 0]);
        assert_eq!(px(&fb, 1, 0), [0, 0, 255]);
    }
}
