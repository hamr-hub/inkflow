use super::*;
use crate::color::{b, g, r};

fn luma(fb: &[u32], w: u32, x: i32, y: i32) -> f32 {
    let p = fb[(y as u32 * w + x as u32) as usize];
    (r(p) as f32 + g(p) as f32 + b(p) as f32) / 3.0
}

/// The anchor the painter actually uses, phase drift included.
fn anchor(scene: &Scene) -> (i32, i32) {
    (
        (scene.moon_x + scene.moon_phase.sin() * 3.0) as i32,
        (scene.moon_y + (scene.moon_phase * 0.6).cos() * 1.5) as i32,
    )
}

fn sky(w: u32, h: u32) -> (Scene, Vec<u32>) {
    let scene = Scene::new(w, h);
    let mut fb = vec![0u32; (w * h) as usize];
    paint_background(&mut fb, w, h, &scene, 0.0, 0.0);
    (scene, fb)
}

/// The module doc claims every falloff is smooth, with no hard circle
/// cutoff. The moon's glow used to break that: the early-out and the
/// per-layer guards cut at 0.003 of linear light, and where this sky sits
/// one 8-bit level is only ~0.0006, so the outermost painted ring was a
/// three-to-five level cliff and a circle appeared around the moon.
///
/// Only the tail is checked. Just outside the disc the halo genuinely falls
/// off about two levels per pixel, which is the fall-off working; the seam
/// was a step *after* a long flat approach, so start past the halo.
#[test]
fn moon_glow_has_no_hard_circular_cutoff() {
    let (w, h) = (640u32, 360u32);
    let (scene, fb) = sky(w, h);
    let (cx, cy) = anchor(&scene);

    for r in 110..200 {
        let step = (luma(&fb, w, cx - r, cy) - luma(&fb, w, cx - r - 1, cy)).abs();
        assert!(
            step < 2.0,
            "glow jumps {step:.1} levels at r={r} — the disc is being cut, not faded"
        );
    }
}

/// The fix that removed the seam also shortened the tail. A moon whose glow
/// stopped early would pass the test above while losing the air it sits in,
/// so pin the reach: well outside the disc, the sky is still lifted.
#[test]
fn moon_glow_still_lights_the_air_around_itself() {
    let (w, h) = (640u32, 360u32);
    let (scene, fb) = sky(w, h);
    let (cx, cy) = anchor(&scene);

    // The same sky with the moon parked off-canvas, for reference.
    let mut dark = Scene::new(w, h);
    dark.moon_x = -10_000.0;
    let mut plain = vec![0u32; (w * h) as usize];
    paint_background(&mut plain, w, h, &dark, 0.0, 0.0);

    let lit = luma(&fb, w, cx - 150, cy);
    let unlit = luma(&plain, w, cx - 150, cy);
    assert!(
        lit > unlit,
        "at r=150 the glow is no brighter than a moonless sky ({lit:.1} vs {unlit:.1})"
    );
    assert!(
        luma(&fb, w, cx - 40, cy) > lit,
        "glow does not fall off outward"
    );
}
