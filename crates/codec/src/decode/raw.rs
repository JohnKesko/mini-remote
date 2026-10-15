//! Raw encoding: every pixel of the rectangle, row by row.

use super::DecodeError;
use super::convert::Converter;
use crate::{Framebuffer, Rect, RfbReader};

/// Rows are read from the network in blocks of about this many bytes.
const CHUNK_BYTES: usize = 256 * 1024;

pub(crate) async fn decode(
    rect: Rect,
    conv: &Converter,
    r: &mut RfbReader,
    fb: &mut Framebuffer,
    buf: &mut Vec<u8>,
) -> Result<(), DecodeError> {
    if rect.is_empty() {
        return Ok(());
    }
    let bpp = conv.bpp();
    let row_len = rect.w as usize * bpp;
    let rows_per_chunk = (CHUNK_BYTES / row_len).max(1);
    let mut row = 0usize;
    while row < rect.h as usize {
        let n = rows_per_chunk.min(rect.h as usize - row);
        buf.resize(n * row_len, 0);
        r.read_exact(&mut buf[..n * row_len]).await?;
        for (i, src) in buf[..n * row_len].chunks_exact(row_len).enumerate() {
            let Ok(y) = u16::try_from(rect.y as usize + row + i) else { break };
            conv.row(src, fb.row_mut(rect.x, y, rect.w));
        }
        row += n;
    }
    fb.mark_dirty(rect);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;

    #[tokio::test]
    async fn raw_rgbx_and_clipping() {
        let mut fb = Framebuffer::new(3, 2);
        let data = vec![1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0, 10, 11, 12, 0];
        let mut r = RfbReader::from_bytes(data);
        let conv = Converter::new(&PixelFormat::RGBX);
        let mut buf = Vec::new();
        // 2x2 at x=2: the right column falls outside the framebuffer.
        decode(Rect::new(2, 0, 2, 2), &conv, &mut r, &mut fb, &mut buf).await.unwrap();
        assert_eq!(fb.read_rect(Rect::new(2, 0, 1, 2)), vec![1, 2, 3, 255, 7, 8, 9, 255]);
        assert_eq!(r.buffered(), 0);
    }

    #[tokio::test]
    async fn raw_rgb565() {
        let mut fb = Framebuffer::new(2, 1);
        let mut data = Vec::new();
        data.extend_from_slice(&0xf800u16.to_le_bytes());
        data.extend_from_slice(&0x07e0u16.to_le_bytes());
        let mut r = RfbReader::from_bytes(data);
        let conv = Converter::new(&PixelFormat::RGB565);
        decode(Rect::new(0, 0, 2, 1), &conv, &mut r, &mut fb, &mut Vec::new()).await.unwrap();
        assert_eq!(fb.read_rect(Rect::new(0, 0, 2, 1)), vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }
}
