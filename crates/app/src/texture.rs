//! The remote picture on the graphics card: one texture per session, kept
//! up to date by uploading only the parts of the screen that changed.

use egui_wgpu::wgpu;
use parking_lot::Mutex;
use rfb::{Rect, SharedFrame};

/// More changed areas than this are uploaded as one bounding box.
const MAX_SEPARATE_UPLOADS: usize = 128;

pub struct RemoteTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    pub id: egui::TextureId,
    pub width: u32,
    pub height: u32,
    filter: wgpu::FilterMode,
    /// Frame generation that was last uploaded in full; `None` forces a full upload.
    generation: Option<u64>,
}

impl RemoteTexture {
    /// Bring the texture in `slot` up to date with `frame`. Returns true when
    /// the picture changed. An empty frame keeps the old picture (e.g. while
    /// reconnecting).
    pub fn sync(slot: &mut Option<RemoteTexture>, gpu: &egui_wgpu::RenderState, frame: &Mutex<SharedFrame>) -> bool {
        let mut f = frame.lock();
        let (w, h) = (u32::from(f.width), u32::from(f.height));
        if w == 0 || h == 0 || f.pixels.len() < (w * h * 4) as usize {
            f.dirty.clear();
            return false;
        }

        let resized = slot.as_ref().is_none_or(|t| t.width != w || t.height != h);
        if resized {
            log::info!("remote picture texture is now {w}x{h}");
            let filter = slot.as_ref().map_or(wgpu::FilterMode::Linear, |t| t.filter);
            let (texture, view) = create(gpu, w, h);
            match slot {
                Some(t) => {
                    gpu.renderer.write().update_egui_texture_from_wgpu_texture(&gpu.device, &view, filter, t.id);
                    t.texture.destroy();
                    t.texture = texture;
                    t.view = view;
                    t.width = w;
                    t.height = h;
                    t.generation = None;
                }
                None => {
                    let id = gpu.renderer.write().register_native_texture(&gpu.device, &view, filter);
                    *slot = Some(RemoteTexture { texture, view, id, width: w, height: h, filter, generation: None });
                }
            }
        }
        let Some(tex) = slot.as_mut() else { return false };

        let full = tex.generation != Some(f.generation);
        if full {
            tex.upload(gpu, &f.pixels, Rect::new(0, 0, f.width, f.height));
            tex.generation = Some(f.generation);
            f.dirty.clear();
            return true;
        }
        if f.dirty.is_empty() {
            return false;
        }
        let dirty = std::mem::take(&mut f.dirty);
        if dirty.len() > MAX_SEPARATE_UPLOADS {
            tex.upload(gpu, &f.pixels, bounding_box(&dirty));
        } else {
            for rect in &dirty {
                tex.upload(gpu, &f.pixels, *rect);
            }
        }
        // Give the (now empty) list back so its memory is reused.
        f.dirty = dirty;
        f.dirty.clear();
        true
    }

    /// Next upload covers the whole picture (used after reconnecting).
    pub fn invalidate(&mut self) {
        self.generation = None;
    }

    /// Sharp pixels at 100%, smooth when scaled.
    pub fn set_filter(&mut self, gpu: &egui_wgpu::RenderState, smooth: bool) {
        let filter = if smooth { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest };
        if filter != self.filter {
            self.filter = filter;
            gpu.renderer.write().update_egui_texture_from_wgpu_texture(&gpu.device, &self.view, filter, self.id);
        }
    }

    pub fn free(self, gpu: &egui_wgpu::RenderState) {
        gpu.renderer.write().free_texture(&self.id);
        self.texture.destroy();
    }

    fn upload(&self, gpu: &egui_wgpu::RenderState, pixels: &[u8], rect: Rect) {
        let Some(rect) = clamp(rect, self.width, self.height) else { return };
        let offset = (u64::from(rect.y) * u64::from(self.width) + u64::from(rect.x)) * 4;
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: u32::from(rect.x), y: u32::from(rect.y), z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(u32::from(rect.h)),
            },
            wgpu::Extent3d { width: u32::from(rect.w), height: u32::from(rect.h), depth_or_array_layers: 1 },
        );
    }
}

fn create(gpu: &egui_wgpu::RenderState, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("remote screen"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // egui's renderer treats texture colours as sRGB-encoded values and
        // needs this plain format (an *Srgb format would be decoded twice and
        // look too dark).
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Keep a rectangle inside the picture; `None` when nothing is left.
fn clamp(r: Rect, width: u32, height: u32) -> Option<Rect> {
    let x0 = u32::from(r.x).min(width);
    let y0 = u32::from(r.y).min(height);
    let x1 = (u32::from(r.x) + u32::from(r.w)).min(width);
    let y1 = (u32::from(r.y) + u32::from(r.h)).min(height);
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0 as u16, y0 as u16, (x1 - x0) as u16, (y1 - y0) as u16))
}

fn bounding_box(rects: &[Rect]) -> Rect {
    let x0 = rects.iter().map(|r| u32::from(r.x)).min().unwrap_or(0);
    let y0 = rects.iter().map(|r| u32::from(r.y)).min().unwrap_or(0);
    let x1 = rects.iter().map(|r| u32::from(r.x) + u32::from(r.w)).max().unwrap_or(0);
    let y1 = rects.iter().map(|r| u32::from(r.y) + u32::from(r.h)).max().unwrap_or(0);
    let clamp16 = |v: u32| v.min(u32::from(u16::MAX)) as u16;
    Rect::new(clamp16(x0), clamp16(y0), clamp16(x1.saturating_sub(x0)), clamp16(y1.saturating_sub(y0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_are_clamped() {
        assert_eq!(clamp(Rect::new(10, 10, 20, 20), 25, 100), Some(Rect::new(10, 10, 15, 20)));
        assert_eq!(clamp(Rect::new(30, 0, 5, 5), 25, 100), None);
        assert_eq!(clamp(Rect::new(0, 0, 0, 5), 25, 100), None);
    }

    #[test]
    fn bounding_box_covers_all() {
        let b = bounding_box(&[Rect::new(5, 5, 10, 10), Rect::new(50, 1, 2, 2)]);
        assert_eq!(b, Rect::new(5, 1, 47, 14));
    }
}
