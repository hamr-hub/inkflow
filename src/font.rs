// inkflow · font.rs
//
// Runtime software rasterizer for the embedded CJK bitmap font defined in
// src/fontdata.rs. Bitmaps are 4bpp packed; we look up by string key (the
// Python script renders each char as a one-character string). Scaling is
// a fixed integer multiple of the source size — bilinear would be nicer
// but a simple box sample keeps the inner loop branchless and fast
// enough for the 200-or-so glyphs the piece carries through each frame.
//
// All blending goes onto a 32bpp BGRA buffer (DRM dumb-buffer byte order
// little-endian: byte 0 = B, 1 = G, 2 = R, 3 = A — the alpha byte is
// unused for XRGB scanout, so we always leave it at 0xFF).

#![allow(dead_code)]

use crate::fontdata::{GLYPHS, GLYPH_H, GLYPH_W};

#[derive(Copy, Clone, Default)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba(0, 0, 0, 0);
    pub fn from_hsl(h: f32, s: f32, l: f32) -> Self {
        let h = (h * 360.0).rem_euclid(360.);
        let c = (1. - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1. - (((h / 60.0) % 2.0) - 1.0).abs());
        let m = l - c / 2.0;
        let (r, g, b) = match (h as u32) / 60 {
            0 => (c, x, 0.),
            1 => (x, c, 0.),
            2 => (0., c, x),
            3 => (0., x, c),
            4 => (x, 0., c),
            _ => (c, 0., x),
        };
        Rgba(
            ((r + m) * 255.0) as u8,
            ((g + m) * 255.0) as u8,
            ((b + m) * 255.0) as u8,
            255,
        )
    }
    pub fn with_alpha(mut self, a: u8) -> Self {
        self.3 = a;
        self
    }
}

/// Look up the 4bpp bitmap for a single char (string of length 1..=3 for
/// the rare two/three-character glyphs we kept in the subset).
pub fn bitmap_for(ch: &str) -> Option<&'static [u8]> {
    GLYPHS.iter().find(|(k, _)| *k == ch).map(|(_, b)| *b)
}

/// Return the static key string for a char that has a bitmap, or None if
/// `ch` is not in the embedded font. The returned `&'static str` aliases
/// into the compiled GLYPHS table — no heap allocation, no leak. Use this
/// in preference to `String::from(ch)` + `Box::leak` when a caller needs a
/// long-lived `&'static str` for the same char.
pub fn static_key_for(ch: &str) -> Option<&'static str> {
    GLYPHS.iter().find(|(k, _)| *k == ch).map(|(k, _)| *k)
}

/// Draw a single character with optional rotation (radians) and alpha.
/// `size` is the *target* pixel height of the cell. The bitmap lives at
/// GLYPH_W × GLYPH_H; we scale by integer factor closest to size/GLYPH_H.
///
/// `pixels` is row-major 32bpp BGRA, pitch in bytes.
#[allow(clippy::too_many_arguments)]
pub fn draw_glyph(
    pixels: &mut [u32],
    pitch_px: usize,
    fb_w: i32,
    fb_h: i32,
    cx: f32,
    cy: f32,
    size: f32,
    ch: &str,
    color: Rgba,
    alpha: f32,
    rot: f32,
) {
    let Some(bits) = bitmap_for(ch) else { return };
    let cell = GLYPH_W as i32;
    let scale = ((size / GLYPH_H as f32).round() as i32).max(1);
    let dw = cell * scale;
    let dh = cell * scale;

    // alpha overall gate
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u32;
    if a == 0 {
        return;
    }

    let rot_c = rot.cos();
    let rot_s = rot.sin();
    let hw = dw as f32 * 0.5;
    let hh = dh as f32 * 0.5;

    let _x0 = cx - hw;
    let _y0 = cy - hh;

    // Bounding box in screen space (rotation might pull corners outside).
    let corners = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)];
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for &(lx, ly) in &corners {
        let rx = lx * rot_c - ly * rot_s + cx;
        let ry = lx * rot_s + ly * rot_c + cy;
        min_x = min_x.min(rx as i32);
        min_y = min_y.min(ry as i32);
        max_x = max_x.max(rx as i32);
        max_y = max_y.max(ry as i32);
    }
    let min_x = min_x.max(0);
    let min_y = min_y.max(0);
    let max_x = max_x.min(fb_w);
    let max_y = max_y.min(fb_h);
    if max_x <= min_x || max_y <= min_y {
        return;
    }

    let cr = color.0 as u32;
    let cg = color.1 as u32;
    let cb = color.2 as u32;

    // Iterate the screen rect; for each pixel, inverse-rotate into the
    // glyph's local coords, sample the packed 4bpp bitmap, blend.
    for sy in min_y..max_y {
        let row_start = sy as usize * pitch_px;
        for sx in min_x..max_x {
            let lx = sx as f32 - cx;
            let ly = sy as f32 - cy;
            let gx = lx * rot_c + ly * rot_s + hw;
            let gy = -lx * rot_s + ly * rot_c + hh;
            if gx < 0.0 || gy < 0.0 || gx >= dw as f32 || gy >= dh as f32 {
                continue;
            }
            let sample_x = (gx / scale as f32) as i32;
            let sample_y = (gy / scale as f32) as i32;
            let cov = sample_packed(bits, sample_x as u32, sample_y as u32);
            if cov == 0 {
                continue;
            }
            // blend with per-pixel coverage and the global alpha
            let m = (cov as u32 * a) >> 8; // 0..=255
            if m == 0 {
                continue;
            }
            blend_pixel(&mut pixels[row_start + sx as usize], cr, cg, cb, m);
        }
    }
}

#[inline]
fn sample_packed(bits: &[u8], x: u32, y: u32) -> u8 {
    if x >= GLYPH_W || y >= GLYPH_H {
        return 0;
    }
    let idx = (y * GLYPH_W + x) as usize;
    let byte = bits[idx / 2];
    if idx & 1 == 0 {
        byte >> 4
    } else {
        byte & 0xF
    }
}

#[inline]
fn blend_pixel(dst: &mut u32, cr: u32, cg: u32, cb: u32, m: u32) {
    let d = *dst;
    let db = d & 0xFF;
    let dg = (d >> 8) & 0xFF;
    let dr = (d >> 16) & 0xFF;
    let inv = 255 - m;
    let nb = (db * inv + cb * m) >> 8;
    let ng = (dg * inv + cg * m) >> 8;
    let nr = (dr * inv + cr * m) >> 8;
    *dst = nb | (ng << 8) | (nr << 16) | 0xFF_00_00_00;
}

// ---------- non-glyph drawing primitives ----------

/// Filled circle with quadratic AA on the edge. cy < 0 / cx out of frame
/// is a safe no-op — caller doesn't have to clip first.
#[allow(clippy::too_many_arguments)]
pub fn fill_circle(
    pixels: &mut [u32],
    pitch_px: usize,
    fb_w: i32,
    fb_h: i32,
    cx: f32,
    cy: f32,
    r: f32,
    color: Rgba,
    alpha: f32,
) {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u32;
    if a == 0 {
        return;
    }
    let cr = color.0 as u32;
    let cg = color.1 as u32;
    let cb = color.2 as u32;
    let r2 = r * r;
    let inner_r2 = (r - 1.0).max(0.0).powi(2);
    let min_x = (cx - r).max(0.0) as i32;
    let max_x = (cx + r).min(fb_w as f32 - 0.5) as i32;
    let min_y = (cy - r).max(0.0) as i32;
    let max_y = (cy + r).min(fb_h as f32 - 0.5) as i32;
    for sy in min_y..=max_y {
        let row = sy as usize * pitch_px;
        let dy = (sy as f32 - cy + 0.5).abs();
        for sx in min_x..=max_x {
            let dx = (sx as f32 - cx + 0.5).abs();
            let d2 = dx * dx + dy * dy;
            if d2 > r2 {
                continue;
            }
            let cov = if d2 < inner_r2 {
                255
            } else {
                // soft edge — 1-pixel feathered ring
                let d = d2.sqrt();
                ((r - d).clamp(0.0, 1.0) * 255.0) as u32
            };
            let m = (a * cov) >> 8;
            if m == 0 {
                continue;
            }
            blend_pixel(&mut pixels[row + sx as usize], cr, cg, cb, m);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn fill_rect(
    pixels: &mut [u32],
    pitch_px: usize,
    fb_w: i32,
    fb_h: i32,
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    color: Rgba,
    alpha: f32,
) {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u32;
    if a == 0 {
        return;
    }
    let cr = color.0 as u32;
    let cg = color.1 as u32;
    let cb = color.2 as u32;
    let m = a;
    let min_x = x0.max(0);
    let max_x = (x0 + w).min(fb_w);
    let min_y = y0.max(0);
    let max_y = (y0 + h).min(fb_h);
    for sy in min_y..max_y {
        let row = sy as usize * pitch_px;
        for sx in min_x..max_x {
            blend_pixel(&mut pixels[row + sx as usize], cr, cg, cb, m);
        }
    }
}

/// Look up the static key string for a single char that has a bitmap
/// in the embedded font. Returns `"墨"` (the sentinel character) if
/// the char is missing, so the renderer can keep painting even when
/// the LLM sends something exotic. The returned `&'static str` aliases
/// into the compiled GLYPHS table — no allocation.
pub fn char_key(c: char) -> &'static str {
    let mut buf = [0u8; 4];
    let s: &str = c.encode_utf8(&mut buf);
    static_key_for(s).unwrap_or("墨")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_key_returns_sentinel_for_unknown() {
        assert_eq!(char_key('\u{20000}'), "墨");
    }

    #[test]
    fn char_key_returns_self_for_ascii() {
        // ASCII printable chars are always in the embedded font.
        assert_eq!(char_key('A'), "A");
        assert_eq!(char_key(' '), " ");
        assert_eq!(char_key('z'), "z");
    }

    #[test]
    fn f32_clamp_method_behaves() {
        // We rely on f32::clamp (Rust ≥1.50) everywhere. Sanity check
        // the standard library contract — the helper module removed
        // its custom clamp in v0.2.1.
        assert_eq!((-1.0f32).clamp(0.0, 1.0), 0.0);
        assert_eq!(2.0f32.clamp(0.0, 1.0), 1.0);
        assert_eq!(0.5f32.clamp(0.0, 1.0), 0.5);
        assert_eq!(0.0f32.clamp(0.0, 1.0), 0.0);
        assert_eq!(1.0f32.clamp(0.0, 1.0), 1.0);
    }
}
