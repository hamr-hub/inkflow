//! Scene rendering — the "poetic phrase on a beat" presentation.
//!
//! Responsibilities:
//!   * Paint a restrained, beautiful background — vertical gradient + soft
//!     nebula glow + vignette + a thin layer of dust motes. No wall of glyphs.
//!   * Pick a composition (center / rule-of-thirds) and lay the hero phrase
//!     there. Optionally a faint echo behind/under it.
//!   * Animate the phrase through Entrance / Hold / Exit with crisp high-
//!     contrast AA. Optionally per-char "type-on" reveal so each character
//!     arrives cleanly.
//!   * Subtle pulse / glow on every beat; on tap, an extra particle burst.
//!
//! IMPORTANT: this is a single-phrase-at-a-time art piece. The hero phrase
//! must dominate the composition. The (optional) echo is purely a faint
//! afterimage — never a duplicate card.

use crate::color::{self, blend_add_lin, blend_screen, rgb};
use crate::glyph;
use crate::phrase::{Mood, Phrase};
use crate::rhythm::{Beat, Phase};

/// Tiny deterministic LCG so the dust looks "natural" but stable across runs.
pub struct Lcg(u64);
impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(
            seed.wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407),
        )
    }
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    #[inline]
    pub fn unit(&mut self) -> f32 {
        (self.next() & 0xFFFFFF) as f32 / 16_777_216.0
    }
}

/// One dust mote — drifts very slowly with a faint sinusoidal sway.
#[derive(Clone, Copy)]
pub struct Dust {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub a: f32,
    pub phase: f32,
    pub speed: f32,
    pub hue: u32,
}

/// Touch burst particle — same primitive, much faster decay, brighter.
#[derive(Clone, Copy)]
pub struct Spark {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub life: f32,
    pub max_life: f32,
    pub hue: u32,
}

/// Whole scene state (no per-frame allocation: fixed-size arrays).
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub rng: Lcg,
    pub dust: Vec<Dust>,
    pub sparks: Vec<Spark>,
    /// Stable echo phrase — chosen lazily, kept faint.
    pub echo: Option<Echo>,
    /// Phase for the soft nebula / vignette center.
    pub nebula_phase: f32,
    /// 0..=1, used to bias palette warmth.
    pub warmth: f32,
    /// Persistent low-frequency pulse — softens into "atmosphere breathing".
    pub ambient_pulse: f32,
}

pub struct Echo {
    pub phrase: &'static Phrase,
    pub alpha: f32,
    pub dx: f32,
    pub dy: f32,
    pub scale: f32,
}

impl Scene {
    pub fn new(width: u32, height: u32) -> Self {
        let mut rng = Lcg::new(0x00C0_FFEE_BEEF);
        let mut dust = Vec::with_capacity(48);
        for _ in 0..48 {
            dust.push(Dust {
                x: rng.unit() * width as f32,
                y: rng.unit() * height as f32,
                r: 0.6 + rng.unit() * 1.8,
                a: 0.05 + rng.unit() * 0.18,
                phase: rng.unit() * core::f32::consts::TAU,
                speed: 0.04 + rng.unit() * 0.12,
                hue: if rng.unit() < 0.5 {
                    color::star::WARM
                } else {
                    color::star::COOL
                },
            });
        }
        let sparks = Vec::with_capacity(64);
        Self {
            width,
            height,
            rng,
            dust,
            sparks,
            echo: None,
            nebula_phase: 0.0,
            warmth: 0.0,
            ambient_pulse: 0.0,
        }
    }

    /// Trigger a touch particle burst at pixel position (px, py) with intensity 0..=1.
    pub fn touch(&mut self, px: f32, py: f32, intensity: f32) {
        let n = (12.0 + intensity * 28.0) as usize;
        let warm = self.warmth > 0.05;
        for _ in 0..n {
            let ang = self.rng.unit() * core::f32::consts::TAU;
            let sp = 40.0 + self.rng.unit() * 220.0 * (0.5 + intensity);
            let hue = if warm && self.rng.unit() < 0.7 {
                color::drop::AMBER_HI
            } else if warm {
                color::drop::AMBER
            } else if self.rng.unit() < 0.7 {
                color::drop::CYAN_HI
            } else {
                color::drop::CYAN
            };
            let life = 0.45 + self.rng.unit() * 0.55;
            self.sparks.push(Spark {
                x: px,
                y: py,
                vx: ang.cos() * sp,
                vy: ang.sin() * sp,
                life,
                max_life: life,
                hue,
            });
        }
        if self.sparks.len() > 64 {
            let extra = self.sparks.len() - 64;
            self.sparks.drain(..extra);
        }
    }

    /// Step the simulation forward.
    pub fn step(&mut self, dt: f32, warmth: f32, pulse: f32) {
        self.warmth = warmth;
        self.nebula_phase += dt * 0.06;
        self.ambient_pulse += dt * 0.4;
        let (w, h) = (self.width as f32, self.height as f32);
        for d in self.dust.iter_mut() {
            d.phase += dt * d.speed;
            // gentle sway + tiny drift
            let sway_x = d.phase.cos() * 0.4;
            let sway_y = d.phase.sin() * 0.3;
            d.x += sway_x * dt * 6.0;
            d.y += sway_y * dt * 4.0 - dt * 0.5; // slow downward drift
                                                 // wrap
            if d.x < -8.0 {
                d.x += w + 16.0;
            }
            if d.x > w + 8.0 {
                d.x -= w + 16.0;
            }
            if d.y < -8.0 {
                d.y += h + 16.0;
            }
            if d.y > h + 8.0 {
                d.y -= h + 16.0;
            }
            // pulse boosts brightness briefly
            d.a *= 1.0 + pulse * 0.2;
            d.a = d.a.clamp(0.0, 0.5);
        }
        // sparks
        let drag = (0.85_f32).powf(dt * 60.0);
        for s in self.sparks.iter_mut() {
            s.life -= dt;
            s.x += s.vx * dt;
            s.y += s.vy * dt;
            s.vx *= drag;
            s.vy *= drag;
            s.vy += dt * 30.0; // light gravity
        }
        self.sparks.retain(|s| s.life > 0.0);
    }
}

// ============================================================
// Background paint
// ============================================================

/// Paint the background layer (gradient + nebula + vignette + dust) into the
/// framebuffer. This is the SAME routine used by the live renderer and the
/// headless harness — no separate "preview" path.
pub fn paint_background(fb: &mut [u32], w: u32, h: u32, scene: &Scene, pulse: f32, warmth: f32) {
    let w_f = w as f32;
    let h_f = h as f32;

    // Pick palette center based on warmth.
    let nebula_top = mix(color::bg::SKY, rgb(20, 16, 22), warmth * 0.4);
    let nebula_mid = mix(color::bg::MID, rgb(48, 30, 36), warmth * 0.5);
    let nebula_bot = mix(color::bg::HORIZON, rgb(110, 70, 50), warmth * 0.6);

    // Slow nebula center — drifts very gently.
    let cx = w_f * 0.5 + (scene.nebula_phase.sin()) * 40.0;
    let cy = h_f * (0.55 + 0.04 * scene.nebula_phase.cos());
    let max_r2 = (w_f * w_f + h_f * h_f) * 0.25;

    // Atmospheric breathing — adds a faint global luminance wave.
    let ambient = 0.02 * (scene.ambient_pulse.sin()) + pulse * 0.06;

    for y in 0..h {
        let v = y as f32 / (h_f - 1.0).max(1.0);
        let base = color::grad3(nebula_top, nebula_mid, nebula_bot, v);
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d2 = (dx * dx + dy * dy) / max_r2;
            // soft nebula glow (radial), tinted slightly warmer on pulse.
            let nebula = (1.0 - d2).clamp(0.0, 1.0).powf(2.0);
            let glow_color = mix(rgb(60, 50, 70), rgb(140, 96, 72), warmth);
            let p = blend_screen(base, glow_color, nebula * (0.18 + pulse * 0.10));
            // vignette darken corners
            let vx = (x as f32 / w_f - 0.5).abs() * 2.0;
            let vy = (y as f32 / h_f - 0.5).abs() * 2.0;
            let vig = (vx * vx + vy * vy).powf(0.7);
            let vig_dark = (vig * 0.65).clamp(0.0, 0.78);
            let p = mix(p, color::bg::DEEP, vig_dark);
            // ambient luminance wave
            let p = blend_add_lin(p, color::star::WARM, ambient * (1.0 - vig_dark * 0.6));
            fb[(y * w + x) as usize] = p;
        }
    }

    // dust layer
    for d in &scene.dust {
        let cx = d.x as i32;
        let cy = d.y as i32;
        let r = d.r as i32 + 1;
        for oy in -r..=r {
            for ox in -r..=r {
                let xx = cx + ox;
                let yy = cy + oy;
                if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                    continue;
                }
                let dist2 = (ox * ox + oy * oy) as f32;
                let rr = (r * r) as f32;
                let a = (1.0 - dist2 / rr).max(0.0) * d.a;
                if a > 0.003 {
                    let idx = (yy as u32 * w + xx as u32) as usize;
                    fb[idx] = blend_add_lin(fb[idx], d.hue, a);
                }
            }
        }
    }

    // sparks
    for s in &scene.sparks {
        if s.life <= 0.0 {
            continue;
        }
        let k = (s.life / s.max_life).clamp(0.0, 1.0);
        let cx = s.x as i32;
        let cy = s.y as i32;
        let r = 1 + (k * 2.5) as i32;
        for oy in -r..=r {
            for ox in -r..=r {
                let xx = cx + ox;
                let yy = cy + oy;
                if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                    continue;
                }
                let d = ((ox * ox + oy * oy) as f32).sqrt();
                let a = (1.0 - d / r as f32).max(0.0) * k * 0.7;
                if a > 0.003 {
                    let idx = (yy as u32 * w + xx as u32) as usize;
                    fb[idx] = blend_add_lin(fb[idx], s.hue, a);
                }
            }
        }
    }
}

// ============================================================
// Phrase paint
// ============================================================

/// Choose a placement rect for the hero phrase based on screen size.
/// Returns (x_baseline, y_baseline, scale_q8).
///
/// `x_baseline` is the pen position of the first character's pen.
/// `y_baseline` is the vertical position of the baseline (glyph.bottom_of_body).
/// `scale_q8` is in Q8 fixed point: 256 = render at the hero bucket's native em.
fn place_phrase(w: u32, h: u32, char_count: usize, beat_index: u64) -> (i32, i32, u32) {
    // Use the actual hero em size from the font table so 1.0 == native.
    let hero_em = glyph::HERO_EM_PX as f32;
    // Aim for the phrase to occupy ~80% of the screen width.  Use the
    // average glyph advance to estimate width: most CJK glyphs are roughly
    // 1.0 em wide; with light tracking (~+6%) the phrase fits comfortably.
    //
    // We clamp the upper bound to `hero_em` so we never upscale beyond the
    // hero bucket's native em — the renderer always picks a bucket whose
    // native em ≤ target and area-samples down.  Scaling *up* would soften
    // crisp 128-px ink.
    let target_px = {
        let n = char_count as f32;
        let ideal_total_w = (w as f32) * 0.82;
        // target per glyph *width * n <= ideal_total_w → target_per_glyph.
        (ideal_total_w / (n * 1.06)).clamp(48.0, hero_em)
    };
    let scale_q8 = ((target_px / hero_em) * 256.0).round() as u32;
    // Estimate total width using average advance (most CJK are ~1.0 em).
    let est_advance = (target_px * 1.06).round() as i32;
    let total_w = est_advance * (char_count as i32 - 1) + (target_px as i32);
    // Center horizontally with breathing margins.
    let pen_x = ((w as i32 - total_w) / 2)
        .max(16)
        .min(w as i32 - total_w - 16)
        .max(0);
    // Center vertically — leave the baseline a touch above the geometric
    // centre for optical balance.  A tiny per-beat alternation gives the
    // composition subtle breath (1-pixel-ish).
    let y_off = (beat_index as i32 % 4) - 2;
    let baseline_y = (h as i32) / 2 + y_off + (target_px as i32 * 5 / 100); // a touch below centre
    (pen_x, baseline_y, scale_q8)
}

/// Compose the hero phrase into `fb`. Returns the bbox used, in case the
/// caller wants to highlight it or compose echo relative to it.
pub fn paint_phrase(
    fb: &mut [u32],
    w: u32,
    h: u32,
    beat: &Beat,
    warmth: f32,
    pulse: f32,
    mood: Mood,
) {
    let text = beat.phrase.text;
    // collect char iter
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return;
    }
    let (pen_x0, baseline_y0, scale_q8) = place_phrase(w, h, n, beat.index);

    // Compute per-char opacity / scale progress for entrance.
    // entrance_progress 0..1
    let ep = match beat.phase {
        Phase::Entrance => beat.entrance_progress(),
        Phase::Hold => 1.0,
        Phase::Exit => 1.0 - beat.exit_progress(),
        Phase::Rest => 0.0,
    };
    // Phase-specific easing.
    let ep_eased = color::smootherstep(ep);

    // Per-character reveal: each char finishes its own entrance slightly after
    // the previous — produces a clean left→right type-on feel that resolves
    // into a static hold.
    let total_chars_delay = 0.40_f32; // fraction of entrance reserved for stagger
    let per_char_window = (1.0 - total_chars_delay) / (n as f32).max(1.0);

    // Phase-aware motion: hero drifts up slightly on entrance (settle in),
    // drifts down on exit (fade out). Reads as a confident, deliberate phrase.
    let slide_y_px = match beat.phase {
        Phase::Entrance => ((1.0 - ep) * 24.0) as i32, // 24px down → 0
        Phase::Exit => (ep * 18.0) as i32,             // 0 → 18px down
        _ => 0,
    };

    // Hero colour: clean cream — slight warmth blend. NO drop shadow —
    // shadows were reading as "card panels". Glow halo instead.
    let base_color = mix(color::ink::CREAM, color::ink::WARM, warmth * 0.5);
    let glow_color = mix(color::ink::GLOW, color::ink::WARM, warmth * 0.6);

    // outer glow alpha tied to phase + pulse + phrase.glow + warmth
    let beat_glow = beat.phrase.glow;
    let glow_alpha = (0.10 + 0.18 * pulse + 0.06 * warmth + beat_glow * 0.10).clamp(0.0, 0.55);

    // pulse: subtle global brightness/scale overshoot on the entrance
    let overshoot = if matches!(beat.phase, Phase::Entrance) {
        let p = beat.entrance_progress();
        if p < 0.7 {
            1.0
        } else {
            let k = (p - 0.7) / 0.3;
            1.0 + 0.06 * (1.0 - k) * (k * core::f32::consts::TAU).sin()
        }
    } else if matches!(beat.phase, Phase::Hold) {
        1.0 + 0.015 * pulse * (beat.t_in_beat * 1.7).sin()
    } else {
        1.0
    };

    let scale_q8 = ((scale_q8 as f32) * overshoot).round() as u32;

    for (i, &ch) in chars.iter().enumerate() {
        // stagger
        let stagger = total_chars_delay * (i as f32) / (n as f32).max(1.0);
        let local = ((ep - stagger) / per_char_window).clamp(0.0, 1.0);
        let local_eased = color::smootherstep(local);

        // per-char alpha: ramps 0..1, but also affected by overall ep so even
        // the first char looks intentional.
        let char_alpha = (local_eased * ep_eased).clamp(0.0, 1.0);

        if char_alpha <= 0.005 {
            continue;
        }

        let glyph_idx = glyph::index_for(ch as u32);
        let slot = glyph_idx as usize;
        // Per-char advance: use the glyph's own advance in Q8 fixed-point pixels.
        let advance_q8 = glyph::HERO_TABLE[slot].advance as i32 * (scale_q8 as i32);
        // pen_x0 is in pixels; advance_q8 is in Q8.  Convert pen_x0 to Q8
        // before adding so the arithmetic is homogeneous.
        let pen_x_q8 = pen_x0 * 256 + advance_q8 * (i as i32);
        let baseline_y = baseline_y0 + slide_y_px;

        // small per-char pop overshoot (front-loaded then settles)
        let micro = 1.0 + 0.06 * (1.0 - local) * (local * core::f32::consts::TAU).sin();
        let char_scale = ((scale_q8 as f32) * micro) as u32;
        // Q8 fixed point sub-pixel positioning.  Pen position becomes the
        // (fx, fy) baseline passed to draw_glyph.
        let fx = pen_x_q8;
        let fy = baseline_y * 256;

        // 1) Soft outer halo (only visible at high char_alpha).
        let glow_alpha_local = glow_alpha * char_alpha;
        if glow_alpha_local > 0.01 {
            glyph::draw_glyph(
                fb,
                w as usize,
                h as usize,
                glyph_idx,
                glow_color,
                glow_color,
                fx,
                fy,
                char_scale,
                glow_alpha_local * 0.45,
            );
        }

        // 2) Main glyph with crisp blend — fully opaque cream, no shadow.
        glyph::draw_glyph(
            fb, w as usize, h as usize, glyph_idx, base_color, glow_color, fx, fy, char_scale,
            char_alpha,
        );

        // suppress unused warning
        let _ = mood;
    }

    // Phase-tail dim for the whole phrase in Exit — a soft fade into the
    // background, NOT a rectangle panel.
    if matches!(beat.phase, Phase::Exit) {
        // The per-char alpha already handles the visible fade; the only
        // additional thing we want is a very faint glow lift right as the
        // phrase is leaving, then it fades out with the chars.
    }
}

/// Paint a faint echo phrase — large, very low opacity, offset well
/// BELOW the hero so it reads as a settled afterimage, not a duplicate card.
pub fn paint_echo(fb: &mut [u32], w: u32, h: u32, echo: &Echo, pulse: f32) {
    let chars: Vec<char> = echo.phrase.text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return;
    }
    let (mut pen_x0, mut baseline_y0, mut scale_q8) = place_phrase(w, h, n, 0);
    // Echo sits clearly *below* the hero — large vertical offset, faint scale.
    baseline_y0 += echo.dy as i32 + (w as i32) / 14; // ~90px down at 1280
    pen_x0 += echo.dx as i32;
    scale_q8 = ((scale_q8 as f32) * echo.scale) as u32;
    let alpha = (echo.alpha * (0.12 + pulse * 0.08)).clamp(0.0, 0.22);
    if alpha < 0.01 {
        return;
    }
    // Echo is dim and monochrome — a gentle shadow-toned hue.
    let color = mix(color::ink::SHADOW, color::bg::MID, 0.6);
    let glow = color::bg::MID;
    for (i, &ch) in chars.iter().enumerate() {
        let glyph_idx = glyph::index_for(ch as u32);
        let slot = glyph_idx as usize;
        let advance_q8 = glyph::HERO_TABLE[slot].advance as i32 * (scale_q8 as i32);
        let pen_x_q8 = pen_x0 * 256 + advance_q8 * (i as i32);
        let fx = pen_x_q8;
        let fy = baseline_y0 * 256;
        glyph::draw_glyph(
            fb, w as usize, h as usize, glyph_idx, color, glow, fx, fy, scale_q8, alpha,
        );
    }
}

#[inline]
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ar = color::r(a) as f32;
    let ag = color::g(a) as f32;
    let ab = color::b(a) as f32;
    let br = color::r(b) as f32;
    let bg = color::g(b) as f32;
    let bb = color::b(b) as f32;
    rgb(
        (ar + (br - ar) * t) as u8,
        (ag + (bg - ag) * t) as u8,
        (ab + (bb - ab) * t) as u8,
    )
}
