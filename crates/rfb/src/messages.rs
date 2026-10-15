//! Builders for client-to-server messages. Each returns the complete wire bytes.

use codec::PixelFormat;

pub const SET_PIXEL_FORMAT: u8 = 0;
pub const SET_ENCODINGS: u8 = 2;
pub const FRAMEBUFFER_UPDATE_REQUEST: u8 = 3;
pub const KEY_EVENT: u8 = 4;
pub const POINTER_EVENT: u8 = 5;
pub const CLIENT_CUT_TEXT: u8 = 6;
pub const ENABLE_CONTINUOUS_UPDATES: u8 = 150;
pub const CLIENT_FENCE: u8 = 248;
pub const SET_DESKTOP_SIZE: u8 = 251;
pub const QEMU_CLIENT_MESSAGE: u8 = 255;

/// Extended clipboard flag bits.
pub mod clip {
    pub const TEXT: u32 = 1 << 0;
    pub const CAPS: u32 = 1 << 24;
    pub const REQUEST: u32 = 1 << 25;
    pub const PEEK: u32 = 1 << 26;
    pub const NOTIFY: u32 = 1 << 27;
    pub const PROVIDE: u32 = 1 << 28;
    pub const FORMAT_MASK: u32 = 0xffff;
}

/// Fence flag bits.
pub mod fence {
    pub const BLOCK_BEFORE: u32 = 1 << 0;
    pub const BLOCK_AFTER: u32 = 1 << 1;
    pub const SYNC_NEXT: u32 = 1 << 2;
    pub const REQUEST: u32 = 1 << 31;
    pub const SUPPORTED: u32 = BLOCK_BEFORE | BLOCK_AFTER | SYNC_NEXT | REQUEST;
}

pub fn set_pixel_format(pf: &PixelFormat) -> Vec<u8> {
    let mut m = vec![SET_PIXEL_FORMAT, 0, 0, 0];
    m.extend_from_slice(&pf.to_bytes());
    m
}

pub fn set_encodings(encodings: &[i32]) -> Vec<u8> {
    let mut m = vec![SET_ENCODINGS, 0];
    m.extend_from_slice(&(encodings.len() as u16).to_be_bytes());
    for e in encodings {
        m.extend_from_slice(&e.to_be_bytes());
    }
    m
}

pub fn update_request(incremental: bool, x: u16, y: u16, w: u16, h: u16) -> Vec<u8> {
    let mut m = vec![FRAMEBUFFER_UPDATE_REQUEST, incremental as u8];
    for v in [x, y, w, h] {
        m.extend_from_slice(&v.to_be_bytes());
    }
    m
}

pub fn key_event(down: bool, keysym: u32) -> Vec<u8> {
    let mut m = vec![KEY_EVENT, down as u8, 0, 0];
    m.extend_from_slice(&keysym.to_be_bytes());
    m
}

/// QEMU extended key event: keysym plus the physical key as an XT scancode.
pub fn qemu_key_event(down: bool, keysym: u32, scancode: u32) -> Vec<u8> {
    let mut m = vec![QEMU_CLIENT_MESSAGE, 0];
    m.extend_from_slice(&(down as u16).to_be_bytes());
    m.extend_from_slice(&keysym.to_be_bytes());
    m.extend_from_slice(&scancode.to_be_bytes());
    m
}

/// Pointer event. `buttons` uses [`crate::buttons`]. With `extended` the
/// back/forward buttons go in the extra byte (ExtendedMouseButtons).
pub fn pointer_event(x: u16, y: u16, buttons: u16, extended: bool) -> Vec<u8> {
    use crate::buttons::{BACK, FORWARD};
    let low = (buttons & 0x7f) as u8;
    if extended {
        let extra = ((buttons & BACK != 0) as u8) | (((buttons & FORWARD != 0) as u8) << 1);
        let mut m = vec![POINTER_EVENT, low | 0x80];
        m.extend_from_slice(&x.to_be_bytes());
        m.extend_from_slice(&y.to_be_bytes());
        m.push(extra);
        m
    } else {
        let mask = low | if buttons & BACK != 0 { 0x80 } else { 0 };
        let mut m = vec![POINTER_EVENT, mask];
        m.extend_from_slice(&x.to_be_bytes());
        m.extend_from_slice(&y.to_be_bytes());
        m
    }
}

/// Classic clipboard message. Text is converted to Latin-1; characters
/// outside it become '?'.
pub fn client_cut_text_latin1(text: &str) -> Vec<u8> {
    let bytes: Vec<u8> = text
        .replace("\r\n", "\n")
        .chars()
        .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
        .collect();
    let mut m = vec![CLIENT_CUT_TEXT, 0, 0, 0];
    m.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    m.extend_from_slice(&bytes);
    m
}

/// Extended clipboard message with a negative length.
pub fn client_cut_text_extended(flags: u32, payload: &[u8]) -> Vec<u8> {
    let len = -((4 + payload.len()) as i32);
    let mut m = vec![CLIENT_CUT_TEXT, 0, 0, 0];
    m.extend_from_slice(&len.to_be_bytes());
    m.extend_from_slice(&flags.to_be_bytes());
    m.extend_from_slice(payload);
    m
}

/// Our extended clipboard capabilities: plain text only, everything via notify.
pub fn clipboard_caps() -> Vec<u8> {
    let flags = clip::CAPS | clip::TEXT | clip::REQUEST | clip::PEEK | clip::NOTIFY | clip::PROVIDE;
    // One size entry for the one format (text). Zero forces notify-then-request.
    client_cut_text_extended(flags, &0u32.to_be_bytes())
}

pub fn clipboard_notify_text() -> Vec<u8> {
    client_cut_text_extended(clip::NOTIFY | clip::TEXT, &[])
}

pub fn clipboard_request_text() -> Vec<u8> {
    client_cut_text_extended(clip::REQUEST | clip::TEXT, &[])
}

/// Provide clipboard text: zlib stream of (u32 size, UTF-8 with CRLF and a trailing NUL).
pub fn clipboard_provide_text(text: &str) -> Vec<u8> {
    use std::io::Write;
    let mut data = text.replace("\r\n", "\n").replace('\n', "\r\n").into_bytes();
    data.push(0);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = z.write_all(&(data.len() as u32).to_be_bytes());
    let _ = z.write_all(&data);
    let compressed = z.finish().unwrap_or_default();
    client_cut_text_extended(clip::PROVIDE | clip::TEXT, &compressed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screen {
    pub id: u32,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub flags: u32,
}

pub fn set_desktop_size(width: u16, height: u16, screens: &[Screen]) -> Vec<u8> {
    let mut m = vec![SET_DESKTOP_SIZE, 0];
    m.extend_from_slice(&width.to_be_bytes());
    m.extend_from_slice(&height.to_be_bytes());
    m.push(screens.len() as u8);
    m.push(0);
    for s in screens {
        m.extend_from_slice(&s.id.to_be_bytes());
        for v in [s.x, s.y, s.w, s.h] {
            m.extend_from_slice(&v.to_be_bytes());
        }
        m.extend_from_slice(&s.flags.to_be_bytes());
    }
    m
}

pub fn client_fence(flags: u32, payload: &[u8]) -> Vec<u8> {
    let payload = &payload[..payload.len().min(64)];
    let mut m = vec![CLIENT_FENCE, 0, 0, 0];
    m.extend_from_slice(&flags.to_be_bytes());
    m.push(payload.len() as u8);
    m.extend_from_slice(payload);
    m
}

pub fn enable_continuous_updates(enable: bool, x: u16, y: u16, w: u16, h: u16) -> Vec<u8> {
    let mut m = vec![ENABLE_CONTINUOUS_UPDATES, enable as u8];
    for v in [x, y, w, h] {
        m.extend_from_slice(&v.to_be_bytes());
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_back_button_uses_bit_7_unless_extended() {
        use crate::buttons::*;
        assert_eq!(pointer_event(1, 2, LEFT | BACK, false), vec![5, 0x81, 0, 1, 0, 2]);
        assert_eq!(pointer_event(1, 2, LEFT | FORWARD, true), vec![5, 0x81, 0, 1, 0, 2, 2]);
    }

    #[test]
    fn clipboard_provide_round_trips() {
        use std::io::Read;
        let m = clipboard_provide_text("a\nb");
        let len = i32::from_be_bytes([m[4], m[5], m[6], m[7]]);
        assert_eq!(-len as usize, m.len() - 8);
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&m[12..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, b"\0\0\0\x05a\r\nb\0");
    }
}
