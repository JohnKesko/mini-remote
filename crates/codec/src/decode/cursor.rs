//! Cursor pseudo-encoding: the pointer picture plus a bitmask saying which
//! pixels are visible.

use super::convert::{Converter, invalid};
use super::{CursorImage, DecodeError};
use crate::{Rect, RfbReader};

/// Largest cursor we accept, in pixels.
const MAX_CURSOR_PIXELS: usize = 1 << 20;

pub(crate) async fn decode(
    rect: Rect,
    conv: &Converter,
    r: &mut RfbReader,
    buf: &mut Vec<u8>,
) -> Result<CursorImage, DecodeError> {
    let (w, h) = (rect.w as usize, rect.h as usize);
    if w == 0 || h == 0 {
        // No pixels and no mask: the cursor is hidden.
        return Ok(CursorImage { width: 0, height: 0, hotspot_x: rect.x, hotspot_y: rect.y, rgba: Vec::new() });
    }
    if w * h > MAX_CURSOR_PIXELS {
        return Err(invalid(format!("the cursor is too large ({w}x{h})")));
    }
    let pixels_len = w * h * conv.bpp();
    let mask_row = w.div_ceil(8);
    buf.resize(pixels_len + mask_row * h, 0);
    r.read_exact(buf).await?;
    let (pixels, mask) = buf.split_at(pixels_len);

    let mut rgba = vec![0u8; w * h * 4];
    conv.row(pixels, &mut rgba);
    for (bits, row) in mask.chunks_exact(mask_row).zip(rgba.chunks_exact_mut(w * 4)) {
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            if (bits[x / 8] >> (7 - x % 8)) & 1 == 0 {
                px.copy_from_slice(&[0; 4]);
            }
        }
    }
    Ok(CursorImage { width: rect.w, height: rect.h, hotspot_x: rect.x, hotspot_y: rect.y, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;

    #[tokio::test]
    async fn cursor_with_mask() {
        // 9x2 cursor: mask rows are 2 bytes each.
        let mut data = Vec::new();
        for i in 0..18u8 {
            data.extend_from_slice(&[i, 100, 200, 0]);
        }
        data.extend_from_slice(&[0b1000_0001, 0b1000_0000, 0b0000_0000, 0b0000_0000]);
        let mut r = RfbReader::from_bytes(data);
        let conv = Converter::new(&PixelFormat::RGBX);
        let c = decode(Rect::new(3, 4, 9, 2), &conv, &mut r, &mut Vec::new()).await.unwrap();
        assert_eq!((c.width, c.height, c.hotspot_x, c.hotspot_y), (9, 2, 3, 4));
        assert_eq!(c.rgba.len(), 9 * 2 * 4);
        let alpha: Vec<u8> = c.rgba.chunks_exact(4).map(|p| p[3]).collect();
        let mut want = vec![0u8; 18];
        want[0] = 255;
        want[7] = 255;
        want[8] = 255;
        assert_eq!(alpha, want);
        assert_eq!(&c.rgba[..4], &[0, 100, 200, 255]);
        assert_eq!(&c.rgba[8 * 4..9 * 4], &[8, 100, 200, 255]);
        assert_eq!(r.buffered(), 0);
    }

    #[tokio::test]
    async fn cursor_rgb565_and_empty() {
        let mut data = 0xf800u16.to_le_bytes().to_vec();
        data.push(0x80);
        let mut r = RfbReader::from_bytes(data);
        let conv = Converter::new(&PixelFormat::RGB565);
        let c = decode(Rect::new(0, 0, 1, 1), &conv, &mut r, &mut Vec::new()).await.unwrap();
        assert_eq!(c.rgba, vec![255, 0, 0, 255]);

        let mut r = RfbReader::from_bytes(Vec::new());
        let c = decode(Rect::new(1, 1, 0, 5), &conv, &mut r, &mut Vec::new()).await.unwrap();
        assert_eq!((c.width, c.height), (0, 0));
        assert!(c.rgba.is_empty());
    }
}
