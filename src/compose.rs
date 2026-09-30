//! Inscription composition — supporting strokes, the hero with its bloom,
//! and the faint work title beneath.
//!
//! Three pieces split across submodules:
//! - [`hero`] paints the focal line and its bloom.
//! - [`seal`] paints the title + cinnabar seal stamp.
//! - this file owns the layout solver ([`place_slot`]), the supporting-slot
//!   renderer, and the per-frame [`paint_composition`] orchestrator.

mod hero;
mod seal;

pub use hero::paint as paint_hero;
pub use seal::paint as paint_work_title;

use crate::color::{self, rgb};
use crate::glyph;
use crate::phrase;
use crate::rhythm::Beat;
use crate::scene::{Align, Scene, Slot, SlotDef, SlotRole};

#[allow(clippy::too_many_arguments)]
pub fn paint_composition(
    fb: &mut [u32],
    w: u32,
    h: u32,
    scene: &Scene,
    beat: Option<&Beat>,
    warmth: f32,
    pulse: f32,
    time: f32,
) {
    for slot in &scene.composition.slots {
        if matches!(slot.def.role, SlotRole::Support) {
            paint_supporting_slot(fb, w, h, slot, time, warmth, pulse);
        }
    }
    if let Some(b) = beat {
        let hero = &scene.composition.slots[scene.composition.hero_idx];
        hero::paint(fb, w, h, b, hero.phrase, warmth, pulse, time, &hero.def);
    }
    if let Some(&group) = phrase::POEM_BY_THEME.get(scene.theme_idx) {
        seal::paint(
            fb,
            w,
            h,
            phrase::poem_group_title(group),
            warmth,
            pulse,
            time,
        );
    }
}

/// Resolve a slot's (pen x, baseline y, Q8 scale) from its geometry.
pub fn place_slot(def: &SlotDef, w: u32, h: u32, char_count: usize) -> (i32, i32, u32) {
    let hero_em = glyph::HERO_EM_PX as f32;
    let mut target_px = if def.em_scale <= 0.0 {
        let ideal = w as f32 * def.target_w_frac.max(0.1);
        (ideal / (char_count as f32 * 1.06)).clamp(40.0, hero_em)
    } else {
        (def.em_scale * hero_em).clamp(16.0, hero_em)
    };

    let width_of = |px: f32| (px * 1.06) as i32 * (char_count as i32 - 1).max(0) + px as i32;
    let mut total_w = width_of(target_px);
    let (ax, ay) = (
        (def.x_frac * w as f32) as i32,
        (def.y_frac * h as f32) as i32,
    );
    let safe = 16i32;
    let (safe_x0, safe_x1) = match def.align {
        Align::Left => (ax + safe, w as i32 - safe),
        Align::Right => (safe, ax - safe),
        Align::Center => (ax - w as i32 / 2 + safe, ax + w as i32 / 2 - safe),
    };
    // Shrink until the run fits, or until it reaches the 14px floor where there
    // is nothing left to give. A fixed attempt count is not enough: eight
    // characters on a 320px screen needs eight shrinks to come inside the edge.
    for _ in 0..32 {
        let pen_end = match def.align {
            Align::Left => safe_x0 + total_w,
            Align::Right => safe_x1,
            Align::Center => ax - total_w / 2 + total_w,
        };
        let pen_x = match def.align {
            Align::Left => safe_x0,
            Align::Right => safe_x1 - total_w,
            Align::Center => ax - total_w / 2,
        };
        if pen_x >= 0 && pen_end <= w as i32 {
            break;
        }
        if target_px <= 14.0 {
            break;
        }
        target_px = (target_px * 0.9).max(14.0);
        total_w = width_of(target_px);
    }
    total_w = width_of(target_px);
    let pen_x = match def.align {
        Align::Left => safe_x0,
        Align::Right => safe_x1 - total_w,
        Align::Center => ax - total_w / 2,
    };
    let top_pad = target_px * 0.85;
    let bot_pad = target_px * 0.10 + 2.0;
    let mut baseline_y = ay + (target_px * 0.05) as i32;
    baseline_y = baseline_y.clamp((top_pad as i32) + safe, (h as f32 - bot_pad) as i32 - safe);
    let scale_q8 = ((target_px / hero_em) * 256.0).round() as u32;
    (pen_x, baseline_y, scale_q8)
}

fn stroke_color(def: &SlotDef, warmth: f32) -> u32 {
    let base = mix(color::ink::CREAM, color::ink::SHADOW, def.shadow_mix);
    // Upper strokes can catch a breath of cool moonlight; lower strokes and
    // touch warmth push the ink toward the warm register.
    let height_cool = ((0.40 - def.y_frac).max(0.0) / 0.40).clamp(0.0, 1.0) * 0.30;
    let warm_tint = (warmth * (1.0 - def.shadow_mix) * 0.25
        + ((def.y_frac - 0.55).max(0.0) * 0.35))
        .clamp(0.0, 0.5);
    let p = mix(base, color::ink::WARM, warm_tint);
    mix(p, color::star::COOL, height_cool)
}

fn paint_supporting_slot(
    fb: &mut [u32],
    w: u32,
    h: u32,
    slot: &Slot,
    time: f32,
    warmth: f32,
    pulse: f32,
) {
    let alpha = slot.alpha_now();
    if alpha < 0.01 {
        return;
    }
    let n_chars = slot.phrase.text.chars().count();
    if n_chars == 0 {
        return;
    }
    let (pen_x, baseline, scale_q8) = place_slot(&slot.def, w, h, n_chars);
    let (dx, dy) = slot.drift(time);
    let (pen_x, baseline) = (pen_x + dx as i32, baseline + dy as i32);
    let breath = 1.0 + 0.25 * pulse * (1.0 - slot.def.shadow_mix);
    let alpha = (alpha * breath).clamp(0.0, 1.0);
    let ink = stroke_color(&slot.def, warmth);

    for (i, ch) in slot.phrase.text.chars().enumerate() {
        let glyph_idx = glyph::index_for(ch as u32);
        let advance = glyph::HERO_TABLE[glyph_idx as usize].advance as i32 * scale_q8 as i32;
        glyph::draw_glyph(
            fb,
            w as usize,
            h as usize,
            glyph_idx,
            ink,
            ink,
            pen_x * 256 + advance * i as i32,
            baseline * 256,
            scale_q8,
            alpha,
        );
    }
}

#[inline]
pub(super) fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    rgb(
        (color::r(a) as f32 + (color::r(b) as f32 - color::r(a) as f32) * t) as u8,
        (color::g(a) as f32 + (color::g(b) as f32 - color::g(a) as f32) * t) as u8,
        (color::b(a) as f32 + (color::b(b) as f32 - color::b(a) as f32) * t) as u8,
    )
}

#[cfg(test)]
mod tests;
