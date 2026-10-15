//! Description of how the server packs colours into bytes, and conversion
//! from that packing into the RGBA layout of our framebuffer.

/// The RFB `PIXEL_FORMAT` structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    pub bits_per_pixel: u8,
    pub depth: u8,
    pub big_endian: bool,
    pub true_colour: bool,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

impl PixelFormat {
    /// 32 bits per pixel with the bytes in memory ordered R, G, B, unused.
    /// This matches the framebuffer layout, so pixels can be copied directly
    /// (only the alpha byte has to be set to 255).
    pub const RGBX: PixelFormat = PixelFormat {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        true_colour: true,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 0,
        green_shift: 8,
        blue_shift: 16,
    };

    /// 16 bits per pixel, 5 bits red, 6 bits green, 5 bits blue.
    pub const RGB565: PixelFormat = PixelFormat {
        bits_per_pixel: 16,
        depth: 16,
        big_endian: false,
        true_colour: true,
        red_max: 31,
        green_max: 63,
        blue_max: 31,
        red_shift: 11,
        green_shift: 5,
        blue_shift: 0,
    };

    /// 8 bits per pixel, 3 bits red, 3 bits green, 2 bits blue (256 colours).
    pub const BGR233: PixelFormat = PixelFormat {
        bits_per_pixel: 8,
        depth: 8,
        big_endian: false,
        true_colour: true,
        red_max: 7,
        green_max: 7,
        blue_max: 3,
        red_shift: 0,
        green_shift: 3,
        blue_shift: 6,
    };

    pub fn bytes_per_pixel(&self) -> usize {
        (self.bits_per_pixel as usize / 8).max(1)
    }

    /// True when pixels arrive in exactly the framebuffer's byte order.
    pub fn is_rgbx(&self) -> bool {
        self.bits_per_pixel == 32
            && self.true_colour
            && self.red_max == 255
            && self.green_max == 255
            && self.blue_max == 255
            && ((!self.big_endian && self.red_shift == 0 && self.green_shift == 8 && self.blue_shift == 16)
                || (self.big_endian && self.red_shift == 24 && self.green_shift == 16 && self.blue_shift == 8))
    }

    /// Wire form (16 bytes, including 3 bytes of padding).
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0] = self.bits_per_pixel;
        b[1] = self.depth;
        b[2] = self.big_endian as u8;
        b[3] = self.true_colour as u8;
        b[4..6].copy_from_slice(&self.red_max.to_be_bytes());
        b[6..8].copy_from_slice(&self.green_max.to_be_bytes());
        b[8..10].copy_from_slice(&self.blue_max.to_be_bytes());
        b[10] = self.red_shift;
        b[11] = self.green_shift;
        b[12] = self.blue_shift;
        b
    }

    pub fn from_bytes(b: &[u8; 16]) -> Self {
        PixelFormat {
            bits_per_pixel: b[0],
            depth: b[1],
            big_endian: b[2] != 0,
            true_colour: b[3] != 0,
            red_max: u16::from_be_bytes([b[4], b[5]]),
            green_max: u16::from_be_bytes([b[6], b[7]]),
            blue_max: u16::from_be_bytes([b[8], b[9]]),
            red_shift: b[10],
            green_shift: b[11],
            blue_shift: b[12],
        }
    }

    /// Read one `PIXEL` (bytes_per_pixel bytes) as a packed integer.
    #[inline]
    pub fn raw_value(&self, bytes: &[u8]) -> u32 {
        match self.bytes_per_pixel() {
            1 => bytes[0] as u32,
            2 => {
                let v = [bytes[0], bytes[1]];
                if self.big_endian { u16::from_be_bytes(v) as u32 } else { u16::from_le_bytes(v) as u32 }
            }
            _ => {
                let v = [bytes[0], bytes[1], bytes[2], bytes[3]];
                if self.big_endian { u32::from_be_bytes(v) } else { u32::from_le_bytes(v) }
            }
        }
    }

    /// Convert a packed pixel value into RGBA with full opacity.
    #[inline]
    pub fn value_to_rgba(&self, v: u32) -> [u8; 4] {
        #[inline]
        fn channel(v: u32, shift: u8, max: u16) -> u8 {
            if max == 0 {
                return 0;
            }
            let c = (v >> shift) & max as u32;
            if max == 255 { c as u8 } else { ((c * 255 + max as u32 / 2) / max as u32) as u8 }
        }
        [
            channel(v, self.red_shift, self.red_max),
            channel(v, self.green_shift, self.green_max),
            channel(v, self.blue_shift, self.blue_max),
            255,
        ]
    }

    /// Read one `PIXEL` and convert it into RGBA.
    #[inline]
    pub fn read_rgba(&self, bytes: &[u8]) -> [u8; 4] {
        if self.is_rgbx() && !self.big_endian {
            return [bytes[0], bytes[1], bytes[2], 255];
        }
        self.value_to_rgba(self.raw_value(bytes))
    }

    /// For ZRLE: when a `CPIXEL` is only 3 bytes, how far those bytes are
    /// shifted up inside the 32-bit pixel value (0 when the colours live in
    /// the low three bytes, 8 when they live in the high three bytes).
    /// `None` means a `CPIXEL` is a whole `PIXEL`.
    pub fn cpixel_shift(&self) -> Option<u32> {
        if self.bits_per_pixel != 32 || !self.true_colour || self.depth > 24 {
            return None;
        }
        let bits = |max: u16, shift: u8| (max as u128).checked_shl(shift as u32).unwrap_or(u128::MAX);
        let used = bits(self.red_max, self.red_shift)
            | bits(self.green_max, self.green_shift)
            | bits(self.blue_max, self.blue_shift);
        // When both would work the low three bytes are used by convention.
        if used < 1 << 24 {
            Some(0)
        } else if used & 0xff == 0 && used < 1 << 32 {
            Some(8)
        } else {
            None
        }
    }

    /// Size of a ZRLE `CPIXEL` in bytes.
    pub fn bytes_per_cpixel(&self) -> usize {
        if self.cpixel_shift().is_some() { 3 } else { self.bytes_per_pixel() }
    }

    /// True when a Tight `TPIXEL` is just 3 bytes: red, green, blue.
    pub fn is_tight_rgb(&self) -> bool {
        self.true_colour
            && self.bits_per_pixel == 32
            && self.depth == 24
            && self.red_max == 255
            && self.green_max == 255
            && self.blue_max == 255
    }

    /// Size of a Tight `TPIXEL` in bytes.
    pub fn bytes_per_tpixel(&self) -> usize {
        if self.is_tight_rgb() { 3 } else { self.bytes_per_pixel() }
    }
}
