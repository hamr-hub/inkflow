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
