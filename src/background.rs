//! Atmospheric background — deep vertical gradient, focal aura, horizon
//! mist, vignette, dust, the moon anchor and touch sparks.
//!
//! Every falloff is smooth; no hard circle cutoffs, so the moon reads as a
//! luminous body in continuous air rather than an eclipsed ring.

use crate::color::{self, blend_add_lin, blend_screen, rgb};
use crate::scene::Scene;

#[inline]
fn stops(colors: &[u32], u: f32) -> u32 {
    let p = u.clamp(0.0, 1.0) * (colors.len() - 1) as f32;
    let i = (p as usize).min(colors.len() - 2);
    color::grad2(colors[i], colors[i + 1], p - i as f32)
}

/// Paint the full background into `fb`.
pub fn paint_background(fb: &mut [u32], w: u32, h: u32, scene: &Scene, pulse: f32, warmth: f32) {
    let (w_f, h_f) = (w as f32, h as f32);

    // Vertical atmosphere: five stops, idle = deep ink indigo; warmth
    // interpolates each stop toward a dusk-amber register. Built on the stack —
    // this runs once per frame, so a `Vec` here would be a steady-state
    // allocator round-trip for a value that is 20 bytes.
    const COOL: [u32; 5] = [
        rgb(5, 8, 18),
        rgb(9, 11, 26),
        rgb(16, 15, 36),
        rgb(28, 22, 40),
        rgb(56, 40, 36),
    ];
    const WARM: [u32; 5] = [
        rgb(16, 11, 18),
        rgb(26, 16, 22),
        rgb(44, 27, 30),
        rgb(66, 40, 36),
        rgb(108, 68, 46),
    ];
    let mut sky = [0u32; 5];
    for (i, slot) in sky.iter_mut().enumerate() {
        *slot = mix(COOL[i], WARM[i], warmth * (0.5 + i as f32 * 0.12));
    }

    // Focal aura — a wide, very soft light behind the hero inscription.
    let aura_cx = w_f * 0.5;
    let aura_cy = h_f * (0.42 + 0.02 * scene.moon_phase.cos());
    let aura_r2 = w_f * w_f * 0.34 + h_f * h_f * 0.42;
    let aura_gain = 0.06 + pulse * 0.03;
    let aura_color = mix(rgb(72, 66, 96), rgb(150, 104, 74), warmth);

    // Horizon mist — warm ground light under the lower strokes.
    let horizon_color = mix(rgb(50, 34, 26), rgb(126, 84, 52), warmth);

    let ambient = (0.015 * scene.ambient_pulse.sin() + pulse * 0.05) * 0.5;

    // The vignette and the aura are both separable sums of a per-column and a
    // per-row term, so precompute each axis once instead of re-deriving
    // `nx * nx` and `dy * dy` for every one of the w*h pixels.
    let mut col_nx2 = vec![0.0_f32; w as usize];
    for (x, slot) in col_nx2.iter_mut().enumerate() {
        let nx = (x as f32 / w_f - 0.5) * 2.0;
        *slot = nx * nx;
    }

    for y in 0..h {
        let v = y as f32 / (h_f - 1.0).max(1.0);
        let base = stops(&sky, v);
        let mist = ((v - 0.55) * (1.0 - v) * 9.9).clamp(0.0, 1.0);
        let mist_gain = mist * 0.08;

        let dy = y as f32 - aura_cy;
        let ny = (v - 0.5) * 2.0;
        let ny2 = ny * ny;
        let row = (y * w) as usize;

        for x in 0..w as usize {
            let mut p = base;

            let dx = x as f32 - aura_cx;
            let d2 = (dx * dx + dy * dy) / aura_r2;
            let aura = (1.0 - d2).clamp(0.0, 1.0).powi(2);
            p = blend_screen(p, aura_color, aura * aura_gain);

            // Vignette — darken the corners smoothly toward deepest shadow.
            let vig = (col_nx2[x] + ny2).powf(1.25);
            p = mix(p, color::bg::DEEP, (vig * 0.55).clamp(0.0, 0.78));

            p = blend_add_lin(p, color::star::WARM, ambient);
            p = blend_screen(p, horizon_color, mist_gain);

            fb[row + x] = p;
        }
    }

    paint_dust(fb, w, h, scene);
    paint_moon(fb, w, h, scene, pulse);
    paint_sparks(fb, w, h, scene);
}

fn paint_dust(fb: &mut [u32], w: u32, h: u32, scene: &Scene) {
    for d in &scene.dust {
        let (cx, cy, r) = (d.x as i32, d.y as i32, d.r as i32 + 1);
        for oy in -r..=r {
            for ox in -r..=r {
                let (xx, yy) = (cx + ox, cy + oy);
                if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                    continue;
                }
                let k = (1.0 - (ox * ox + oy * oy) as f32 / (r * r) as f32).max(0.0) * d.a;
                if k > 0.003 {
                    let idx = (yy as u32 * w + xx as u32) as usize;
                    fb[idx] = blend_add_lin(fb[idx], d.hue, k);
                }
            }
        }
    }
}

/// Moon anchor — smooth luminous disc with gentle limb darkening, floating
/// in nested gaussian air (halo + wide sky glow), every edge continuous.
fn paint_moon(fb: &mut [u32], w: u32, h: u32, scene: &Scene, pulse: f32) {
    let mcx = scene.moon_x + scene.moon_phase.sin() * 3.0;
    let mcy = scene.moon_y + (scene.moon_phase * 0.6).cos() * 1.5;
    let (mcx_i, mcy_i) = (mcx as i32, mcy as i32);

    const BODY_R: f32 = 19.0;
    const HALO_SIGMA: f32 = 26.0;
    const SKY_SIGMA: f32 = 66.0;
    let body_peak = 0.5125 * (1.0 + pulse * 0.02);
    let halo_peak = 0.14 * (1.0 + pulse * 0.05);
    let sky_peak = 0.085 * (1.0 + pulse * 0.03);

    // The glow is purely radial, so evaluate its profile once per frame instead
    // of two exp() per pixel. GLOW_MIN is under half an 8-bit level at this
    // depth of sky, where the early-out's step is otherwise a visible ring.
    const GLOW_MIN: f32 = 0.0003;
    const GLOW_R: usize = (SKY_SIGMA * 4.0) as usize + 2;
    let win_lo = SKY_SIGMA * 2.0;
    let win_hi = (GLOW_R - 1) as f32;
    let mut glow_sky = [0.0_f32; GLOW_R];
    let mut glow_halo = [0.0_f32; GLOW_R];
    for (i, (sky, halo)) in glow_sky.iter_mut().zip(glow_halo.iter_mut()).enumerate() {
        let d = i as f32;
        let win = 1.0 - color::smootherstep((d - win_lo) / (win_hi - win_lo));
        *sky = (-0.5 * (d / SKY_SIGMA).powi(2)).exp() * sky_peak * win;
        *halo = (-0.5 * (d / HALO_SIGMA).powi(2)).exp() * halo_peak * win;
    }
    // Past the last ring that still carries light there is nothing to blend, so
    // walk only that far — the window fades to zero well before the table ends.
    let mut reach = 0usize;
    for i in 0..GLOW_R {
        if glow_sky[i] + glow_halo[i] > GLOW_MIN {
            reach = i;
        }
    }
    let extent = reach as i32;
    let reach2 = (reach as f32 + 1.0) * (reach as f32 + 1.0);

    for oy in -extent..=extent {
        for ox in -extent..=extent {
            let (xx, yy) = (mcx_i + ox, mcy_i + oy);
            if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                continue;
            }
            // Reject the far corners on the squared distance alone: most of
            // the square is outside the disc, and this skips its sqrt.
            let d2 = ox * ox + oy * oy;
            if d2 as f32 >= reach2 {
                continue;
            }
            let d = (d2 as f32).sqrt();

            // Limb-darkened body: k = 1 - (d/R)^2, zero smoothly at the edge.
            let u = d / BODY_R;
            let body_k = if u < 1.0 { 1.0 - u * u } else { 0.0 };
            // Terminator — slightly warmer toward the lower (horizon) side.
            let term = 1.0 + 0.12 * (oy as f32 / BODY_R).clamp(-1.0, 1.0);
            let body = body_k * body_peak * term;
            let body_warm = ((oy as f32 / BODY_R) * 0.10).max(0.0);
            let body_cool = ((-oy as f32 / BODY_R) * 0.10).max(0.0);

            // The table covers every d the loop can reach; the +2 in GLOW_R
            // keeps the corner distances (up to extent*sqrt2) in range.
            let di = d as usize;
            let (sky, halo) = if di < GLOW_R {
                (glow_sky[di], glow_halo[di])
            } else {
                (0.0, 0.0)
            };

            let a = sky + halo + body;
            if a <= GLOW_MIN {
                continue;
            }
            let idx = (yy as u32 * w + xx as u32) as usize;
            if sky > GLOW_MIN {
                fb[idx] = blend_add_lin(fb[idx], color::star::COOL, sky);
            }
            if halo > GLOW_MIN {
                fb[idx] = blend_add_lin(fb[idx], color::ink::WARM, halo);
            }
            if body > GLOW_MIN {
                let mut body_color = mix(color::ink::WARM, color::drop::AMBER, body_warm);
                body_color = mix(body_color, color::star::COOL, body_cool);
                fb[idx] = blend_add_lin(fb[idx], body_color, body);
            }
        }
    }
}

fn paint_sparks(fb: &mut [u32], w: u32, h: u32, scene: &Scene) {
    for s in &scene.sparks {
        if s.life <= 0.0 {
            continue;
        }
        let k = (s.life / s.max_life).clamp(0.0, 1.0);
        let (cx, cy, r) = (s.x as i32, s.y as i32, 1 + (k * 2.5) as i32);
        for oy in -r..=r {
            for ox in -r..=r {
                let (xx, yy) = (cx + ox, cy + oy);
                if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                    continue;
                }
                let a = (1.0 - ((ox * ox + oy * oy) as f32).sqrt() / r as f32).max(0.0) * k * 0.7;
                if a > 0.003 {
                    let idx = (yy as u32 * w + xx as u32) as usize;
                    fb[idx] = blend_add_lin(fb[idx], s.hue, a);
                }
            }
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
mod tests;
