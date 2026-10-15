//! Everything needed to turn the bytes a VNC server sends into pixels:
//! the pixel format description, the local framebuffer, a buffered reader
//! for the network stream and the decoders for each picture encoding.

pub mod decode;
pub mod framebuffer;
pub mod pixel;
pub mod reader;

pub use decode::{CursorImage, DecodeError, Decoders, encoding};
pub use framebuffer::{Framebuffer, Rect};
pub use pixel::PixelFormat;
pub use reader::RfbReader;
