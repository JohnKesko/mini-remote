//! Open H.264 encoding: H.264 video for a fixed screen area. Each area
//! (x, y, width, height) has its own video decoder, called a context.

use openh264::decoder::Decoder;
use openh264::formats::YUVSource;

use super::DecodeError;
use super::convert::{invalid, read_into};
use crate::{Framebuffer, Rect, RfbReader};

const RESET_CONTEXT: u32 = 1;
const RESET_ALL_CONTEXTS: u32 = 2;

/// The specification asks clients to keep at least this many contexts.
const MAX_CONTEXTS: usize = 64;

/// Largest block of video data we accept in one rectangle.
const MAX_DATA_BYTES: usize = 64 << 20;

/// How the stream's Y, U and V values turn into RGB, as named in its
/// sequence parameter set (SPS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ColourSpace {
    /// Values use 0..=255 instead of 16..=235 (16..=240 for colour).
    full_range: bool,
    /// The H.264 `matrix_coefficients` number.
    matrix: u8,
}

impl ColourSpace {
    /// What to assume when the stream does not say: BT.601, limited range.
    const DEFAULT: ColourSpace = ColourSpace { full_range: false, matrix: 6 };
}

/// Lookup tables for YUV to RGB in 16.16 fixed point.
struct Tables {
    space: ColourSpace,
    y: [i32; 256],
    rv: [i32; 256],
    gu: [i32; 256],
    gv: [i32; 256],
    bu: [i32; 256],
}

impl Tables {
    fn new(space: ColourSpace) -> Box<Self> {
        let (kr, kb) = match space.matrix {
            1 => (0.2126, 0.0722),
            4 => (0.30, 0.11),
            7 => (0.212, 0.087),
            9 | 10 => (0.2627, 0.0593),
            _ => (0.299, 0.114),
        };
        let kg = 1.0 - kr - kb;
        let (y_scale, y_offset, c_scale) =
            if space.full_range { (1.0, 0.0, 1.0) } else { (255.0 / 219.0, 16.0, 255.0 / 224.0) };
        let one = 65536.0;
        let mut t = Box::new(Tables { space, y: [0; 256], rv: [0; 256], gu: [0; 256], gv: [0; 256], bu: [0; 256] });
        for i in 0..256 {
            let c = (i as f64 - 128.0) * c_scale * one;
            // Half a unit is added here so the final shift rounds.
            t.y[i] = ((i as f64 - y_offset) * y_scale * one + one / 2.0).round() as i32;
            t.rv[i] = (c * 2.0 * (1.0 - kr)).round() as i32;
            t.bu[i] = (c * 2.0 * (1.0 - kb)).round() as i32;
            t.gu[i] = (-c * 2.0 * kb * (1.0 - kb) / kg).round() as i32;
            t.gv[i] = (-c * 2.0 * kr * (1.0 - kr) / kg).round() as i32;
        }
        t
    }
}

#[inline]
fn clamp(v: i32) -> u8 {
    (v >> 16).clamp(0, 255) as u8
}

/// Convert one row of pixels. `u` and `v` hold one value per two pixels.
#[inline]
fn convert_row(t: &Tables, y: &[u8], u: &[u8], v: &[u8], dst: &mut [u8]) {
    let n = dst.len() / 4;
    let pairs = n / 2;
    let rows = dst[..pairs * 8].chunks_exact_mut(8).zip(y.chunks_exact(2)).zip(u).zip(v);
    for (((d, yy), &uu), &vv) in rows {
        let (r, g, b) = (t.rv[vv as usize], t.gu[uu as usize] + t.gv[vv as usize], t.bu[uu as usize]);
        let (y0, y1) = (t.y[yy[0] as usize], t.y[yy[1] as usize]);
        d.copy_from_slice(&[
            clamp(y0 + r),
            clamp(y0 + g),
            clamp(y0 + b),
            255,
            clamp(y1 + r),
            clamp(y1 + g),
            clamp(y1 + b),
            255,
        ]);
    }
    if n % 2 == 1 {
        let (Some(&yy), Some(&uu), Some(&vv)) = (y.get(n - 1), u.get(pairs), v.get(pairs)) else { return };
        let y0 = t.y[yy as usize];
        dst[(n - 1) * 4..n * 4].copy_from_slice(&[
            clamp(y0 + t.rv[vv as usize]),
            clamp(y0 + t.gu[uu as usize] + t.gv[vv as usize]),
            clamp(y0 + t.bu[uu as usize]),
            255,
        ]);
    }
}

/// Draw a decoded picture into `rect`, cropping it on the right and bottom
/// if it is larger.
fn draw(pic: &impl YUVSource, rect: Rect, fb: &mut Framebuffer, t: &Tables) {
    let (pw, ph) = pic.dimensions();
    let (ys, us, vs) = pic.strides();
    let (yp, up, vp) = (pic.y(), pic.u(), pic.v());
    let w = pw.min(rect.w as usize);
    for row in 0..ph.min(rect.h as usize) {
        let Ok(y) = u16::try_from(rect.y as usize + row) else { break };
        let dst = fb.row_mut(rect.x, y, w as u16);
        if dst.is_empty() {
            break;
        }
        let n = dst.len() / 4;
        let (c, cn) = (row / 2, n.div_ceil(2));
        let planes = (yp.get(row * ys..row * ys + n), up.get(c * us..c * us + cn), vp.get(c * vs..c * vs + cn));
        let (Some(yr), Some(ur), Some(vr)) = planes else { break };
        convert_row(t, yr, ur, vr, dst);
    }
}

struct Context {
    rect: Rect,
    decoder: Decoder,
    tables: Box<Tables>,
    last_used: u64,
}

/// One access unit (all the pieces of one picture) inside the data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Unit {
    start: usize,
    end: usize,
    picture: bool,
}

pub(crate) struct H264 {
    contexts: Vec<Context>,
    clock: u64,
    units: Vec<Unit>,
}

impl H264 {
    pub fn new() -> Self {
        H264 { contexts: Vec::new(), clock: 0, units: Vec::new() }
    }

    pub async fn decode(
        &mut self,
        rect: Rect,
        r: &mut RfbReader,
        fb: &mut Framebuffer,
        data: &mut Vec<u8>,
    ) -> Result<(), DecodeError> {
        let len = r.read_u32().await? as usize;
        let flags = r.read_u32().await?;
        if len > MAX_DATA_BYTES {
            return Err(invalid(format!("video rectangle of {len} bytes is too large")));
        }
        read_into(r, data, len).await?;

        // Flags apply before anything is decoded.
        if flags & RESET_ALL_CONTEXTS != 0 {
            self.contexts.clear();
        } else if flags & RESET_CONTEXT != 0 {
            self.contexts.retain(|c| c.rect != rect);
        }
        if len == 0 || rect.is_empty() {
            return Ok(());
        }

        let data = &data[..len];
        let space = split_units(data, &mut self.units);
        let i = self.context(rect)?;
        let ctx = &mut self.contexts[i];
        if let Some(space) = space.filter(|s| *s != ctx.tables.space) {
            ctx.tables = Tables::new(space);
        }

        // Decode every picture, but only draw the last one.
        let last = self.units.iter().rposition(|u| u.picture);
        let mut drawn = false;
        for (n, u) in self.units.iter().enumerate() {
            match ctx.decoder.decode(&data[u.start..u.end]) {
                Ok(Some(pic)) if Some(n) == last => {
                    draw(&pic, rect, fb, &ctx.tables);
                    drawn = true;
                }
                Ok(_) => {}
                // The specification suggests waiting for the next good picture.
                Err(e) => log::debug!("could not decode a video picture: {e}"),
            }
        }
        if drawn {
            fb.mark_dirty(rect);
        }
        Ok(())
    }

    /// Find or create the context for `rect`, dropping the one that has
    /// waited longest when there are too many.
    fn context(&mut self, rect: Rect) -> Result<usize, DecodeError> {
        self.clock += 1;
        if let Some(i) = self.contexts.iter().position(|c| c.rect == rect) {
            self.contexts[i].last_used = self.clock;
            return Ok(i);
        }
        if self.contexts.len() >= MAX_CONTEXTS {
            if let Some(oldest) = (0..self.contexts.len()).min_by_key(|&i| self.contexts[i].last_used) {
                self.contexts.swap_remove(oldest);
            }
        }
        let decoder = Decoder::new().map_err(|e| invalid(format!("could not start the video decoder: {e}")))?;
        let tables = Tables::new(ColourSpace::DEFAULT);
        self.contexts.push(Context { rect, decoder, tables, last_used: self.clock });
        Ok(self.contexts.len() - 1)
    }
}

/// Position of the next `00 00 01` start code at or after `from`.
fn find_start_code(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from + 2;
    while i < data.len() {
        match data[i] {
            0 => i += 1,
            1 if data[i - 1] == 0 && data[i - 2] == 0 => return Some(i - 2),
            // A start code cannot end here or in the next two bytes.
            _ => i += 3,
        }
    }
    None
}

/// Split an H.264 byte stream into access units. Returns the colour space
/// named by the last SPS in the data, if any.
fn split_units(data: &[u8], units: &mut Vec<Unit>) -> Option<ColourSpace> {
    units.clear();
    let mut space = None;
    let (mut start, mut picture) = (0, false);
    let mut next = find_start_code(data, 0);
    while let Some(code) = next {
        let nal_start = code + 3;
        next = find_start_code(data, nal_start);
        let nal = &data[nal_start..next.unwrap_or(data.len())];
        let Some(&header) = nal.first() else { continue };
        let kind = header & 0x1f;
        let slice = kind == 1 || kind == 5;
        // A new picture starts with a slice whose first macroblock is 0 (the
        // first bit after the header is 1), or with these non-picture units.
        let begins = if slice { nal.get(1).is_some_and(|b| b & 0x80 != 0) } else { matches!(kind, 6..=9 | 14..=18) };
        if begins && picture {
            let end = if code > start && data[code - 1] == 0 { code - 1 } else { code };
            units.push(Unit { start, end, picture });
            (start, picture) = (end, false);
        }
        picture |= slice;
        if kind == 7 {
            space = sps_colour_space(nal).or(space);
        }
    }
    if start < data.len() {
        units.push(Unit { start, end: data.len(), picture });
    }
    space
}

/// Reads bits, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        self.pos += 1;
        Some(((byte >> (7 - (self.pos - 1) % 8)) & 1) as u32)
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }

    /// Unsigned Exp-Golomb number.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        u32::try_from((1u64 << zeros) - 1 + self.bits(zeros)? as u64).ok()
    }

    /// Signed Exp-Golomb number.
    fn se(&mut self) -> Option<i64> {
        let k = self.ue()? as i64;
        Some(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }
}

fn skip_scaling_list(r: &mut Bits, size: usize) -> Option<()> {
    let (mut last, mut next) = (8i64, 8i64);
    for _ in 0..size {
        if next != 0 {
            next = (last + r.se()?).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Some(())
}

/// Read the colour description from an SPS unit (header byte included).
fn sps_colour_space(nal: &[u8]) -> Option<ColourSpace> {
    // Remove the emulation prevention bytes (00 00 03 -> 00 00).
    let mut rbsp = [0u8; 512];
    let (mut n, mut zeros) = (0, 0);
    for &b in nal.iter().skip(1) {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        if n == rbsp.len() {
            break;
        }
        rbsp[n] = b;
        n += 1;
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    let r = &mut Bits { data: &rbsp[..n], pos: 0 };
    let profile = r.bits(8)?;
    r.bits(16)?; // constraint flags and level
    r.ue()?; // parameter set id
    if matches!(profile, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        let chroma_format = r.ue()?;
        if chroma_format == 3 {
            r.bit()?;
        }
        r.ue()?; // luma bit depth
        r.ue()?; // chroma bit depth
        r.bit()?; // lossless bypass
        if r.bit()? == 1 {
            let lists = if chroma_format == 3 { 12 } else { 8 };
            for i in 0..lists {
                if r.bit()? == 1 {
                    skip_scaling_list(r, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    r.ue()?; // frame number size
    match r.ue()? {
        0 => {
            r.ue()?;
        }
        1 => {
            r.bit()?;
            r.se()?;
            r.se()?;
            for _ in 0..r.ue()?.min(255) {
                r.se()?;
            }
        }
        _ => {}
    }
    r.ue()?; // reference frames
    r.bit()?; // gaps allowed
    r.ue()?; // width in macroblocks
    r.ue()?; // height in map units
    if r.bit()? == 0 {
        r.bit()?; // field coding
    }
    r.bit()?; // direct 8x8 inference
    if r.bit()? == 1 {
        for _ in 0..4 {
            r.ue()?; // cropping
        }
    }
    if r.bit()? == 0 {
        return Some(ColourSpace::DEFAULT); // no VUI
    }
    if r.bit()? == 1 && r.bits(8)? == 255 {
        r.bits(32)?; // sample aspect ratio
    }
    if r.bit()? == 1 {
        r.bit()?; // overscan
    }
    if r.bit()? == 0 {
        return Some(ColourSpace::DEFAULT); // no video signal type
    }
    r.bits(3)?; // video format
    let full_range = r.bit()? == 1;
    let matrix = if r.bit()? == 1 {
        r.bits(16)?; // primaries and transfer
        r.bits(8)? as u8
    } else {
        2
    };
    Some(ColourSpace { full_range, matrix })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openh264::OpenH264API;
    use openh264::encoder::{Encoder, EncoderConfig, RateControlMode, VuiConfig};
    use openh264::formats::YUVBuffer;

    const W: usize = 64;
    const H: usize = 48;

    fn solid(y: u8, u: u8, v: u8) -> YUVBuffer {
        let mut data = vec![y; W * H];
        data.extend(std::iter::repeat_n(u, W * H / 4));
        data.extend(std::iter::repeat_n(v, W * H / 4));
        YUVBuffer::from_vec(data, W, H)
    }

    fn encoder(vui: Option<VuiConfig>) -> Encoder {
        let mut config = EncoderConfig::new().skip_frames(false).rate_control_mode(RateControlMode::Off);
        if let Some(vui) = vui {
            config = config.vui(vui);
        }
        Encoder::with_api_config(OpenH264API::from_source(), config).unwrap()
    }

    /// BT.601 limited-range red, green and blue.
    const RED: (u8, u8, u8) = (81, 90, 240);
    const GREEN: (u8, u8, u8) = (145, 54, 34);
    const BLUE: (u8, u8, u8) = (41, 240, 110);

    fn frame(enc: &mut Encoder, c: (u8, u8, u8)) -> Vec<u8> {
        enc.encode(&solid(c.0, c.1, c.2)).unwrap().to_vec()
    }

    fn wire(flags: u32, data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&flags.to_be_bytes());
        out.extend_from_slice(data);
        out
    }

    async fn run(h: &mut H264, rect: Rect, fb: &mut Framebuffer, bytes: Vec<u8>) {
        let mut r = RfbReader::from_bytes(bytes);
        h.decode(rect, &mut r, fb, &mut Vec::new()).await.unwrap();
        assert_eq!(r.buffered(), 0);
    }

    fn assert_near(fb: &Framebuffer, x: u16, y: u16, want: [u8; 3], tolerance: i32) {
        let p = fb.read_rect(Rect::new(x, y, 1, 1));
        let ok = (0..3).all(|i| (p[i] as i32 - want[i] as i32).abs() <= tolerance) && p[3] == 255;
        assert!(ok, "pixel ({x}, {y}) is {p:?}, wanted about {want:?}");
    }

    #[tokio::test]
    async fn h264_round_trip_two_frames() {
        let mut enc = encoder(None);
        let (f1, f2) = (frame(&mut enc, RED), frame(&mut enc, BLUE));
        let mut h = H264::new();
        let mut fb = Framebuffer::new(80, 60);
        let rect = Rect::new(8, 4, W as u16, H as u16);
        run(&mut h, rect, &mut fb, wire(0, &f1)).await;
        assert_near(&fb, 20, 20, [255, 0, 0], 12);
        assert_near(&fb, 8 + 63, 4 + 47, [255, 0, 0], 12);
        assert_near(&fb, 7, 4, [0, 0, 0], 0);
        run(&mut h, rect, &mut fb, wire(0, &f2)).await;
        assert_near(&fb, 30, 30, [0, 0, 255], 12);
        assert_eq!(h.contexts.len(), 1);
    }

    #[tokio::test]
    async fn h264_several_frames_in_one_message_and_cropping() {
        let mut enc = encoder(None);
        let mut data = frame(&mut enc, RED);
        data.extend_from_slice(&frame(&mut enc, GREEN));
        data.extend_from_slice(&frame(&mut enc, BLUE));
        let mut units = Vec::new();
        split_units(&data, &mut units);
        assert_eq!(units.iter().filter(|u| u.picture).count(), 3);

        let mut h = H264::new();
        let mut fb = Framebuffer::new(80, 60);
        // The rectangle is smaller than the picture: crop right and bottom.
        let rect = Rect::new(0, 0, 60, 40);
        run(&mut h, rect, &mut fb, wire(0, &data)).await;
        assert_near(&fb, 59, 39, [0, 0, 255], 12);
        assert_near(&fb, 60, 10, [0, 0, 0], 0);
        assert_near(&fb, 10, 40, [0, 0, 0], 0);

        // Green as a second message continues the same context.
        run(&mut h, rect, &mut fb, wire(0, &frame(&mut enc, GREEN))).await;
        assert_near(&fb, 5, 5, [0, 255, 0], 12);
    }

    #[tokio::test]
    async fn h264_flags_reset_contexts() {
        let mut enc = encoder(None);
        let first = frame(&mut enc, RED);
        let second = frame(&mut enc, GREEN);
        let mut h = H264::new();
        let mut fb = Framebuffer::new(64, 96);
        let a = Rect::new(0, 0, W as u16, H as u16);
        let b = Rect::new(0, 48, W as u16, H as u16);
        run(&mut h, a, &mut fb, wire(0, &first)).await;
        run(&mut h, b, &mut fb, wire(0, &first)).await;
        assert_eq!(h.contexts.len(), 2);
        // A control message with no data resets one context.
        run(&mut h, a, &mut fb, wire(RESET_CONTEXT, &[])).await;
        assert_eq!(h.contexts.len(), 1);
        // A picture that depends on an earlier one cannot be decoded in a
        // fresh context; it is skipped without an error.
        run(&mut h, a, &mut fb, wire(0, &second)).await;
        assert_near(&fb, 5, 5, [255, 0, 0], 12);
        run(&mut h, b, &mut fb, wire(RESET_ALL_CONTEXTS, &[])).await;
        assert!(h.contexts.is_empty());
    }

    #[tokio::test]
    async fn h264_full_range_bt709_from_sps() {
        // Colour (40, 200, 120) in BT.709 full range.
        let (r, g, b) = (40.0f64, 200.0, 120.0);
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let u = (b - y) / 1.8556 + 128.0;
        let v = (r - y) / 1.5748 + 128.0;
        let mut enc = encoder(Some(VuiConfig::bt709_full()));
        let data = enc.encode(&solid(y.round() as u8, u.round() as u8, v.round() as u8)).unwrap().to_vec();
        let mut units = Vec::new();
        assert_eq!(split_units(&data, &mut units), Some(ColourSpace { full_range: true, matrix: 1 }));

        let mut h = H264::new();
        let mut fb = Framebuffer::new(W as u16, H as u16);
        run(&mut h, Rect::new(0, 0, W as u16, H as u16), &mut fb, wire(0, &data)).await;
        assert_near(&fb, 10, 10, [40, 200, 120], 6);
    }

    #[test]
    fn default_stream_has_no_colour_description() {
        let mut enc = encoder(None);
        let data = frame(&mut enc, RED);
        let mut units = Vec::new();
        let space = split_units(&data, &mut units);
        assert!(space.is_none() || space == Some(ColourSpace::DEFAULT), "{space:?}");
    }

    #[test]
    fn convert_odd_width_row() {
        let t = Tables::new(ColourSpace::DEFAULT);
        let mut dst = [0u8; 12];
        convert_row(&t, &[235, 16, 235], &[128, 128], &[128, 128], &mut dst);
        assert_eq!(dst, [255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn start_codes() {
        let data = [0, 0, 0, 1, 0x67, 5, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x65, 0x88];
        assert_eq!(find_start_code(&data, 0), Some(1));
        assert_eq!(find_start_code(&data, 4), Some(6));
        assert_eq!(find_start_code(&data, 9), Some(11));
        assert_eq!(find_start_code(&data, 14), None);
    }
}
