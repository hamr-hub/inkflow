use super::*;

/// A blend that contributes nothing must leave the pixel exactly as it
/// found it. Every layer in the piece is composited through these ops, so
/// a broken identity here is a whole-frame drift that no amount of looking
/// at a screenshot would catch.
#[test]
fn zero_alpha_blends_are_the_identity() {
    for v in 0..=255u8 {
        let p = rgb(v, v.wrapping_mul(3), v.wrapping_mul(7));
        assert_eq!(blend_add_lin(p, rgb(200, 224, 248), 0.0), p);
        assert_eq!(blend_screen(p, rgb(200, 224, 248), 0.0), p);
        assert_eq!(blend_over_lin(p, rgb(200, 224, 248), 0.0), p);
        assert_eq!(blend_mul(p, rgb(200, 224, 248), 0.0), p);
    }
}

/// The sRGB round trip has to be exact for every 8-bit value, or a
/// no-op round trip through a blend would quietly shift the palette.
#[test]
fn srgb_round_trip_is_exact() {
    for v in 0..=255u8 {
        assert_eq!(lin_to_srgb(srgb_to_lin(v)), v, "round trip broke at {v}");
    }
}

/// Guards the threshold the moon's glow relies on. `GLOW_MIN` in
/// background.rs only keeps the disc boundary invisible because an alpha
/// that small still moves a channel by at most one level; at the old
/// 0.003 it moved three to five, which is the seam it used to print.
/// Guards the threshold the moon's glow relies on. `GLOW_MIN` in
/// background.rs only keeps the disc boundary invisible because an alpha
/// that small still moves a channel by at most one level, wherever the sky
/// actually is.
///
/// The floor is level 8, and that is not a fudge: the piece's background
/// measures around 25-29 where the glow lands, and the next test records
/// why anything below 8 behaves differently.
#[test]
fn a_negligible_add_moves_at_most_one_level() {
    for v in 8..=255u8 {
        let got = blend_add_lin(rgb(v, v, v), star::COOL, 0.0003);
        let worst = [r(got), g(got), b(got)]
            .iter()
            .map(|&c| (c as i32 - v as i32).abs())
            .max()
            .unwrap();
        assert!(
            worst <= 1,
            "alpha 0.0003 moved a channel by {worst} at level {v}"
        );
    }
}

/// The pipeline encodes with gamma 2.0, so the darkest levels have a very
/// high gain: on pure black an alpha of 0.0003 still lands near level 4.
/// Nothing in the piece composites a glow onto black, but this is why
/// "negligible alpha" is not a uniform guarantee, and why `GLOW_MIN` is
/// not simply "as small as possible".
#[test]
fn near_black_has_high_gain() {
    let lifted = blend_add_lin(rgb(0, 0, 0), star::COOL, 0.0003);
    assert!(
        r(lifted) >= 3,
        "expected gamma gain near black, got level {}",
        r(lifted)
    );
}

/// Light only adds, and it saturates rather than wrapping around.
#[test]
fn additive_light_never_darkens_and_never_wraps() {
    let dst = rgb(40, 90, 160);
    for step in 0..=10 {
        let a = step as f32 / 10.0;
        let got = blend_add_lin(dst, rgb(255, 255, 255), a);
        assert!(r(got) >= r(dst) && g(got) >= g(dst) && b(got) >= b(dst));
    }
    let white = blend_add_lin(rgb(250, 250, 250), rgb(255, 255, 255), 1.0);
    assert_eq!((r(white), g(white), b(white)), (255, 255, 255));
}

/// Alpha-over endpoints, and that a out-of-range alpha is clamped rather
/// than allowed to extrapolate past the source colour.
#[test]
fn over_lin_endpoints_and_clamping() {
    let (black, white) = (rgb(0, 0, 0), rgb(255, 255, 255));
    assert_eq!(blend_over_lin(white, black, 0.0), white);
    assert_eq!(blend_over_lin(black, white, 1.0), white);
    assert_eq!(blend_over_lin(white, black, 1.0), black);
    assert_eq!(
        blend_over_lin(white, black, 2.0),
        black,
        "alpha > 1 must clamp"
    );
    assert_eq!(
        blend_over_lin(white, black, -1.0),
        white,
        "alpha < 0 must clamp"
    );
}

/// Screen is the soft additive the atmosphere leans on: it must reach
/// white but never carry past it.
#[test]
fn screen_soft_adds_to_white() {
    let black = rgb(0, 0, 0);
    assert_eq!(blend_screen(black, white_rgb(), 1.0), white_rgb());
    let mid = blend_screen(black, white_rgb(), 0.5);
    assert!(
        r(mid) > 0 && r(mid) < 255,
        "half-strength screen should be mid-grey"
    );
}

fn white_rgb() -> u32 {
    rgb(255, 255, 255)
}

#[test]
fn gradients_land_on_their_stops() {
    let (a, b, c) = (rgb(10, 20, 30), rgb(120, 130, 140), rgb(240, 250, 255));
    assert_eq!(grad2(a, b, 0.0), a);
    assert_eq!(grad2(a, b, 1.0), b);
    assert_eq!(grad2(a, b, -5.0), a, "u below range clamps");
    assert_eq!(grad2(a, b, 5.0), b, "u above range clamps");
    assert_eq!(grad3(a, b, c, 0.0), a);
    assert_eq!(grad3(a, b, c, 1.0), c);
}

/// The window the moon's tail uses. Endpoints must be exact or the glow
/// does not actually reach zero where the code says it does.
#[test]
fn smoothstep_endpoints_are_exact_and_clamped() {
    assert_eq!(smoothstep(0.0), 0.0);
    assert_eq!(smoothstep(1.0), 1.0);
    assert_eq!(smootherstep(0.0), 0.0);
    assert_eq!(smootherstep(1.0), 1.0);
    assert_eq!(smoothstep(-1.0), 0.0);
    assert_eq!(smoothstep(2.0), 1.0);
    assert_eq!(smootherstep(-1.0), 0.0);
    assert_eq!(smootherstep(2.0), 1.0);
}

#[test]
fn channel_packing_round_trips() {
    for (rr, gg, bb) in [(0u8, 0u8, 0u8), (255, 255, 255), (1, 2, 3), (200, 224, 248)] {
        let p = rgb(rr, gg, bb);
        assert_eq!((r(p), g(p), b(p)), (rr, gg, bb));
    }
}
