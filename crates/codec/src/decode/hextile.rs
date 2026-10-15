//! Hextile encoding: 16x16 tiles, each raw or a small RRE picture.

use super::DecodeError;
use super::convert::{Converter, blit, fill, fill_buf, sub_rect};
use crate::{Framebuffer, Rect, RfbReader};

const RAW: u8 = 1;
const BACKGROUND: u8 = 2;
const FOREGROUND: u8 = 4;
const ANY_SUBRECTS: u8 = 8;
const SUBRECTS_COLOURED: u8 = 16;

async fn read_pixel(r: &mut RfbReader, conv: &Converter) -> Result<[u8; 4], DecodeError> {
    let mut px = [0u8; 4];
    r.read_exact(&mut px[..conv.bpp()]).await?;
    Ok(conv.pixel(&px[..conv.bpp()]))
}

pub(crate) async fn decode(
    rect: Rect,
    conv: &Converter,
    r: &mut RfbReader,
    fb: &mut Framebuffer,
    buf: &mut Vec<u8>,
) -> Result<(), DecodeError> {
    let bpp = conv.bpp();
    let (w, h) = (rect.w as usize, rect.h as usize);
    let mut tile = [0u8; 16 * 16 * 4];
    let mut bg = [0, 0, 0, 255];
    let mut fg = [0, 0, 0, 255];
    for ty in (0..h).step_by(16) {
        let th = (h - ty).min(16);
        for tx in (0..w).step_by(16) {
            let tw = (w - tx).min(16);
            let target = sub_rect(rect, tx, ty, tw, th);
            let mask = r.read_u8().await?;
            if mask & RAW != 0 {
                let n = tw * th * bpp;
                buf.resize(n, 0);
                r.read_exact(&mut buf[..n]).await?;
                conv.row(&buf[..n], &mut tile[..tw * th * 4]);
                if let Some(t) = target {
                    blit(fb, t, &tile[..tw * th * 4]);
                }
                continue;
            }
            if mask & BACKGROUND != 0 {
                bg = read_pixel(r, conv).await?;
            }
            if mask & FOREGROUND != 0 {
                fg = read_pixel(r, conv).await?;
            }
            if mask & ANY_SUBRECTS == 0 {
                if let Some(t) = target {
                    fill(fb, t, bg);
                }
                continue;
            }
            let count = r.read_u8().await? as usize;
            let coloured = mask & SUBRECTS_COLOURED != 0;
            let item = if coloured { bpp + 2 } else { 2 };
            buf.resize(count * item, 0);
            r.read_exact(&mut buf[..count * item]).await?;
            fill_buf(&mut tile, tw, 0, 0, tw, th, bg);
            for sub in buf[..count * item].chunks_exact(item) {
                let (colour, xy, wh) =
                    if coloured { (conv.pixel(&sub[..bpp]), sub[bpp], sub[bpp + 1]) } else { (fg, sub[0], sub[1]) };
                let (sx, sy) = ((xy >> 4) as usize, (xy & 15) as usize);
                let sw = ((wh >> 4) as usize + 1).min(tw.saturating_sub(sx));
                let sh = ((wh & 15) as usize + 1).min(th.saturating_sub(sy));
                fill_buf(&mut tile, tw, sx, sy, sw, sh, colour);
            }
            if let Some(t) = target {
                blit(fb, t, &tile[..tw * th * 4]);
            }
        }
    }
    fb.mark_dirty(rect);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;

    fn px(fb: &Framebuffer, x: u16, y: u16) -> [u8; 3] {
        let v = fb.read_rect(Rect::new(x, y, 1, 1));
        [v[0], v[1], v[2]]
    }

    #[tokio::test]
    async fn hextile_tiles_with_subrects() {
        // A 20x18 rectangle has four tiles: 16x16, 4x16, 16x2 and 4x2.
        let mut d = Vec::new();
        // Tile 1: background + foreground + two plain subrects.
        d.push(BACKGROUND | FOREGROUND | ANY_SUBRECTS);
        d.extend_from_slice(&[10, 10, 10, 0]);
        d.extend_from_slice(&[200, 0, 0, 0]);
        d.push(2);
        d.extend_from_slice(&[0x00, 0x00]); // (0,0) 1x1
        d.extend_from_slice(&[0x23, 0x21]); // (2,3) 3x2
        // Tile 2: raw 4x16.
        d.push(RAW);
        for i in 0..64u8 {
            d.extend_from_slice(&[i, 0, 0, 0]);
        }
        // Tile 3: background carried over, coloured subrects.
        d.push(ANY_SUBRECTS | SUBRECTS_COLOURED);
        d.push(1);
        d.extend_from_slice(&[0, 0, 99, 0, 0x51, 0x10]); // (5,1) 2x1
        // Tile 4: only background, carried over from tile 3.
        d.push(0);
        let mut fb = Framebuffer::new(20, 18);
        let mut r = RfbReader::from_bytes(d);
        let conv = Converter::new(&PixelFormat::RGBX);
        decode(Rect::new(0, 0, 20, 18), &conv, &mut r, &mut fb, &mut Vec::new()).await.unwrap();
        assert_eq!(r.buffered(), 0);
        assert_eq!(px(&fb, 0, 0), [200, 0, 0]);
        assert_eq!(px(&fb, 1, 0), [10, 10, 10]);
        assert_eq!(px(&fb, 2, 3), [200, 0, 0]);
        assert_eq!(px(&fb, 4, 4), [200, 0, 0]);
        assert_eq!(px(&fb, 5, 4), [10, 10, 10]);
        assert_eq!(px(&fb, 4, 5), [10, 10, 10]);
        assert_eq!(px(&fb, 16, 0), [0, 0, 0]);
        assert_eq!(px(&fb, 19, 15), [63, 0, 0]);
        assert_eq!(px(&fb, 5, 17), [0, 0, 99]);
        assert_eq!(px(&fb, 6, 17), [0, 0, 99]);
        assert_eq!(px(&fb, 7, 17), [10, 10, 10]);
        assert_eq!(px(&fb, 5, 16), [10, 10, 10]);
        assert_eq!(px(&fb, 18, 17), [10, 10, 10]);
    }

    #[tokio::test]
    async fn hextile_rgb565_solid() {
        let mut d = vec![BACKGROUND];
        d.extend_from_slice(&0x001fu16.to_le_bytes());
        let mut fb = Framebuffer::new(4, 4);
        let mut r = RfbReader::from_bytes(d);
        let conv = Converter::new(&PixelFormat::RGB565);
        decode(Rect::new(0, 0, 4, 4), &conv, &mut r, &mut fb, &mut Vec::new()).await.unwrap();
        assert_eq!(px(&fb, 3, 3), [0, 0, 255]);
    }
}
