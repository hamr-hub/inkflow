// inkflow · scene_anim.rs
//
// Per-frame spawn logic — translating touch + mood + LLM char queue
// into a constant-rate stream of glyphs and touch-derived particles.
// Pure of any rendering: only mutates the `Scene`.
//
// Spawn rates:
//   - Glyphs: `dt * (3.0 + energy * 11.0)` characters per frame, so
//     at idle the stream produces ~3 chars/s and at high energy ~14/s.
//   - Touch particles: per contact, 1-3 particles per frame at speed
//     proportional to touch velocity. Touch birth-pressure puts the
//     spawn at the exact contact point so each finger paints a tail.
//   - Drift particles: emitted when the particle set is below 90
//     entries, every 8th frame, from below the screen — these are the
//     background embers.
//
// All spawn sizes / speeds / hues are derived from the deterministic
// LCG so the same `(tick, scene length)` tuple always produces the
// same glyph — useful for reproducing screenshots and verifying
// telemetry.

use crate::evdev::TouchState;
use crate::fallback;
use crate::llm_loop;
use crate::mood::FrameMood;
use crate::scene::{lcg, Glyph, Particle, Scene};
use std::sync::{Arc, Mutex};

/// Glyph size when the character came from the LLM stream.
pub const LLM_SIZE: f32 = 34.0;
/// Glyph size when the LLM is healthy but the LLM queue happened to
/// be empty this spawn — the fallback glyph gets LLM-near sizing so
/// the ambient doesn't visibly shrink when ollama is paused.
pub const LLM_BACKUP_SIZE: f32 = 28.0;
/// Glyph size when the LLM is down (no characters for >4 s).
pub const FALLBACK_SIZE_BUMP: f32 = 32.0;

/// Spawn-rate accumulator. The frame loop accumulates `dt * base_rate`
/// and pops whole glyphs each time the accumulator crosses 1.0.
#[derive(Default)]
pub struct SpawnAccum {
    pub glyph_acc: f32,
}

/// The 'ink current' — a slowly drifting x-bias in (0, 1) that glyphs
/// cluster around. Sum of two sine waves with coprime-ish periods
/// (~785 s and ~465 s) so the current never re-aligns with the nebula
/// or moon drift; over a few minutes it sweeps the full width.
///
/// Glyphs spawn within ±15 % of fb_w of the current — the opposite
/// half of the screen reads as 留白 (intentional emptiness, the
/// single most important Song-dynasty composition rule).
///
/// Output is clamped to [0.05, 0.95] so the current never parks
/// the entire stream off one side of the canvas.
pub(crate) fn ink_current_x(t: f32) -> f32 {
    let a = (t * 0.008).sin() * 0.32;
    let b = (t * 0.0135 + 1.3).cos() * 0.12;
    (0.5 + a + b).clamp(0.05, 0.95)
}

/// Spawn glyphs + particles for one frame, given the current mood,
/// touch state, and shared LLM queue. Touch state is read under its
/// own mutex and not held across the spawn.
#[allow(clippy::too_many_arguments)]
pub fn spawn_for_frame(
    scene: &mut Scene,
    accum: &mut SpawnAccum,
    frame: &FrameMood,
    touch: &Arc<Mutex<TouchState>>,
    shared: &Arc<Mutex<llm_loop::Shared>>,
    fb_w: u32,
    fb_h: u32,
    tick: u64,
    dt: f32,
    t: f32,
) {
    spawn_glyphs(scene, accum, frame, shared, fb_w, fb_h, tick, dt, t);
    spawn_particles(scene, frame, touch, fb_w, fb_h, tick);
}

/// Drain the LLM queue, top up with fallback glyphs, push into the
/// Scene at the per-frame spawn rate.
///
/// Two interleaved streams (rising from the bottom, falling from the
/// top — picked by tick parity so they don't sync into a single pulse)
/// keep the screen reading as a deliberate layered piece instead of
/// a sparse demo. Combined spawn rate lands ~50–90 chars visible at
/// idle and climbs with energy + contact pressure.
#[allow(clippy::too_many_arguments)]
fn spawn_glyphs(
    scene: &mut Scene,
    accum: &mut SpawnAccum,
    frame: &FrameMood,
    shared: &Arc<Mutex<llm_loop::Shared>>,
    fb_w: u32,
    fb_h: u32,
    tick: u64,
    dt: f32,
    t: f32,
) {
    let energy = frame.effective_energy();
    // ~22 glyphs/sec at idle (energy=0.05), climbing to ~40 at high
    // energy. Slower drift speed (35-115 vs old 55-185 px/s) so
    // each char spends ~30 s in flight — every horizontal band
    // always has visible ink instead of wave-troughs.
    accum.glyph_acc += dt * (12.0 + energy * 28.0);
    while accum.glyph_acc >= 1.0 {
        accum.glyph_acc -= 1.0;
        // Top stream falls slower so the breath reads as "ink rising +
        // ash falling", not two synchronized streams.
        let descend = (tick.wrapping_add(scene.glyphs.len() as u64) & 1) == 0;
        // Pop one LLM-derived char under the lock; if none, fall
        // back to the static pool. The fallback string is
        // `&'static str` so it aliases into our static pools.
        let (from_llm, ch) = {
            let llm_char_str = llm_loop::pop_char(shared).map(crate::font::char_key);
            let ch = llm_char_str.unwrap_or_else(|| {
                fallback::local_glyph(
                    frame.warmth,
                    energy,
                    tick.wrapping_add(scene.glyphs.len() as u64),
                )
            });
            (llm_char_str.is_some(), ch)
        };
        let llm_ok = llm_loop::llm_ok(shared);
        let base_size = if from_llm {
            LLM_SIZE
        } else if llm_ok {
            LLM_BACKUP_SIZE
        } else {
            FALLBACK_SIZE_BUMP
        };
        let speed_jitter = 0.82
            + lcg(tick
                .wrapping_add(131)
                .wrapping_add(scene.glyphs.len() as u64))
                * 0.36;
        let speed = (35.0 + energy * 80.0) * speed_jitter;
        let size_jitter = 0.88
            + lcg(tick
                .wrapping_add(113)
                .wrapping_add(scene.glyphs.len() as u64))
                * 0.24;
        let max_life = (fb_h as f32 + 40.0) / speed + 1.5;
        let (spawn_y, vy) = if descend {
            (fb_h as f32 + 20.0, -speed)
        } else {
            (-20.0, speed * 0.55)
        };
        // Ink current — glyphs cluster around ink_current_x(t) rather
        // than spawning uniformly across fb_w. ±15 % of fb_w jitter
        // gives the stream a deliberate rhythm: dense in one region,
        // empty in the other. Per ARTIFACT.md 'Composition asymmetry'
        // + 'Song-dynasty 留白'.
        let current = ink_current_x(t);
        let anchor_x = current * fb_w as f32;
        let x_jitter = (lcg(tick.wrapping_add(11)) - 0.5) * fb_w as f32 * 0.30;
        let x = (anchor_x + x_jitter).clamp(2.0, fb_w as f32 - 2.0);
        scene.push_glyph(Glyph {
            ch,
            x,
            y: spawn_y,
            vx: (lcg(tick.wrapping_add(5)) - 0.5) * (10.0 + energy * 40.0),
            vy,
            life: max_life,
            max_life,
            size: (base_size + energy * 16.0) * size_jitter,
            phase: lcg(tick
                .wrapping_add(197)
                .wrapping_add(scene.glyphs.len() as u64))
                * core::f32::consts::TAU,
        });
    }
}

/// Spawn per-contact particles proportional to energy, plus a small
/// drift emission every 8th frame so the void has embers even at rest.
fn spawn_particles(
    scene: &mut Scene,
    frame: &FrameMood,
    touch: &Arc<Mutex<TouchState>>,
    fb_w: u32,
    fb_h: u32,
    tick: u64,
) {
    let energy = frame.effective_energy();

    // Touch-derived particles — one per contact, multiplied when the
    // user is actively pressing. The per-particle direction comes
    // from a deterministic angle so the same tick always emits the
    // same spray.
    let mut sources: Vec<(f32, f32)> = Vec::with_capacity(8);
    {
        let s = touch.lock().unwrap();
        for c in s.contacts.iter() {
            sources.push((c.x * fb_w as f32, c.y * fb_h as f32));
        }
        // Mouse-as-touch counts as an additional contact so a held
        // left button paints the same particle trail as a finger.
        if let Some(m) = s.mouse {
            sources.push((m.x * fb_w as f32, m.y * fb_h as f32));
        }
    }
    let n_per = if energy > 0.6 { 3 } else { 1 };
    for &(px, py) in &sources {
        for k in 0..n_per {
            let ang = lcg(tick.wrapping_add(k as u64 * 31)) * core::f32::consts::TAU;
            let sp = 20.0 + energy * 90.0;
            scene.push_particle(Particle {
                x: px,
                y: py,
                vx: ang.cos() * sp,
                vy: ang.sin() * sp - 20.0,
                life: 1.5 + lcg(tick.wrapping_add(99)) * 2.0,
                max_life: 3.5,
                r: 1.5 + lcg(tick.wrapping_add(77)) * 3.0,
            });
        }
    }

    // Ambient drift ember — fill the lower screen with a few slow
    // particles so the void never reads as completely empty.
    if scene.particles.len() < 90 && tick.is_multiple_of(8) {
        scene.push_particle(Particle {
            x: lcg(tick.wrapping_add(41)) * fb_w as f32,
            y: fb_h as f32 + 4.0,
            vx: (lcg(tick.wrapping_add(43)) - 0.5) * 12.0,
            vy: -8.0 - energy * 20.0,
            life: 4.0,
            max_life: 6.0,
            r: 1.0 + lcg(tick.wrapping_add(47)) * 2.0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_acc_starts_empty() {
        let a = SpawnAccum::default();
        assert_eq!(a.glyph_acc, 0.0);
    }

    #[test]
    fn size_constants_have_sane_order() {
        // The visual contract:
        //   LLM chars  >=  fallback chars (when LLM is up)
        //   fallback chars  >  backup chars (so the stream doesn't
        //                            visibly shrink when ollama drops)
        // The backup size is intentionally the smallest: it's used when
        // the LLM is healthy but the queue happened to be empty this
        // spawn, so we want it to read as a "quiet LLM glyph".
        assert!(LLM_SIZE > FALLBACK_SIZE_BUMP);
        assert!(FALLBACK_SIZE_BUMP > LLM_BACKUP_SIZE);
    }

    #[test]
    fn glyph_acc_is_default_zero() {
        // Just a sanity check that default() == 0.
        let a: SpawnAccum = Default::default();
        assert_eq!(a.glyph_acc, 0.0);
    }

    #[test]
    fn ink_current_stays_in_unit_interval() {
        // The current is clamped to [0.05, 0.95] regardless of t.
        for t in (0..10_000).map(|i| i as f32 * 0.1) {
            let c = ink_current_x(t);
            assert!((0.05..=0.95).contains(&c), "t={t} c={c}");
        }
    }

    #[test]
    fn ink_current_oscillates_within_range() {
        // The unclamped function covers roughly [0.06, 0.94] over
        // the full ~785 s dominant period + ~465 s modulation.
        // Confirm the unclamped max exceeds 0.85 and min drops below
        // 0.15 (i.e. the current actually moves, it doesn't park at
        // 0.5). Sample over 3600 s (one full "viewing session") with
        // 1 s resolution — that's 3600 evaluations, fast.
        let mut lo = 1.0_f32;
        let mut hi = 0.0_f32;
        for t in (0..3_600).map(|i| i as f32) {
            let c = ink_current_x(t);
            if c < lo {
                lo = c;
            }
            if c > hi {
                hi = c;
            }
        }
        assert!(hi > 0.85, "current should reach the right edge: hi={hi}");
        assert!(lo < 0.15, "current should reach the left edge: lo={lo}");
    }

    #[test]
    fn ink_current_is_deterministic() {
        // Same t always produces the same current — needed for the
        // (tick, scene length) → same glyph determinism contract.
        for t in [0.0_f32, 1.0, 13.7, 137.0, 1024.5] {
            assert_eq!(ink_current_x(t), ink_current_x(t), "t={t}");
        }
    }
}
