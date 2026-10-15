//! The local copy of the remote screen, stored as tightly packed RGBA bytes,
//! plus a record of which parts changed since the picture was last shown.

/// A rectangle in framebuffer coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Rect { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn right(&self) -> u32 {
        self.x as u32 + self.w as u32
    }

    pub fn bottom(&self) -> u32 {
        self.y as u32 + self.h as u32
    }

    pub fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        let r = self.right().max(o.right());
        let b = self.bottom().max(o.bottom());
        Rect::new(x, y, (r - x as u32) as u16, (b - y as u32) as u16)
    }

    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        if r <= x as u32 || b <= y as u32 {
            return None;
        }
        Some(Rect::new(x, y, (r - x as u32) as u16, (b - y as u32) as u16))
    }
}

/// When more changed rectangles than this pile up they are merged into one.
const MAX_DIRTY_RECTS: usize = 64;

/// RGBA framebuffer. Row stride is always `width * 4`.
pub struct Framebuffer {
    width: u16,
    height: u16,
    data: Vec<u8>,
    dirty: Vec<Rect>,
}

impl Framebuffer {
    pub fn new(width: u16, height: u16) -> Self {
        let mut data = vec![0u8; width as usize * height as usize * 4];
        for px in data.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Framebuffer { width, height, data, dirty: vec![Rect::new(0, 0, width, height)] }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Direct access to the pixels. Callers must call [`mark_dirty`](Self::mark_dirty)
    /// for every area they change.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Change the size. Existing content stays in the top-left corner and
    /// the new area is black. Everything is marked as changed.
    pub fn resize(&mut self, width: u16, height: u16) {
        if width == self.width && height == self.height {
            return;
        }
        let mut next = Framebuffer::new(width, height);
        let copy_w = width.min(self.width) as usize * 4;
        for y in 0..height.min(self.height) as usize {
            let src = y * self.stride();
            let dst = y * next.stride();
            next.data[dst..dst + copy_w].copy_from_slice(&self.data[src..src + copy_w]);
        }
        *self = next;
    }

    /// Clip a rectangle to the framebuffer. `None` when nothing is left.
    pub fn clip(&self, r: Rect) -> Option<Rect> {
        r.intersect(&self.bounds())
    }

    pub fn mark_dirty(&mut self, r: Rect) {
        let Some(r) = self.clip(r) else { return };
        if self.dirty.len() >= MAX_DIRTY_RECTS {
            let merged = self.dirty.iter().fold(r, |acc, d| acc.union(d));
            self.dirty.clear();
            self.dirty.push(merged);
        } else {
            self.dirty.push(r);
        }
    }

    /// Take the list of changed areas, leaving it empty.
    pub fn take_dirty(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.dirty)
    }

    pub fn has_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Fill a rectangle with one colour.
    pub fn fill_rect(&mut self, r: Rect, rgba: [u8; 4]) {
        let Some(c) = self.clip(r) else { return };
        let stride = self.stride();
        for y in c.y as usize..c.bottom() as usize {
            let start = y * stride + c.x as usize * 4;
            let row = &mut self.data[start..start + c.w as usize * 4];
            for px in row.as_chunks_mut::<4>().0 {
                px.copy_from_slice(&rgba);
            }
        }
        self.mark_dirty(c);
    }

    /// Write tightly packed RGBA pixels (`r.w * r.h * 4` bytes) into a rectangle.
    /// Parts outside the framebuffer are skipped.
    pub fn put_pixels(&mut self, r: Rect, rgba: &[u8]) {
        debug_assert!(rgba.len() >= r.area() as usize * 4);
        let Some(c) = self.clip(r) else { return };
        let stride = self.stride();
        let src_stride = r.w as usize * 4;
        let dx = (c.x - r.x) as usize * 4;
        let row_len = c.w as usize * 4;
        for row in 0..c.h as usize {
            let sy = (c.y - r.y) as usize + row;
            let src = sy * src_stride + dx;
            let dst = (c.y as usize + row) * stride + c.x as usize * 4;
            self.data[dst..dst + row_len].copy_from_slice(&rgba[src..src + row_len]);
        }
        self.mark_dirty(c);
    }

    /// Copy an area of the framebuffer to another place (CopyRect).
    /// Overlapping areas are handled correctly.
    pub fn copy_rect(&mut self, src_x: u16, src_y: u16, dst: Rect) {
        let Some(src) = self.clip(Rect::new(src_x, src_y, dst.w, dst.h)) else { return };
        let Some(d) = self.clip(Rect::new(dst.x, dst.y, src.w, src.h)) else { return };
        let stride = self.stride();
        let row_len = d.w as usize * 4;
        let rows: Box<dyn Iterator<Item = usize>> =
            if d.y > src.y { Box::new((0..d.h as usize).rev()) } else { Box::new(0..d.h as usize) };
        for row in rows {
            let s = (src.y as usize + row) * stride + src.x as usize * 4;
            let t = (d.y as usize + row) * stride + d.x as usize * 4;
            self.data.copy_within(s..s + row_len, t);
        }
        self.mark_dirty(d);
    }

    /// Mutable slice for `w` pixels of one row starting at (x, y), already
    /// clipped to the framebuffer width. Does not mark anything as changed.
    pub fn row_mut(&mut self, x: u16, y: u16, w: u16) -> &mut [u8] {
        if y >= self.height || x >= self.width {
            return &mut [];
        }
        let w = w.min(self.width - x) as usize;
        let start = y as usize * self.stride() + x as usize * 4;
        &mut self.data[start..start + w * 4]
    }

    /// Copy one rectangle out as tightly packed RGBA.
    pub fn read_rect(&self, r: Rect) -> Vec<u8> {
        let Some(c) = self.clip(r) else { return Vec::new() };
        let mut out = Vec::with_capacity(c.area() as usize * 4);
        for y in c.y as usize..c.bottom() as usize {
            let start = y * self.stride() + c.x as usize * 4;
            out.extend_from_slice(&self.data[start..start + c.w as usize * 4]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_rect_handles_overlap_downwards() {
        let mut fb = Framebuffer::new(4, 4);
        for y in 0..4u16 {
            fb.fill_rect(Rect::new(0, y, 4, 1), [y as u8, 0, 0, 255]);
        }
        fb.copy_rect(0, 0, Rect::new(0, 1, 4, 3));
        let rows: Vec<u8> = (0..4).map(|y| fb.read_rect(Rect::new(0, y, 1, 1))[0]).collect();
        assert_eq!(rows, vec![0, 0, 1, 2]);
    }

    #[test]
    fn put_pixels_clips() {
        let mut fb = Framebuffer::new(2, 2);
        let px = [9u8; 3 * 3 * 4];
        fb.put_pixels(Rect::new(1, 1, 3, 3), &px);
        assert_eq!(fb.read_rect(Rect::new(1, 1, 1, 1)), vec![9, 9, 9, 9]);
        assert_eq!(fb.read_rect(Rect::new(0, 0, 1, 1)), vec![0, 0, 0, 255]);
    }
}
