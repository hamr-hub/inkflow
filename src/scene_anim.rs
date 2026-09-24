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
use crate::llm_loop;
use crate::mood::FrameMood;
use crate::poetry::PoetryCursor;
use crate::scene::{lcg, Glyph, Particle, Scene};
use std::sync::{Arc, Mutex};

/// Per-voice base glyph sizes. Each of the five curatorial voices
/// (婉约 / 豪放 / 禅寂 / 稚拙 / 苍茫) carries its own typographic
/// weight — see `voice_base_size`. A 禅寂 glyph should look smaller
/// than a 豪放 glyph at the same energy: it reads as 'an austere
/// trace', not as 'a shouted line'. The voice-specific sizes
/// replaced the legacy single-size constants — each curatorial voice
/// is now a real voice, including its type design.
///
/// The picker drives this — see `crate::net_ollama::style_for`.
pub(crate) fn voice_base_size(voice: &str) -> f32 {
    match voice {
        "婉约" => 26.0, // lyrical, smaller
        "豪放" => 44.0, // bold, the largest
        "禅寂" => 20.0, // austere, the smallest
        "稚拙" => 28.0, // naive, slightly larger than 婉约
        "苍茫" => 36.0, // vast, mid-large
        _ => 30.0,      // unknown voice → mid default
    }
}

/// Per-voice hue offset (added at render time to the global hue,
/// then rem_euclid-ed into [0, 1)). Each curatorial voice carries
/// its own colour temperature so a viewer can identify the voice
/// from the palette alone — 禅寂 chars lean cool, 豪放 chars lean
/// warm, 苍茫 chars lean toward dusk violet. Combined with
/// voice_base_size this gives each voice a real typographic
/// identity, not just a name.
///
/// Values are bounded to ±0.20 because the renderer applies a row-
/// hue jitter of ±0.045 on top; anything bigger starts to wash out
/// the per-y variation that gives the stream its sense of motion.
pub(crate) fn voice_hue_offset(voice: &str) -> f32 {
    match voice {
        "婉约" => -0.05, // warm (sunset)
        "豪放" => -0.10, // deep warm
        "禅寂" => 0.10,  // cool
        "稚拙" => 0.05,  // spring warm
        "苍茫" => 0.15,  // dusk violet
        _ => 0.0,        // unknown voice → no offset
    }
}

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
    poetry: &mut PoetryCursor,
    frame: &FrameMood,
    touch: &Arc<Mutex<TouchState>>,
    shared: &Arc<Mutex<llm_loop::Shared>>,
    fb_w: u32,
    fb_h: u32,
    tick: u64,
    dt: f32,
    t: f32,
) {
    spawn_glyphs(scene, accum, poetry, frame, shared, fb_w, fb_h, tick, dt, t);
    spawn_particles(scene, frame, touch, fb_w, fb_h, tick);
}

/// Drain the LLM queue, top up with **classical poetry** when LLM is
/// quiet, push into the Scene at the per-frame spawn rate.
///
/// Text source priority:
///   1. live LLM char queue (when ollama is streaming)
///   2. curated Tang/Song phrase corpus, walked one char at a time
///      with a 2.4 s silence between phrases
///   3. (no third tier — the random-pool fallback was removed; the
///      piece no longer reads as random-character noise)
///
/// Spawn cadence:
///   - poetry cadence: ~1 char/sec when idle, climbing to ~1.7/s
///     when energy / contacts are high
///   - 1.7 chars/sec × 12 s life = ~20 chars in flight steady state
///   - phrase-to-phrase silence is honored by `is_breathing()` so the
///     viewer gets a deliberate breath between couplets
#[allow(clippy::too_many_arguments)]
fn spawn_glyphs(
    scene: &mut Scene,
    accum: &mut SpawnAccum,
    poetry: &mut PoetryCursor,
    frame: &FrameMood,
    shared: &Arc<Mutex<llm_loop::Shared>>,
    fb_w: u32,
    fb_h: u32,
    tick: u64,
    dt: f32,
    t: f32,
) {
    let energy = frame.effective_energy();
    // Meditative cadence — ~0.85 chars/sec at idle, climbing to ~1.6
    // when energy / contacts push. With ~10 s life that means
    // 8–16 glyphs in flight at any moment. Spread across the full
    // canvas (x_jitter 0.80·fb_w) so adjacent glyphs never
    // overlap on the horizontal axis. The poetry cursor still
    // inserts a 2.4 s silence between phrases so the piece breathes
    // rather than buzzing.
    accum.glyph_acc += dt * (0.85 + energy * 0.7);
    while accum.glyph_acc >= 1.0 && !poetry.is_breathing() {
        accum.glyph_acc -= 1.0;
        // Top stream falls slower so the breath reads as "ink rising +
        // ash falling", not two synchronized streams.
        let descend = (tick.wrapping_add(scene.glyphs.len() as u64) & 1) == 0;
        // Source priority: live LLM char → curated poetry line.
        // The old random 4-pool fallback is gone — the piece no
        // longer reads as random-character noise.
        let (from_llm, ch) = {
            let llm_char_str = llm_loop::pop_char(shared).map(crate::font::char_key);
            let ch = llm_char_str.unwrap_or_else(|| {
                // Walk the corpus one char at a time. When the line
                // is exhausted the cursor sets its own breath timer;
                // the surrounding `while !is_breathing()` makes sure
                // we don't emit during the silence.
                poetry.pop().unwrap_or("墨")
            });
            (llm_char_str.is_some(), ch)
        };
        let llm_ok = llm_loop::llm_ok(shared);
        // Voice drives the base size — each curatorial voice carries
        // its own typographic weight. When the LLM is down or the queue
        // is empty, we still know the voice from (warmth, energy), so
        // even offline glyphs read as the same curatorial voice.
        let voice = crate::net_ollama::style_for(frame.warmth, frame.effective_energy());
        let voice_size = voice_base_size(voice);
        let base_size = if from_llm {
            voice_size
        } else if llm_ok {
            // LLM backup: shrink slightly so the ambient feels
            // 'quieter' when ollama just hasn't replied yet.
            voice_size * 0.85
        } else {
            // LLM down: bump slightly above the LLM default so the
            // stream doesn't feel anaemic when fully offline.
            voice_size * 0.95
        };
        let speed_jitter = 0.82
            + lcg(tick
                .wrapping_add(131)
                .wrapping_add(scene.glyphs.len() as u64))
                * 0.36;
        let speed = (28.0 + energy * 60.0) * speed_jitter;
        let size_jitter = 0.88
            + lcg(tick
                .wrapping_add(113)
                .wrapping_add(scene.glyphs.len() as u64))
                * 0.24;
        // ~10-14 s on screen so each char has time to be read. The
        // phrase-to-phrase silence in the poetry cursor is the
        // dominant pacing — individual char life is just "long
        // enough to settle into a reading position".
        let max_life = 10.0 + energy * 4.0;
        let (spawn_y, vy) = if descend {
            (fb_h as f32 + 20.0, -speed)
        } else {
            (-20.0, speed * 0.55)
        };
        // Composition: each glyph picks its own x across the full
        // width (lcg-driven), with a soft pull toward the current
        // ink band. The previous "anchor + 30 % jitter" formula was
        // heavily right-biased because the band drifted to ~0.6 of
        // fb_w; widening to 0.80 of fb_w + light band pull keeps the
        // 留白 (negative space) Song-dynasty composition while
        // spreading glyphs across the full horizontal canvas.
        let current = ink_current_x(t);
        let anchor_x = current * fb_w as f32;
        let x_jitter = (lcg(tick.wrapping_add(11)) - 0.5) * fb_w as f32 * 0.80;
        let x = (anchor_x + x_jitter).clamp(2.0, fb_w as f32 - 2.0);
        // Hue offset comes from the same voice picker as the size —
        // a single source of truth for what 'this voice looks like'.
        // voice_hue_offset is cheap (a match on a &str).
        let voice_hue = voice_hue_offset(voice);
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
            hue_bias: voice_hue,
        });

        // Ink burst at the spawn point — 4 small motes in a
        // deterministic fan so each new char announces itself with
        // a tiny explosion rather than fading in silently. The
        // motes' short life means they dissolve before the
        // glyph settles, keeping the screen from feeling cluttered.
        let burst_n = 4u32;
        for k in 0..burst_n {
            let ku = k as u64;
            let ang = lcg(tick.wrapping_add(ku.wrapping_mul(47))) * core::f32::consts::TAU;
            let sp = 40.0 + 30.0 * lcg(tick.wrapping_add(ku.wrapping_mul(89)));
            scene.push_particle(Particle {
                x,
                y: spawn_y,
                vx: ang.cos() * sp,
                vy: ang.sin() * sp,
                life: 0.9,
                max_life: 1.4,
                r: 1.6 + lcg(tick.wrapping_add(ku.wrapping_mul(13))) * 1.4,
            });
        }
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
    fn voice_base_size_orders_match_artistic_intent() {
        // Per ARTIFACT.md: 禅寂 is austere (smallest), 豪放 is bold
        // (largest), 苍茫 is mid-large. 婉约 and 稚拙 both sit at
        // the small end but in opposite directions — 婉约 (lyrical,
        // refined) is slightly smaller than 稚拙 (naive, childlike,
        // a touch larger).
        assert!(voice_base_size("禅寂") < voice_base_size("婉约"));
        assert!(voice_base_size("婉约") < voice_base_size("稚拙"));
        assert!(voice_base_size("稚拙") < voice_base_size("苍茫"));
        assert!(voice_base_size("苍茫") < voice_base_size("豪放"));
    }

    #[test]
    fn voice_base_size_unknown_falls_back_safely() {
        // A future addition to VOICE_PARAMS (e.g. an unknown voice
        // name from a future LLM reply) shouldn't panic the spawn
        // loop. We fall back to the legacy LLM default.
        let v = voice_base_size("???");
        // The legacy default was 34 px — kept here so the spirit of
        // the original "sane mid-size default" survives.
        assert!((30.0..=40.0).contains(&v));
    }

    #[test]
    fn voice_base_size_range_is_within_render_bounds() {
        // All per-voice base sizes must stay in a band the renderer
        // can draw cleanly: too small and the glyph is unreadable,
        // too large and it clips the screen. The lowest current
        // value is 22 (禅寂) and the highest is 44 (豪放); with
        // size_jitter ~0.88-1.12 and an energy bump up to +16 px,
        // the rendered range is ~20-66 px.
        for v in ["婉约", "豪放", "禅寂", "稚拙", "苍茫"] {
            let s = voice_base_size(v);
            assert!((18.0..=50.0).contains(&s), "{v}={s} out of band");
        }
    }

    #[test]
    fn voice_hue_offset_distinguishes_warm_from_cool() {
        // Per ARTIFACT.md "禅寂 cool, 豪放 warm" — the hue offsets
        // must split cleanly into negative (toward warm/red) and
        // positive (toward cool/blue) buckets so the palette shift
        // is visible when the voice changes.
        assert!(voice_hue_offset("豪放") < 0.0, "豪放 must be warm");
        assert!(voice_hue_offset("婉约") < 0.0, "婉约 must be warm");
        assert!(voice_hue_offset("禅寂") > 0.0, "禅寂 must be cool");
        assert!(voice_hue_offset("稚拙") > 0.0, "稚拙 must lean warm-spring");
        assert!(voice_hue_offset("苍茫") > 0.0, "苍茫 must be cool/dusk");
    }

    #[test]
    fn voice_hue_offset_magnitude_is_bounded() {
        // The renderer adds a row-hue jitter of ±0.045 on top of
        // the bias. Anything bigger starts to wash out the per-y
        // variation that gives the stream motion. Pin the absolute
        // offset to ≤ 0.20.
        for v in ["婉约", "豪放", "禅寂", "稚拙", "苍茫"] {
            let h = voice_hue_offset(v);
            assert!(h.abs() <= 0.20, "{v} hue bias {h} too large");
        }
    }

    #[test]
    fn voice_hue_offset_unknown_falls_back_safely() {
        // Unknown voices must not panic; falling back to no offset
        // is the right conservative default (the glyph will then
        // use only the row_hue jitter).
        assert_eq!(voice_hue_offset("???"), 0.0);
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
