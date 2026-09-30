//! Static slot layout — the four inscription positions the screen always
//! starts on. Tuned over many iterations; each `SlotDef` carries the geometry,
//! drift envelope and fade timing the renderer uses for the rest of the run.

use super::slot::{Align, Slot, SlotDef, SlotRole};

/// Beats spent on one poem line before the hero advances to the next.
pub const BEATS_PER_LINE: u64 = 4;

/// Beats for one full pass of a four-line poem; then the work rotates.
pub const BEATS_PER_WORK: u32 = 16;

/// Build the canonical one-hero-plus-three-support composition. All other state
/// (`theme_idx`, `beats_since_theme`, …) starts fresh.
pub fn build_default() -> (Vec<Slot>, usize) {
    let defs: Vec<SlotDef> = vec![
        SlotDef {
            role: SlotRole::Hero,
            x_frac: 0.50,
            y_frac: 0.42,
            align: Align::Center,
            em_scale: 0.0,
            target_w_frac: 0.58,
            max_chars: 8,
            alpha: 1.0,
            shadow_mix: 0.0,
            drift_x: 3.0,
            drift_y: 2.0,
            drift_fx: 0.18,
            drift_fy: 0.13,
            drift_phase: 0.0,
            lifetime: f32::INFINITY,
            fade_in: 0.45,
            fade_out: 0.55,
            stagger: 0.0,
        },
        // Closest echo to the hero, set at a quiet weight so the focal
        // voice stands alone and the three supporting strokes read as
        // whispers of the same poem rather than a title-and-subtitle pair.
        // Drift is half the hero's, so the supporting line settles into
        // its place as the calmest of the voices still close enough to
        // the hero to feel like its echo (upper-right / lower-left are
        // even calmer, at ±~1 px).
        SlotDef {
            role: SlotRole::Support,
            x_frac: 0.50,
            y_frac: 0.58,
            align: Align::Center,
            em_scale: 0.369,
            target_w_frac: 0.0,
            max_chars: 7,
            alpha: 0.30,
            shadow_mix: 0.18,
            drift_x: 1.5,
            drift_y: 0.75,
            drift_fx: 0.21,
            drift_fy: 0.17,
            drift_phase: 0.7,
            lifetime: 12.0,
            fade_in: 0.55,
            fade_out: 0.7,
            stagger: 0.18,
        },
        // Upper-right echo — shares the moon's stillness so the
        // upper-right reads as one constellation (moon + echo).
        SlotDef {
            role: SlotRole::Support,
            x_frac: 0.82,
            y_frac: 0.27,
            align: Align::Right,
            em_scale: 0.32,
            target_w_frac: 0.0,
            max_chars: 5,
            alpha: 0.533,
            shadow_mix: 0.30,
            drift_x: 1.2,
            drift_y: 0.9,
            drift_fx: 0.13,
            drift_fy: 0.16,
            drift_phase: 1.4,
            lifetime: 12.0,
            fade_in: 0.6,
            fade_out: 0.7,
            stagger: 0.34,
        },
        // Lower-left echo — faintest voice, paired with the upper-right
        // echo in motion (both ±~1 px) so neither competes with the focal
        // line for attention. Sized just large enough that the five
        // characters hold together without raising the alpha.
        SlotDef {
            role: SlotRole::Support,
            x_frac: 0.18,
            y_frac: 0.74,
            align: Align::Left,
            em_scale: 0.245,
            target_w_frac: 0.0,
            max_chars: 5,
            alpha: 0.4305,
            shadow_mix: 0.46,
            drift_x: 1.2,
            drift_y: 0.9,
            drift_fx: 0.13,
            drift_fy: 0.16,
            drift_phase: 2.8,
            lifetime: 12.0,
            fade_in: 0.6,
            fade_out: 0.7,
            stagger: 0.50,
        },
    ];
    let slots: Vec<Slot> = defs.into_iter().map(Slot::new).collect();
    (slots, 0)
}
