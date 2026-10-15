//! Decoders for the picture encodings a VNC server can send.
//!
//! The session reads each rectangle header (x, y, w, h, encoding) itself and
//! then hands the rest of the rectangle to [`Decoders::decode`]. Pseudo
//! encodings that only carry control data (desktop size, desktop name, ...)
//! stay in the session; the cursor shape is decoded here because it needs
//! the pixel format.

// `as_chunks` would need a newer Rust than the workspace minimum.
#![allow(clippy::chunks_exact_to_as_chunks)]

mod convert;
mod cursor;
mod h264;
mod hextile;
mod raw;
mod rre;
mod tight;
mod zrle;

use thiserror::Error;

use crate::{Framebuffer, PixelFormat, Rect, RfbReader};
use convert::{Converter, invalid};

/// Largest compressed block we accept for one rectangle.
const MAX_INPUT_BYTES: usize = 256 << 20;

/// Largest rectangle (in pixels) the compressed encodings will unpack.
/// 8K screens are about 33 million pixels.
const MAX_RECT_PIXELS: usize = 1 << 26;

/// Encoding numbers from the RFB specification.
pub mod encoding {
    pub const RAW: i32 = 0;
    pub const COPY_RECT: i32 = 1;
    pub const RRE: i32 = 2;
    pub const HEXTILE: i32 = 5;
    pub const TIGHT: i32 = 7;
    pub const ZRLE: i32 = 16;
    pub const OPEN_H264: i32 = 50;

    pub const DESKTOP_SIZE: i32 = -223;
    pub const LAST_RECT: i32 = -224;
    pub const CURSOR: i32 = -239;
    pub const QEMU_EXTENDED_KEY_EVENT: i32 = -258;
    pub const DESKTOP_NAME: i32 = -307;
    pub const EXTENDED_DESKTOP_SIZE: i32 = -308;
    pub const FENCE: i32 = -312;
    pub const CONTINUOUS_UPDATES: i32 = -313;
    pub const EXTENDED_MOUSE_BUTTONS: i32 = -316;
    pub const EXTENDED_CLIPBOARD: i32 = 0xC0A1_E5CE_u32 as i32;
    /// Tight JPEG quality 0..=9 is sent as `JPEG_QUALITY_0 + level`.
    pub const JPEG_QUALITY_0: i32 = -32;
    /// Tight compression level 0..=9 is sent as `COMPRESS_LEVEL_0 + level`.
    pub const COMPRESS_LEVEL_0: i32 = -256;

    /// Short human name, for the connection info panel.
    pub fn name(enc: i32) -> &'static str {
        match enc {
            RAW => "Raw",
            COPY_RECT => "CopyRect",
            RRE => "RRE",
            HEXTILE => "Hextile",
            TIGHT => "Tight",
            ZRLE => "ZRLE",
            OPEN_H264 => "Video (H.264)",
            _ => "Other",
        }
    }
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("network error: {0}")]
    Io(#[from] std::io::Error),
    #[error("the server sent picture data we could not understand: {0}")]
    Invalid(String),
    #[error("the server used an unsupported picture encoding ({0})")]
    Unsupported(i32),
}

/// Mouse pointer image from the Cursor pseudo-encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorImage {
    pub width: u16,
    pub height: u16,
    pub hotspot_x: u16,
    pub hotspot_y: u16,
    /// Straight (not premultiplied) RGBA, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Reusable buffers shared by the decoders, so a steady stream of updates
/// does not allocate.
#[derive(Default)]
struct Scratch {
    /// Bytes as they came from the network (compressed data, raw rows, ...).
    input: Vec<u8>,
    /// Decompressed bytes.
    inflated: Vec<u8>,
    /// Decoded RGBA pixels.
    rgba: Vec<u8>,
}

/// Per-connection decoder state (zlib streams, video decoders, ...).
pub struct Decoders {
    conv: Converter,
    zrle: zrle::Zrle,
    tight: tight::Tight,
    h264: h264::H264,
    scratch: Scratch,
}

impl Default for Decoders {
    fn default() -> Self {
        Self::new()
    }
}

/// Only true-colour formats with 8, 16 or 32 bits per pixel are used.
fn check_format(pf: &PixelFormat) -> Result<(), DecodeError> {
    if !pf.true_colour {
        return Err(invalid("colour map pixel formats are not supported"));
    }
    if !matches!(pf.bits_per_pixel, 8 | 16 | 32) {
        return Err(invalid(format!("{} bits per pixel is not supported", pf.bits_per_pixel)));
    }
    Ok(())
}

impl Decoders {
    pub fn new() -> Self {
        Decoders {
            conv: Converter::new(&PixelFormat::RGBX),
            zrle: zrle::Zrle::new(),
            tight: tight::Tight::new(),
            h264: h264::H264::new(),
            scratch: Scratch::default(),
        }
    }

    /// Whether `encoding` (a real encoding, not a pseudo one) can be decoded.
    pub fn supports(encoding: i32) -> bool {
        use encoding::*;
        matches!(encoding, RAW | COPY_RECT | RRE | HEXTILE | TIGHT | ZRLE | OPEN_H264)
    }

    /// Rebuild the pixel converter when the pixel format changes.
    fn converter(&mut self, pf: &PixelFormat) -> Result<(), DecodeError> {
        check_format(pf)?;
        if self.conv.format() != pf {
            self.conv = Converter::new(pf);
        }
        Ok(())
    }

    /// Decode the payload of one rectangle into `fb`, marking the changed area.
    pub async fn decode(
        &mut self,
        encoding: i32,
        rect: Rect,
        pf: &PixelFormat,
        r: &mut RfbReader,
        fb: &mut Framebuffer,
    ) -> Result<(), DecodeError> {
        match encoding {
            encoding::COPY_RECT => {
                let src_x = r.read_u16().await?;
                let src_y = r.read_u16().await?;
                fb.copy_rect(src_x, src_y, rect);
                Ok(())
            }
            encoding::OPEN_H264 => self.h264.decode(rect, r, fb, &mut self.scratch.input).await,
            encoding::RAW | encoding::RRE | encoding::HEXTILE | encoding::ZRLE | encoding::TIGHT => {
                self.converter(pf)?;
                let (conv, s) = (&self.conv, &mut self.scratch);
                match encoding {
                    encoding::RAW => raw::decode(rect, conv, r, fb, &mut s.input).await,
                    encoding::RRE => rre::decode(rect, conv, r, fb, &mut s.input).await,
                    encoding::HEXTILE => hextile::decode(rect, conv, r, fb, &mut s.input).await,
                    encoding::ZRLE => self.zrle.decode(rect, conv, r, fb, s).await,
                    _ => self.tight.decode(rect, conv, r, fb, s).await,
                }
            }
            other => Err(DecodeError::Unsupported(other)),
        }
    }

    /// Decode the payload of a Cursor pseudo-encoding rectangle. The rectangle
    /// position is the hotspot and its size is the cursor size.
    pub async fn decode_cursor(
        &mut self,
        rect: Rect,
        pf: &PixelFormat,
        r: &mut RfbReader,
    ) -> Result<CursorImage, DecodeError> {
        self.converter(pf)?;
        cursor::decode(rect, &self.conv, r, &mut self.scratch.input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoders_can_move_between_threads() {
        fn is_send<T: Send>(_: &T) {}
        let mut d = Decoders::new();
        is_send(&d);
        let mut r = RfbReader::from_bytes(Vec::new());
        let mut fb = Framebuffer::new(1, 1);
        let pf = PixelFormat::RGBX;
        let rect = Rect::new(0, 0, 1, 1);
        is_send(&d.decode(encoding::RAW, rect, &pf, &mut r, &mut fb));
    }

    #[tokio::test]
    async fn dispatches_by_encoding() {
        let mut d = Decoders::new();
        let mut fb = Framebuffer::new(4, 2);
        let pf = PixelFormat::RGBX;

        let mut r = RfbReader::from_bytes(vec![9, 8, 7, 0]);
        d.decode(encoding::RAW, Rect::new(0, 0, 1, 1), &pf, &mut r, &mut fb).await.unwrap();
        // CopyRect from (0, 0) to (3, 1).
        let mut r = RfbReader::from_bytes(vec![0, 0, 0, 0]);
        fb.take_dirty();
        d.decode(encoding::COPY_RECT, Rect::new(3, 1, 1, 1), &pf, &mut r, &mut fb).await.unwrap();
        assert_eq!(fb.read_rect(Rect::new(3, 1, 1, 1)), vec![9, 8, 7, 255]);
        assert_eq!(fb.take_dirty(), vec![Rect::new(3, 1, 1, 1)]);

        // The same decoder switches to another pixel format.
        let mut r = RfbReader::from_bytes(vec![0x80, 0x00, 0xf8]);
        d.decode(encoding::TIGHT, Rect::new(0, 0, 4, 2), &PixelFormat::RGB565, &mut r, &mut fb).await.unwrap();
        assert_eq!(fb.read_rect(Rect::new(2, 1, 1, 1)), vec![255, 0, 0, 255]);
        assert_eq!(fb.take_dirty(), vec![Rect::new(0, 0, 4, 2)]);

        let mut r = RfbReader::from_bytes(Vec::new());
        let res = d.decode(99, Rect::new(0, 0, 1, 1), &pf, &mut r, &mut fb).await;
        assert!(matches!(res, Err(DecodeError::Unsupported(99))));

        let colour_map = PixelFormat { true_colour: false, ..PixelFormat::BGR233 };
        let mut r = RfbReader::from_bytes(vec![0]);
        let res = d.decode(encoding::RAW, Rect::new(0, 0, 1, 1), &colour_map, &mut r, &mut fb).await;
        assert!(matches!(res, Err(DecodeError::Invalid(_))));
    }

    #[tokio::test]
    async fn cursor_through_public_api() {
        let mut d = Decoders::new();
        let mut r = RfbReader::from_bytes(vec![0b00_000_111, 0x80]);
        let c = d.decode_cursor(Rect::new(0, 0, 1, 1), &PixelFormat::BGR233, &mut r).await.unwrap();
        assert_eq!(c.rgba, vec![255, 0, 0, 255]);
    }

    #[tokio::test]
    async fn truncated_data_is_an_error_not_a_panic() {
        let mut d = Decoders::new();
        let mut fb = Framebuffer::new(8, 8);
        let pf = PixelFormat::RGBX;
        let rect = Rect::new(0, 0, 8, 8);
        for enc in
            [encoding::RAW, encoding::RRE, encoding::HEXTILE, encoding::ZRLE, encoding::TIGHT, encoding::OPEN_H264]
        {
            let mut r = RfbReader::from_bytes(vec![1, 0, 0, 0, 5, 1]);
            assert!(d.decode(enc, rect, &pf, &mut r, &mut fb).await.is_err(), "{enc}");
        }
        // A huge length is refused before anything is allocated.
        let mut r = RfbReader::from_bytes(vec![0xff, 0xff, 0xff, 0xff]);
        let res = d.decode(encoding::ZRLE, rect, &pf, &mut r, &mut fb).await;
        assert!(matches!(res, Err(DecodeError::Invalid(_))));
    }
}
