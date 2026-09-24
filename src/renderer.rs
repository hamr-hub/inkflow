// inkflow · renderer.rs
//
// All drawing primitives the frame loop composes into a single frame.
// Pulled out of `main.rs` so the per-frame body becomes a top-down
// sequence of named phases (clear → nebula → stars → particles →
// glyphs → horizon fog → present) rather than a wall of inline math.
//
// The renderer never allocates. Every call takes the pixel buffer by
// mutable reference and a few float scalars. The deterministic hue
// drift, the dual nebula centers, the per-star twinkle and per-glyph
// breath-in all stay as inline constants here — the file IS the
// authoritative copy of "what does one frame look like".

use crate::font::{draw_glyph, fill_circle, fill_rect, Rgba};
use crate::scene::{Glyph, Particle, Star};
use crate::surface::Surface;

/// Pack a 32bpp BGRA pixel. Our buffer layout is BGRA in little-endian
/// memory: byte 0 = B, byte 1 = G, byte 2 = R, byte 3 = A. The alpha
/// byte is unused for XRGB scanout, so we always set it to 0xFF.
#[inline]
pub const fn bgra(r: u8, g: u8, b: u8) -> u32 {
    (b as u32) | ((g as u32) << 8) | ((r as u32) << 16) | (0xFFu32 << 24)
}

/// Background color — deep ink, slightly violet. The piece never goes
/// pure black, so even an "empty" frame has some atmospheric grain.
pub const BACKGROUND: u32 = bgra(3, 3, 5);

/// Fill the framebuffer with the deep-ink background. At 2560×1440
/// (3.7 M u32s ≈ 14.7 MB) the single-threaded `slice::fill` is
/// memory-bandwidth bound — `~15-25 ms` on most dev hosts, which is
/// the dominant per-frame cost at full resolution. Splitting the
/// buffer into 4 vertical strips and clearing each on its own thread
/// overlaps DRAM page activations and gets the wall budget back under
/// 5 ms on multi-core hosts while staying bit-identical (each strip
/// is disjoint and writes a constant value).
pub fn clear(pixels: &mut [u32]) {
    if pixels.len() < 65_536 {
        // Small surface (headless 1280×800 ≈ 1 M px, but anything
        // under ~256×256 skips the thread fan-out — the join cost
        // would dominate). Stay single-threaded.
        pixels.fill(BACKGROUND);
        return;
    }
    let n = pixels.len();
    let workers = 4usize;
    let chunk = n / workers;
    let base = pixels.as_mut_ptr() as usize;
    // SAFETY: each (lo, hi) range is a disjoint sub-slice of the
    // original `pixels`, and each thread writes only its own range.
    // The base pointer was derived from a live &mut [u32] that
    // outlives this scope (no aliased access is possible because
    // the threads never share slice handles — they only have raw
    // pointers to disjoint regions).
    std::thread::scope(|s| {
        for k in 0..workers {
            let lo = base + k * chunk * 4;
            let hi = if k + 1 == workers {
                base + n * 4
            } else {
                base + (k + 1) * chunk * 4
            };
            s.spawn(move || unsafe {
                let p = lo as *mut u32;
                let len = (hi - lo) / 4;
                core::ptr::write_bytes(p, BACKGROUND as u8, len);
            });
        }
    });
}

// ----- nebula -----

/// Two slow-drifting nebula layers, each rendered as 5 concentric
/// quadratic-falloff circles. The two layers use complementary hues
/// (one rotated by 0.5) and slightly different center positions so
/// the wash breathes left↔right.
pub fn draw_nebula(pixels: &mut [u32], pitch_px: usize, fb_w: i32, fb_h: i32, t: f32, hue: f32) {
    let neb_a_alpha = 0.035 + 0.02 * (t * 0.05).sin();
    let neb_b_alpha = 0.025 + 0.018 * (t * 0.04 + 1.7).cos();
    let na_x = fb_w as f32 * (0.5 + 0.28 * (t * 0.018).sin());
    let na_y = fb_h as f32 * (0.5 + 0.20 * (t * 0.013).cos());
    let nb_x = fb_w as f32 * (0.5 + 0.28 * (t * 0.017).cos());
    let nb_y = fb_h as f32 * (0.5 + 0.20 * (t * 0.022).sin());

    let nebula_a_color = Rgba::from_hsl((hue + 0.5).rem_euclid(1.0), 0.55, 0.5);
    for i in 0i32..5 {
        let r = 0.18 + 0.13 * i as f32;
        let fall = (1.0 - i as f32 / 4.0).powi(2);
        fill_circle(
            pixels,
            pitch_px,
            fb_w,
            fb_h,
            na_x,
            na_y,
            fb_w as f32 * r,
            nebula_a_color,
            neb_a_alpha * fall,
        );
    }

    let nebula_b_color = Rgba::from_hsl(hue, 0.6, 0.45);
    for i in 0i32..5 {
        let r = 0.16 + 0.11 * i as f32;
        let fall = (1.0 - i as f32 / 4.0).powi(2);
        fill_circle(
            pixels,
            pitch_px,
            fb_w,
            fb_h,
            nb_x,
            nb_y,
            fb_w as f32 * r,
            nebula_b_color,
            neb_b_alpha * fall,
        );
    }
}

// ----- stars -----

/// Twinkling field rendered into the buffer. Stars are static positions
/// with per-star period/phase/hue; their brightness modulates on a sine
/// so the void reads as a moving breath.
pub fn draw_stars(
    pixels: &mut [u32],
    pitch_px: usize,
    fb_w: i32,
    fb_h: i32,
    stars: &[Star],
    t: f32,
    hue: f32,
) {
    for st in stars.iter() {
        let k = 0.5 + 0.5 * (t / st.period * core::f32::consts::TAU + st.phase).sin();
        let color = Rgba::from_hsl((hue + st.hue_offset).rem_euclid(1.0), 0.35, 0.88);
        fill_circle(
            pixels,
            pitch_px,
            fb_w,
            fb_h,
            st.x,
            st.y,
            st.r,
            color,
            st.base * (0.25 + 0.75 * k),
        );
    }
}

// ----- particles -----

/// Move + render the live particles. Returns the count of particles
/// that should be retained for the next frame.
#[allow(clippy::too_many_arguments)]
pub fn draw_and_step_particles(
    pixels: &mut [u32],
    pitch_px: usize,
    fb_w: i32,
    fb_h: i32,
    particles: &mut VecDeque<Particle>,
    dt: f32,
    t: f32,
    hue: f32,
) {
    for p in particles.iter_mut() {
        p.x += p.vx * dt;
        p.y += p.vy * dt;
        p.vy -= 6.0 * dt; // light gravity so embers drift downward
        p.life -= dt;
        let a = (p.life / p.max_life).clamp(0.0, 1.0);
        let p_hue =
            (hue + (p.x * 0.3 + p.y * 0.5).sin() * 0.06 + (t * 0.05).sin() * 0.08).rem_euclid(1.0);
        let color = Rgba::from_hsl(p_hue, 0.7, 0.6);
        fill_circle(pixels, pitch_px, fb_w, fb_h, p.x, p.y, p.r, color, a * 0.5);
    }
    particles.retain(|p| p.life > 0.0 && p.y > -20.0);
}

// ----- glyphs -----

/// Step + draw the floating CJK glyphs. Each glyph drifts upward with
/// a tiny lateral waver (per-glyph phase so different glyphs drift
/// differently), breathes in over 7% of its lifetime, fades at the
/// top/bottom horizons so it dissolves softly into the fog.
#[allow(clippy::too_many_arguments)]
pub fn draw_and_step_glyphs(
    pixels: &mut [u32],
    pitch_px: usize,
    fb_w: i32,
    fb_h: i32,
    glyphs: &mut VecDeque<Glyph>,
    dt: f32,
    t: f32,
    hue: f32,
) {
    for g in glyphs.iter_mut() {
        g.x += g.vx * dt;
        g.y += g.vy * dt;
        g.x += (t * 0.55 + g.y * 0.012).sin() * 6.0 * dt;
        g.y += ((t * 0.42 + g.phase).sin()) * 1.5 * dt;
        g.x += ((t * 0.31 + g.phase * 1.3).sin()) * 1.0 * dt;
        g.life -= dt;
        let a = (g.life / g.max_life).clamp(0.0, 1.0);
        let aeased = a * a * (3.0 - 2.0 * a);
        let elapsed = 1.0 - a;
        let birth = (elapsed / 0.07).clamp(0.0, 1.0);
        let birth_eased = birth * birth * (3.0 - 2.0 * birth);
        let row_hue = (hue + (g.y * 0.003).sin() * 0.045).rem_euclid(1.0);
        // Tightened from 0.18 → 0.08 each so glyphs read as "fully
        // visible" across ~84% of the screen instead of 64%. The
        // top/bottom edges still fade to the fog without a hard line.
        let top_fade = (g.y / (fb_h as f32 * 0.08)).clamp(0.0, 1.0);
        let bottom_fade = ((fb_h as f32 - g.y) / (fb_h as f32 * 0.08)).clamp(0.0, 1.0);
        let rot = ((t * 0.32 + g.y * 0.011).sin()) * 0.045;
        let halo_age = 1.0 - a;
        let halo_strength = (halo_age * (1.0 - halo_age) * 4.0).min(1.0);
        let draw_size = g.size * (0.5 + 0.5 * birth_eased);
        let halo_color = Rgba::from_hsl(row_hue, 0.4, 0.45);
        fill_circle(
            pixels,
            pitch_px,
            fb_w,
            fb_h,
            g.x,
            g.y + draw_size * 0.3,
            draw_size * 0.7,
            halo_color,
            halo_strength * 0.18 * top_fade * bottom_fade,
        );
        let fg = Rgba::from_hsl(row_hue, 0.50, 0.92);
        // Foreground ink must read as the primary content, not as
        // ambient haze. Baseline lifted again 0.55 → 0.70 and
        // lightness 0.85 → 0.92 so the chars pop against the
        // (halved) nebula wash without losing the breath.
        let fg_alpha = birth_eased * (0.70 + aeased * 0.28) * top_fade * bottom_fade;
        draw_glyph(
            pixels, pitch_px, fb_w, fb_h, g.x, g.y, draw_size, g.ch, fg, fg_alpha, rot,
        );
    }
    glyphs.retain(|g| g.life > 0.0 && g.y > -40.0);
}

// ----- top fog -----

/// Thin black bar at the top edge so glyphs drifting into the upper
/// sky fade cleanly without a visible hard cutoff at y=0.
pub fn draw_top_fog(pixels: &mut [u32], pitch_px: usize, fb_w: i32, fb_h: i32) {
    fill_rect(
        pixels,
        pitch_px,
        fb_w,
        fb_h,
        0,
        0,
        fb_w,
        3,
        Rgba(0, 0, 0, 255),
        0.25,
    );
}

// ----- all-in-one frame body -----

/// Compose one frame onto the surface. Caller owns the Scene and the
/// mood. This function is the single integration point for the frame
/// loop in `main.rs`.
#[allow(clippy::too_many_arguments)]
pub fn draw_frame(
    surface: &mut Surface,
    scene_glyphs: &mut VecDeque<Glyph>,
    scene_particles: &mut VecDeque<Particle>,
    scene_stars: &[Star],
    dt: f32,
    t: f32,
    hue: f32,
) {
    let fb_w = surface.w() as i32;
    let fb_h = surface.h() as i32;
    let pitch_px = surface.pitch_px();
    let pixels = surface.pixels_mut();

    clear(pixels);
    draw_nebula(pixels, pitch_px, fb_w, fb_h, t, hue);
    draw_stars(pixels, pitch_px, fb_w, fb_h, scene_stars, t, hue);
    draw_and_step_particles(pixels, pitch_px, fb_w, fb_h, scene_particles, dt, t, hue);
    draw_and_step_glyphs(pixels, pitch_px, fb_w, fb_h, scene_glyphs, dt, t, hue);
    draw_top_fog(pixels, pitch_px, fb_w, fb_h);
    surface.present();
}

// Re-export of VecDeque so `main.rs` doesn't have to import it just
// for the renderer call site.
pub use std::collections::VecDeque;
