//! Hero inscription paint — the focal line, with its bloom and per-character
//! stagger.
//!
//! `paint_hero` is the per-frame entry point; `draw_bloom` paints one outline
//! pass of the glyph at a slightly different scale for the soft outer halo.

use super::{mix, place_slot};
use crate::color;
use crate::glyph;
use crate::phrase::Phrase;
use crate::rhythm::{Beat, Phase};
use crate::scene::SlotDef;

#[allow(clippy::too_many_arguments)]
pub fn paint(
    fb: &mut [u32],
    w: u32,
    h: u32,
    beat: &Beat,
    phrase: &'static Phrase,
    warmth: f32,
    pulse: f32,
    time: f32,
    def: &SlotDef,
) {
    let n_chars = phrase.text.chars().count();
    if n_chars == 0 {
        return;
    }
    let (pen_x, baseline, base_scale) = place_slot(def, w, h, n_chars);
    let dx = def.drift_x * (time * def.drift_fx + def.drift_phase).sin();
    let dy = def.drift_y * (time * def.drift_fy + def.drift_phase * 1.3).cos();
    let (pen_x, baseline) = (pen_x + dx as i32, baseline + dy as i32);

    let ep = match beat.phase {
        Phase::Entrance => beat.entrance_progress(),
        Phase::Hold => 1.0,
        Phase::Exit => 1.0 - beat.exit_progress(),
        Phase::Rest => 0.30,
    };
    let ep_eased = color::smootherstep(ep);
    let slide_y = match beat.phase {
        Phase::Entrance => ((1.0 - ep) * 24.0) as i32,
        Phase::Exit => (ep * 18.0) as i32,
        _ => 0,
    };

    let ink = mix(color::ink::CREAM, color::ink::WARM, warmth * 0.5);
    let glow = mix(color::ink::GLOW, color::ink::WARM, warmth * 0.35);
    let glow_alpha = (0.10 + 0.16 * pulse + 0.06 * warmth + phrase.glow * 0.10).clamp(0.0, 0.5);
    let bloom_alpha = (0.04 + 0.04 * pulse + 0.02 * warmth + phrase.glow * 0.03).clamp(0.0, 0.12);
    let bloom2_alpha = (0.02 + 0.02 * pulse + 0.01 * warmth + phrase.glow * 0.015).clamp(0.0, 0.06);

    // Gentle overshoot pop at the end of the entrance.
    let overshoot = if matches!(beat.phase, Phase::Entrance) {
        let p = beat.entrance_progress();
        if p < 0.7 {
            1.0
        } else {
            let k = (p - 0.7) / 0.3;
            1.0 + 0.06 * (1.0 - k) * (k * core::f32::consts::TAU).sin()
        }
    } else if matches!(beat.phase, Phase::Hold) {
        1.0 + 0.012 * pulse * (beat.t_in_beat * 1.7).sin()
    } else {
        1.0
    };
    let scale = ((base_scale as f32) * overshoot).round() as u32;

    let total_delay = 0.4_f32;
    let n_chars_f = n_chars as f32;
    let per_char = (1.0 - total_delay) / n_chars_f;
    for (i, ch) in phrase.text.chars().enumerate() {
        // Rest ghost stays a complete word; entrance/exit dissolve per char.
        let stagger = total_delay * i as f32 / n_chars_f;
        let local = if matches!(beat.phase, Phase::Rest) {
            ep
        } else {
            color::smootherstep(((ep - stagger) / per_char).clamp(0.0, 1.0))
        };
        let char_alpha = if matches!(beat.phase, Phase::Rest) {
            local
        } else {
            (local * ep_eased).clamp(0.0, 1.0)
        };
        if char_alpha <= 0.005 {
            continue;
        }
        let glyph_idx = glyph::index_for(ch as u32);
        let advance = glyph::HERO_TABLE[glyph_idx as usize].advance as i32 * scale as i32;
        let fx = pen_x * 256 + advance * i as i32;
        let fy = (baseline + slide_y) * 256;
        let micro = 1.0 + 0.06 * (1.0 - local) * (local * core::f32::consts::TAU).sin();
        let char_scale = (scale as f32 * micro) as u32;

        draw_bloom(
            fb,
            w,
            h,
            glyph_idx,
            fx,
            fy,
            char_scale,
            1.10,
            glow,
            bloom2_alpha * char_alpha,
        );
        draw_bloom(
            fb,
            w,
            h,
            glyph_idx,
            fx,
            fy,
            char_scale,
            1.06,
            glow,
            bloom_alpha * char_alpha,
        );
        if glow_alpha * char_alpha > 0.01 {
            glyph::draw_glyph(
                fb,
                w as usize,
                h as usize,
                glyph_idx,
                glow,
                glow,
                fx,
                fy,
                char_scale,
                glow_alpha * char_alpha * 0.45,
            );
        }
        glyph::draw_glyph(
            fb, w as usize, h as usize, glyph_idx, ink, glow, fx, fy, char_scale, char_alpha,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_bloom(
    fb: &mut [u32],
    w: u32,
    h: u32,
    glyph_idx: u8,
    fx: i32,
    fy: i32,
    char_scale: u32,
    grow: f32,
    color: u32,
    alpha: f32,
) {
    if alpha <= 0.005 {
        return;
    }
    let bloom_scale = ((char_scale as f32) * grow).round() as u32;
    let info = glyph::HERO_TABLE[glyph_idx as usize];
    let diff = char_scale as i32 - bloom_scale as i32;
    let bx = info.bearing_x as i32 + info.w as i32 / 2;
    let by = info.bearing_y as i32 - info.h as i32 / 2;
    glyph::draw_glyph(
        fb,
        w as usize,
        h as usize,
        glyph_idx,
        color,
        color,
        fx + diff * bx,
        fy - diff * by,
        bloom_scale,
        alpha,
    );
}
