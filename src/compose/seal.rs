//! Faint title inscription near the bottom — the calligrapher's seal line.
//!
//! Painted as one block (title + cinnabar seal stamp) recentred together so
//! the pair reads as one signature. The seal is a flat rectangle: at this
//! size a textured glyph would just read as noise, while a flat stamp
//! reads unmistakably as a seal.

use super::mix;
use crate::color::{self, blend_over_lin};
use crate::glyph;

/// Paint the title + seal signature line into the bottom band.
pub fn paint(fb: &mut [u32], w: u32, h: u32, title: &str, warmth: f32, pulse: f32, time: f32) {
    let n_chars = title.chars().count();
    if n_chars == 0 {
        return;
    }
    let target_px = 22.0_f32;
    let total_w = (target_px * 1.06) as i32 * (n_chars as i32 - 1) + target_px as i32;
    let dx = 1.6 * (time * 0.11 + 3.7).sin();
    let dy = 1.0 * (time * 0.15 + 4.8).cos();
    // Cinnabar seal stamp — weathered, not lacquer — placed at the right of
    // the signature line so the title + seal pair reads as one inscription.
    // The pair is recentred together so the seal does not push the title off-
    // axis.
    let seal_size = 14i32;
    let seal_gap = 10i32;
    let combined_w = total_w + seal_gap + seal_size;
    let pen_x = (w as i32 - combined_w) / 2 + dx as i32;
    let baseline = h as f32 * 0.90 + dy;

    if baseline - target_px * 0.85 < 16.0 || baseline + target_px * 0.1 > h as f32 - 10.0 {
        return;
    }
    if pen_x < 12 || pen_x + combined_w > w as i32 - 12 {
        return;
    }
    let base = mix(color::ink::CREAM, color::ink::SHADOW, 0.4);
    let ink = mix(base, color::ink::WARM, warmth * 0.4);
    let alpha = (0.5125 * (1.0 + 0.2 * pulse)).clamp(0.0, 0.8);
    let scale = ((target_px / glyph::HERO_EM_PX as f32) * 256.0).round() as u32;

    let mut pen = pen_x * 256;
    for ch in title.chars() {
        let glyph_idx = glyph::index_for(ch as u32);
        if glyph_idx != 0 {
            glyph::draw_glyph(
                fb,
                w as usize,
                h as usize,
                glyph_idx,
                ink,
                ink,
                pen,
                baseline as i32 * 256,
                scale,
                alpha,
            );
        }
        pen += glyph::HERO_TABLE[glyph_idx as usize].advance as i32 * scale as i32;
    }

    paint_seal(
        fb,
        w,
        h,
        pen_x + total_w + seal_gap,
        baseline,
        seal_size,
        alpha,
    );
}

/// Cinnabar seal stamp — a small weathered square at the end of the signature
/// line.
fn paint_seal(
    fb: &mut [u32],
    w: u32,
    h: u32,
    seal_x: i32,
    baseline: f32,
    seal_size: i32,
    title_alpha: f32,
) {
    let seal_top = (baseline - seal_size as f32 * 1.05) as i32;
    let seal_alpha = (title_alpha * 0.78).clamp(0.0, 0.7);
    let x0 = seal_x.max(0);
    let x1 = (seal_x + seal_size).min(w as i32);
    let y0 = seal_top.max(0);
    let y1 = (seal_top + seal_size).min(h as i32);
    if x0 >= x1 || y0 >= y1 || seal_alpha <= 0.005 {
        return;
    }
    for sy in y0..y1 {
        let row = sy as usize * w as usize;
        for sx in x0..x1 {
            fb[row + sx as usize] =
                blend_over_lin(fb[row + sx as usize], color::ink::SEAL, seal_alpha);
        }
    }
}
