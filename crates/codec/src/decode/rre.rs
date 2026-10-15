//! RRE encoding: a background colour plus a list of solid rectangles.

use super::DecodeError;
use super::convert::{Converter, fill, invalid, sub_rect};
use crate::{Framebuffer, Rect, RfbReader};

/// Sub-rectangles are read from the network in batches of this many.
const BATCH: usize = 4096;

pub(crate) async fn decode(
    rect: Rect,
    conv: &Converter,
    r: &mut RfbReader,
    fb: &mut Framebuffer,
    buf: &mut Vec<u8>,
) -> Result<(), DecodeError> {
    let count = r.read_u32().await? as usize;
    let bpp = conv.bpp();
    let mut px = [0u8; 4];
    r.read_exact(&mut px[..bpp]).await?;
    fill(fb, rect, conv.pixel(&px[..bpp]));

    let item = bpp + 8;
    let mut left = count;
    while left > 0 {
        let n = left.min(BATCH);
        buf.resize(n * item, 0);
        r.read_exact(&mut buf[..n * item]).await?;
        for sub in buf[..n * item].chunks_exact(item) {
            let colour = conv.pixel(&sub[..bpp]);
            let v = |i: usize| u16::from_be_bytes([sub[bpp + i], sub[bpp + i + 1]]) as usize;
            let (x, y, w, h) = (v(0), v(2), v(4), v(6));
            if x + w > rect.w as usize || y + h > rect.h as usize {
                return Err(invalid("an RRE sub-rectangle lies outside its rectangle"));
            }
            if let Some(s) = sub_rect(rect, x, y, w, h) {
                fill(fb, s, colour);
            }
        }
        left -= n;
    }
    fb.mark_dirty(rect);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;

    fn subrect(out: &mut Vec<u8>, px: [u8; 4], x: u16, y: u16, w: u16, h: u16) {
        out.extend_from_slice(&px);
        for v in [x, y, w, h] {
            out.extend_from_slice(&v.to_be_bytes());
        }
    }

    #[tokio::test]
    async fn rre_background_and_subrects() {
        let mut data = 2u32.to_be_bytes().to_vec();
        data.extend_from_slice(&[10, 10, 10, 0]);
        subrect(&mut data, [200, 0, 0, 0], 0, 0, 1, 1);
        subrect(&mut data, [0, 200, 0, 0], 1, 1, 2, 1);
        let mut fb = Framebuffer::new(4, 3);
        let mut r = RfbReader::from_bytes(data);
        let conv = Converter::new(&PixelFormat::RGBX);
        decode(Rect::new(1, 1, 3, 2), &conv, &mut r, &mut fb, &mut Vec::new()).await.unwrap();
        let px = |x, y| fb.read_rect(Rect::new(x, y, 1, 1));
        assert_eq!(px(0, 0), vec![0, 0, 0, 255]);
        assert_eq!(px(1, 1), vec![200, 0, 0, 255]);
        assert_eq!(px(2, 1), vec![10, 10, 10, 255]);
        assert_eq!(px(2, 2), vec![0, 200, 0, 255]);
        assert_eq!(px(3, 2), vec![0, 200, 0, 255]);
        assert_eq!(px(1, 2), vec![10, 10, 10, 255]);
    }

    #[tokio::test]
    async fn rre_rejects_subrect_outside() {
        let mut data = 1u32.to_be_bytes().to_vec();
        data.extend_from_slice(&[0, 0, 0, 0]);
        subrect(&mut data, [1, 1, 1, 0], 2, 0, 2, 1);
        let mut fb = Framebuffer::new(4, 4);
        let mut r = RfbReader::from_bytes(data);
        let conv = Converter::new(&PixelFormat::RGBX);
        let res = decode(Rect::new(0, 0, 3, 3), &conv, &mut r, &mut fb, &mut Vec::new()).await;
        assert!(matches!(res, Err(DecodeError::Invalid(_))));
    }
}
