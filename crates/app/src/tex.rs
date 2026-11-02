//! Texture cache for breadboard part artwork.
//!
//! A part's `breadboard.svg` is rasterised with resvg (no text, no raster images: the artwork is
//! simple vector shapes) into an egui texture. Textures are cached per (type, zoom bucket): the
//! bucket is the on-screen pixel length of the artwork's long side rounded up to a power of two
//! between [`MIN_BUCKET`] and [`MAX_BUCKET`], so zooming re-renders only when the bucket changes
//! and the texture is at most about twice the displayed size. The cache is cleared whenever the
//! part library is reloaded.

use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions};
use short_circ_core::ViewDef;
use std::collections::HashMap;

pub const MIN_BUCKET: u32 = 64;
pub const MAX_BUCKET: u32 = 4096;
/// Buckets kept per type (zooming back and forth over a boundary must not re-render each time).
const KEEP_PER_TYPE: usize = 3;

/// Zoom bucket for artwork whose long side is `px` pixels on screen.
pub fn bucket(px: f32) -> u32 {
    let px = if px.is_finite() { px.ceil().max(1.0) } else { 1.0 };
    (px.min(MAX_BUCKET as f32) as u32).next_power_of_two().clamp(MIN_BUCKET, MAX_BUCKET)
}

/// Pixel size `(w, h)` of the raster for artwork of `size` grid units whose long side gets
/// `long_px` pixels.
pub fn raster_size(size: [f64; 2], long_px: u32) -> (u32, u32) {
    let long = size[0].max(size[1]).max(1e-6);
    let s = long_px as f64 / long;
    (((size[0] * s).round() as u32).clamp(1, long_px), ((size[1] * s).round() as u32).clamp(1, long_px))
}

/// A rasterised image: straight RGBA would lose quality in blending, egui wants premultiplied
/// alpha, which is what the rasteriser produces.
pub struct Raster {
    pub size: [usize; 2],
    pub rgba_premultiplied: Vec<u8>,
}

/// Rasterise a view's `svg` artwork stretched over `size` (the same stretch as `render_svg`) with
/// its long side at `long_px` pixels. `Err` when the source is unusable (not SVG, script, ...).
pub fn rasterize(svg: &str, size: [f64; 2], long_px: u32) -> Result<Raster, String> {
    if !(size[0] > 0.0 && size[1] > 0.0 && size[0].is_finite() && size[1].is_finite()) {
        return Err("part has an empty size".into());
    }
    let doc = short_circ_core::svg::artwork_document(svg, size).ok_or("not a usable SVG document")?;
    let tree = resvg::usvg::Tree::from_str(&doc, &resvg::usvg::Options::default()).map_err(|e| e.to_string())?;
    let (w, h) = raster_size(size, long_px);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("cannot allocate the image")?;
    let ts = resvg::tiny_skia::Transform::from_scale(w as f32 / tree.size().width(), h as f32 / tree.size().height());
    resvg::render(&tree, ts, &mut pixmap.as_mut());
    Ok(Raster { size: [w as usize, h as usize], rgba_premultiplied: pixmap.take() })
}

struct Entry {
    /// `None` when the artwork could not be rasterised (not retried until the cache is cleared).
    tex: Option<TextureHandle>,
    last_used: u64,
}

#[derive(Default)]
pub struct TexCache {
    entries: HashMap<(String, u32), Entry>,
    tick: u64,
}

impl TexCache {
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Texture of `vd`'s artwork for a type, sized for `long_px` screen pixels on the long side.
    /// `None` when the part has no artwork or it cannot be rasterised.
    pub fn get(&mut self, ctx: &egui::Context, ty: &str, vd: &ViewDef, long_px: f32) -> Option<TextureId> {
        let svg = vd.svg.as_deref()?;
        self.tick += 1;
        let b = bucket(long_px);
        let key = (ty.to_string(), b);
        if !self.entries.contains_key(&key) {
            let tex = match rasterize(svg, vd.size, b) {
                Ok(r) => {
                    let img = ColorImage::from_rgba_premultiplied(r.size, &r.rgba_premultiplied);
                    Some(ctx.load_texture(format!("art:{ty}:{b}"), img, TextureOptions::LINEAR))
                }
                Err(_) => None,
            };
            self.entries.insert(key.clone(), Entry { tex, last_used: self.tick });
            self.evict(ty, b);
        }
        let e = self.entries.get_mut(&key)?;
        e.last_used = self.tick;
        e.tex.as_ref().map(|t| t.id())
    }

    /// Keep the newest [`KEEP_PER_TYPE`] buckets of `ty`, never dropping `current`.
    fn evict(&mut self, ty: &str, current: u32) {
        let mut mine: Vec<(u32, u64)> =
            self.entries.iter().filter(|((t, _), _)| t == ty).map(|((_, b), e)| (*b, e.last_used)).collect();
        if mine.len() <= KEEP_PER_TYPE {
            return;
        }
        mine.sort_by_key(|(_, used)| *used);
        for (b, _) in mine.into_iter().filter(|(b, _)| *b != current).take(self.count(ty) - KEEP_PER_TYPE) {
            self.entries.remove(&(ty.to_string(), b));
        }
    }

    fn count(&self, ty: &str) -> usize {
        self.entries.keys().filter(|(t, _)| t == ty).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ART: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 40 10'><rect width='20' height='10' fill='#ff0000'/></svg>";

    #[test]
    fn buckets_are_powers_of_two_in_range() {
        assert_eq!(bucket(1.0), 64);
        assert_eq!(bucket(64.0), 64);
        assert_eq!(bucket(64.5), 128);
        assert_eq!(bucket(100.0), 128);
        assert_eq!(bucket(129.0), 256);
        assert_eq!(bucket(4096.0), 4096);
        assert_eq!(bucket(100_000.0), 4096);
        assert_eq!(bucket(f32::NAN), 64);
        assert_eq!(bucket(-5.0), 64);
        assert_eq!(bucket(f32::INFINITY), 64, "garbage input falls back to the smallest bucket");
    }

    #[test]
    fn raster_size_keeps_aspect() {
        assert_eq!(raster_size([4.0, 1.0], 256), (256, 64));
        assert_eq!(raster_size([1.0, 4.0], 256), (64, 256));
        assert_eq!(raster_size([3.0, 3.0], 64), (64, 64));
        assert_eq!(raster_size([1000.0, 1.0], 64), (64, 1), "never zero");
    }

    #[test]
    fn rasterize_stretches_viewbox_over_size() {
        // viewBox 40x10 stretched over a 4x4 part: the red rect covers the left half of a square.
        let r = rasterize(ART, [4.0, 4.0], 64).unwrap();
        assert_eq!(r.size, [64, 64]);
        let px = |x: usize, y: usize| {
            let i = (y * 64 + x) * 4;
            r.rgba_premultiplied[i..i + 4].to_vec()
        };
        assert_eq!(px(8, 32), vec![255, 0, 0, 255], "left half is the rect");
        assert_eq!(px(56, 32)[3], 0, "right half is transparent");
        // Non-square: long side gets the requested pixels.
        let wide = rasterize(ART, [4.0, 1.0], 128).unwrap();
        assert_eq!(wide.size, [128, 32]);
    }

    #[test]
    fn bad_artwork_is_an_error() {
        assert!(rasterize("not svg", [2.0, 2.0], 64).is_err());
        assert!(rasterize("<svg xmlns='http://www.w3.org/2000/svg'><script/></svg>", [2.0, 2.0], 64).is_err());
        assert!(rasterize(ART, [0.0, 2.0], 64).is_err());
    }

    #[test]
    fn real_part_artwork_rasterises() {
        let lib = short_circ_core::Library::builtin();
        for ty in ["resistor", "led", "battery", "breadboard_half"] {
            let vd = lib.get(ty).unwrap().breadboard.as_ref().unwrap();
            let r = rasterize(vd.svg.as_deref().unwrap(), vd.size, 256).unwrap_or_else(|e| panic!("{ty}: {e}"));
            assert!(r.rgba_premultiplied.chunks(4).any(|p| p[3] > 0), "{ty} drew nothing");
        }
    }

    #[test]
    fn cache_reuses_buckets_and_limits_entries() {
        let ctx = egui::Context::default();
        let vd = ViewDef { size: [4.0, 1.0], svg: Some(ART.into()), ..ViewDef::default() };
        let mut c = TexCache::default();
        let a = c.get(&ctx, "r", &vd, 100.0).unwrap();
        assert_eq!(c.get(&ctx, "r", &vd, 120.0), Some(a), "same bucket, same texture");
        assert_eq!(c.len(), 1);
        let b = c.get(&ctx, "r", &vd, 200.0).unwrap();
        assert_ne!(a, b, "a new bucket re-renders");
        for px in [400.0, 800.0, 1600.0] {
            c.get(&ctx, "r", &vd, px).unwrap();
        }
        assert_eq!(c.len(), KEEP_PER_TYPE, "old buckets are evicted");
        // Parts without artwork or with broken artwork give None.
        assert!(c.get(&ctx, "plain", &ViewDef::default(), 100.0).is_none());
        let broken = ViewDef { size: [1.0, 1.0], svg: Some("junk".into()), ..ViewDef::default() };
        assert!(c.get(&ctx, "broken", &broken, 100.0).is_none());
        c.clear();
        assert_eq!(c.len(), 0);
    }
}
