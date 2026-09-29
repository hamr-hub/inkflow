//! Inscription composition — supporting strokes, the hero with its bloom,
//! and the faint work title beneath.

use crate::color::{self, rgb};
use crate::glyph;
use crate::phrase::{self, Phrase};
use crate::rhythm::{Beat, Phase};
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
        paint_hero(fb, w, h, b, hero.phrase, warmth, pulse, time, &hero.def);
    }
    if let Some(&group) = phrase::POEM_BY_THEME.get(scene.theme_idx) {
        paint_work_title(
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
    let height_cool = ((0.40 - def.y_frac).max(0.0) / 0.40).clamp(0.0, 1.0) * 0.1025;
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

#[allow(clippy::too_many_arguments)]
pub fn paint_hero(
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

/// Faint title inscription near the bottom — the calligrapher's seal line.
fn paint_work_title(
    fb: &mut [u32],
    w: u32,
    h: u32,
    title: &str,
    warmth: f32,
    pulse: f32,
    time: f32,
) {
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
/// line. The mark is solid (no carved interior): at this size a textured glyph
/// would just read as noise, while a flat stamp reads unmistakably as a seal.
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
                color::blend_over_lin(fb[row + sx as usize], color::ink::SEAL, seal_alpha);
        }
    }
}

#[inline]
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    rgb(
        (color::r(a) as f32 + (color::r(b) as f32 - color::r(a) as f32) * t) as u8,
        (color::g(a) as f32 + (color::g(b) as f32 - color::g(a) as f32) * t) as u8,
        (color::b(a) as f32 + (color::b(b) as f32 - color::b(a) as f32) * t) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::HERO_EM_PX;
    use crate::scene::{Align, Composition, SlotDef, SlotRole};

    /// The width `place_slot` laid out, recovered from the scale it returned.
    /// `scale_q8` is rounded, so this can be off by about a pixel per glyph;
    /// callers get the tolerance, not the test.
    fn laid_out_width(scale_q8: u32, char_count: usize) -> i32 {
        let px = scale_q8 as f32 / 256.0 * HERO_EM_PX as f32;
        (px * 1.06) as i32 * (char_count as i32 - 1).max(0) + px as i32
    }

    fn a_slot(align: Align) -> SlotDef {
        let base = Composition::default_layout().slots[0].def;
        SlotDef {
            role: SlotRole::Support,
            align,
            em_scale: 0.30,
            ..base
        }
    }

    /// The piece promises never to run a line off the edge, and the layouts cap
    /// a slot at `max_chars` (8 for the hero, less below it) while the pinned
    /// quatrains are five characters. Within that bound the run has to fit at
    /// any of these sizes, in every alignment.
    #[test]
    fn a_slot_never_lays_out_past_the_edge_of_the_screen() {
        let sizes = [
            (1920u32, 1080u32),
            (1280, 720),
            (800, 480),
            (640, 360),
            (320, 240),
        ];
        for (w, h) in sizes {
            for align in [Align::Left, Align::Center, Align::Right] {
                for char_count in 1..=8usize {
                    let def = a_slot(align);
                    let (pen_x, _y, scale_q8) = place_slot(&def, w, h, char_count);
                    let right = pen_x + laid_out_width(scale_q8, char_count);
                    // One pixel per glyph of slack for the scale rounding.
                    let slack = char_count as i32;
                    assert!(
                        pen_x >= -slack && right <= w as i32 + slack,
                        "{align:?} at {w}x{h}, {char_count} chars: \
                         pen_x={pen_x} right={right} (screen {w})"
                    );
                }
            }
        }
    }

    /// Past that bound the run cannot fit even at the 14px floor — twelve
    /// characters need 168px and a 320px screen has 144 left of margin. The
    /// contract is that it bottoms out at the floor rather than shrinking to
    /// something unreadable or overflowing by an arbitrary amount.
    #[test]
    fn content_too_long_for_the_screen_bottoms_out_at_the_floor() {
        let (w, h) = (320u32, 240u32);
        let (_x, _y, scale_q8) = place_slot(&a_slot(Align::Left), w, h, 12);
        let px = scale_q8 as f32 / 256.0 * HERO_EM_PX as f32;
        assert!(
            px < 15.0,
            "an impossible line should stop at the 14px floor, not {px:.1}px"
        );
    }

    /// The real slots, at the length the real poems are, on the real screen.
    #[test]
    fn the_actual_composition_fits_its_own_lines() {
        let (w, h) = (1280u32, 720u32);
        for slot in Composition::default_layout().slots {
            let def = slot.def;
            let n = def.max_chars.min(5); // the quatrains are five characters
            let (pen_x, y, scale_q8) = place_slot(&def, w, h, n);
            let right = pen_x + laid_out_width(scale_q8, n);
            assert!(
                pen_x >= 0 && right <= w as i32,
                "{:?} pen_x={pen_x} right={right}",
                def.role
            );
            assert!(y > 0 && y < h as i32, "{:?} baseline {y}", def.role);
        }
    }

    /// The baseline has to stay on screen too, or a line's ink is clipped by
    /// the frame rather than placed.
    #[test]
    fn the_baseline_stays_within_the_frame() {
        for (w, h) in [(1280u32, 720u32), (640, 360), (320, 240)] {
            for align in [Align::Left, Align::Center, Align::Right] {
                for char_count in 1..=12usize {
                    let def = a_slot(align);
                    let (_x, y, _s) = place_slot(&def, w, h, char_count);
                    assert!(
                        y > 0 && y < h as i32,
                        "{align:?} at {w}x{h}, {char_count} chars: baseline {y}"
                    );
                }
            }
        }
    }

    /// More characters must mean a smaller line, or a five-character quatrain
    /// and a two-character pair would be drawn at the same size.
    #[test]
    fn longer_phrases_are_set_smaller_when_auto_sizing() {
        let mut def = a_slot(Align::Center);
        def.em_scale = 0.0;
        // 0.20 keeps both ends clear of the clamps: a wider fraction has 2 and
        // 5 characters both pinned to the hero em, so there is nothing to
        // compare.
        def.target_w_frac = 0.20;
        let px = |n: usize| {
            let (_, _, s) = place_slot(&def, 1280, 720, n);
            s as f32 / 256.0 * HERO_EM_PX as f32
        };
        assert!(px(2) > px(5), "2 chars {} vs 5 chars {}", px(2), px(5));
        assert!(px(5) > px(8), "5 chars {} vs 8 chars {}", px(5), px(8));
    }

    /// Alignment has to mean what it says, since the composition leans on the
    /// upper-right echo hanging from the right edge and the title from the left.
    #[test]
    fn alignment_anchors_the_way_it_claims() {
        let (w, h) = (1280u32, 720u32);
        let n = 5usize;

        let left = place_slot(&a_slot(Align::Left), w, h, n);
        let right = place_slot(&a_slot(Align::Right), w, h, n);
        let centre = place_slot(&a_slot(Align::Center), w, h, n);

        let rw = laid_out_width(right.2, n);
        let cw = laid_out_width(centre.2, n);

        // Left: pen starts one safe-margin in from the anchor. Right: the run
        // *ends* there. Both are relative to x_frac, not the frame edge.
        let def = a_slot(Align::Center);
        let ax = (def.x_frac * w as f32) as i32;
        assert_eq!(left.0, ax + 16, "left pen should sit on the margin");
        assert_eq!(right.0 + rw, ax - 16, "right run should end on the margin");
        // Centre: the middle of the run lands on the anchor.
        let mid = centre.0 as f32 + cw as f32 / 2.0;
        let anchor = 0.50 * w as f32;
        assert!(
            (mid - anchor).abs() < 2.0,
            "centre drifted to {mid} vs {anchor}"
        );
    }

    /// A slot with no declared em_scale sizes itself to the fraction of the
    /// width it was given; one with a scale ignores that fraction.
    #[test]
    fn explicit_scale_wins_over_the_width_fraction() {
        let mut def = a_slot(Align::Center);
        def.em_scale = 0.0;
        def.target_w_frac = 0.2;
        let narrow = place_slot(&def, 1280, 720, 5);
        def.target_w_frac = 0.9;
        let wide = place_slot(&def, 1280, 720, 5);
        assert!(
            wide.2 > narrow.2,
            "a wider target fraction should give a bigger line ({} vs {})",
            wide.2,
            narrow.2
        );

        def.em_scale = 0.25;
        def.target_w_frac = 0.2;
        let scaled_a = place_slot(&def, 1280, 720, 5);
        def.target_w_frac = 0.9;
        let scaled_b = place_slot(&def, 1280, 720, 5);
        assert_eq!(
            scaled_a.2, scaled_b.2,
            "an explicit em_scale must ignore it"
        );
    }
}
