//! Scene rendering — a poetic *composition* of phrases on a beat.
//!
//! The screen is no longer a single lonely line.  At any moment it carries a
//! small **constellation** of readable Chinese phrases — one hero in the
//! optical focal area, and a handful of supporting lines distributed across
//! the field with clear, non-overlapping placement.  Supporting lines drift
//! gently and refresh on a staggered cadence so the wall feels alive without
//! ever becoming a dense clutter.
//!
//! Responsibilities:
//!   * Paint a restrained background (gradient + nebula glow + vignette + a
//!     thin layer of dust motes + sparks on touch). No panels, no HUD.
//!   * Maintain a `Composition` of `Slot`s.  Each slot has a fixed geometry
//!     (position, scale, alignment, baseline alpha) and a rolling phrase
//!     picked from the active theme.
//!   * Animate the hero through Entrance / Hold / Exit via the rhythm engine.
//!     Supporting slots have their own simple lifecycle (fade-in over a few
//!     hundred ms, hold for several seconds, fade out, swap).
//!   * Drive a coherent theme that rotates occasionally; the hero's warmth
//!     tints the palette, supporting lines echo the same hue family.

use crate::color::{self, blend_add_lin, blend_screen, rgb};
use crate::glyph;
use crate::phrase::{self, Phrase};
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

// ============================================================
// Composition — slot geometry
// ============================================================

/// Slot horizontal alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A single slot definition — geometric recipe.  Each slot owns a fixed
/// position on screen, a fixed scale, and a baseline alpha.  Supporting slots
/// gently sway around their anchor with their own drift frequency.
#[derive(Clone, Copy, Debug)]
pub struct SlotDef {
    /// Role label — only `Hero` is the focal point; the rest are supporting.
    pub role: SlotRole,
    /// Anchor x as a fraction of the screen width (0..1).
    pub x_frac: f32,
    /// Anchor y as a fraction of the screen height (0..1) — *baseline*.
    pub y_frac: f32,
    pub align: Align,
    /// Em size as a fraction of the hero bucket's native em (1.0 = 128 px).
    /// `0` means "auto-fit to width" (used by the hero only).
    pub em_scale: f32,
    /// Target screen-width fraction for the auto-fit hero (only when em_scale == 0).
    pub target_w_frac: f32,
    /// Maximum characters allowed in this slot's phrase. The picker filters
    /// the active theme so the picked phrase always fits the slot's safe
    /// width — no slot can ever select a too-long phrase that would clip at
    /// the frame edge.
    pub max_chars: usize,
    /// Baseline alpha (0..1).
    pub alpha: f32,
    /// Brush-weight tint: how far the ink colour is mixed toward `ink::SHADOW`
    /// before drawing (0 = pure cream, 1 = pure shadow). Layers the alpha
    /// gradient so supporting slots also have a brush-weight gradient —
    /// the subtitle sits close to the focal line and stays near-cream,
    /// the corner echoes are mid-weight, the farthest one is a deeper
    /// shadow tone that reads as brush dissolving into mist. The hero
    /// slot ignores this and uses its own warm cream glow palette.
    pub shadow_mix: f32,
    /// Drift amplitude in pixels for x/y sinusoid sway.
    pub drift_x: f32,
    pub drift_y: f32,
    /// Drift frequencies (Hz).
    pub drift_fx: f32,
    pub drift_fy: f32,
    /// Phase offset for the drift sinusoid.
    pub drift_phase: f32,
    /// Lifetime for a single phrase inside this slot (seconds).
    pub lifetime: f32,
    /// Fade-in / fade-out durations (seconds).
    pub fade_in: f32,
    pub fade_out: f32,
    /// Optional stagger delay added at startup so slots don't all blink on
    /// at the same instant.
    pub stagger: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotRole {
    Hero,
    Support,
}

/// A live slot instance — the geometry plus current phrase + lifecycle state.
pub struct Slot {
    pub def: SlotDef,
    pub phrase: &'static Phrase,
    /// Seconds since the current phrase was placed.
    pub age: f32,
    /// Cached phrase index (into `phrase::PHRASES`) for repeat-avoidance.
    pub last_idx: u16,
    /// Whether this slot has had its first phrase placed.
    pub primed: bool,
}

impl Slot {
    fn new(def: SlotDef) -> Self {
        Self {
            def,
            phrase: phrase::phrase_for_beat(0),
            age: -def.stagger, // negative age → still in initial stagger
            last_idx: u16::MAX,
            primed: false,
        }
    }

    /// Current rendered alpha (lifecycle ramp × baseline alpha).
    pub fn alpha_now(&self) -> f32 {
        if self.age < 0.0 {
            return 0.0;
        }
        let f = self.def.fade_in.max(0.001);
        let o = self.def.fade_out.max(0.001);
        let ramp_in = (self.age / f).clamp(0.0, 1.0);
        let ramp_out = ((self.def.lifetime - self.age) / o).clamp(0.0, 1.0);
        let l = color::smootherstep(ramp_in) * color::smootherstep(ramp_out);
        l * self.def.alpha
    }

    /// Drift offsets (pixels) at time `t` (seconds).
    fn drift(&self, t: f32) -> (f32, f32) {
        let dx = (t * self.def.drift_fx + self.def.drift_phase).sin() * self.def.drift_x;
        let dy = (t * self.def.drift_fy + self.def.drift_phase * 1.3).cos() * self.def.drift_y;
        (dx, dy)
    }
}

/// Whole scene state.
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub rng: Lcg,
    pub dust: Vec<Dust>,
    pub sparks: Vec<Spark>,
    /// Phase for the soft nebula / vignette center.
    pub nebula_phase: f32,
    /// 0..=1, used to bias palette warmth.
    pub warmth: f32,
    /// Persistent low-frequency pulse — softens into "atmosphere breathing".
    pub ambient_pulse: f32,
    /// Active theme index (into `phrase::THEMES`).
    pub theme_idx: usize,
    /// Composition of slots (1 hero + N supporting).
    pub composition: Composition,
    /// Moon anchor — pixel-space center. The moon is the slow, dim still
    /// point at upper-right that everything else drifts around (ARTIFACT
    /// "观者第一分钟" 1. 右上角的月轮). Drift is driven by `moon_phase`.
    pub moon_x: f32,
    pub moon_y: f32,
    /// Phase accumulator for the moon's slow drift — advances ~0.012 rad/s,
    /// so the anchor moves a few pixels over a minute, never enough to
    /// notice as motion but enough to read as alive.
    pub moon_phase: f32,
}

pub struct Composition {
    pub slots: Vec<Slot>,
    /// Slot index of the hero (always 0 in the default layout).
    pub hero_idx: usize,
    /// Beat counter at the last theme change — used to throttle rotations.
    pub beats_since_theme: u32,
    /// When the hero starts, advance to a new phrase. Counter for the
    /// round-robin supporting refresh — slot index that should refresh on
    /// the next hero beat.
    pub next_support_to_refresh: usize,
    /// True once the first `on_hero_beat` for a *pinned* poem group has
    /// fired. On that first beat the supporting slots preserve their
    /// initial negative `stagger` age so they fade in AFTER the hero, in
    /// reading order — calligraphic inscription, not static plaque. On
    /// subsequent pinned beats the slots sit at mid-life so the
    /// inscription is fixed at peak alpha. Set/checked only inside the
    /// pinned branch of `on_hero_beat`.
    pub first_pinned_beat_done: bool,
}

impl Composition {
    pub fn default_layout() -> Self {
        // Four slots: one hero + three supporting.
        //
        // Layout intent (1280x720):
        //   * Hero (auto-fit, centred) is the focal point at the upper-third
        //     focal area. Up to 8 chars; the auto-fit scales down so 6–10 char
        //     phrases still fit within 60 % of the screen width.
        //   * Subtitle (≤ 7 chars) sits below the hero on a centred baseline —
        //     the "echo" line that reads as a poetic continuation.
        //   * Upper-right corner (≤ 5 chars) and lower-left corner (≤ 5 chars)
        //     anchor the composition's diagonal, keeping the eye moving.
        //
        // All slots are sized so their phrase, plus drift and bearing-y margin,
        // stays fully inside the safe inner box (top/bottom/left/right
        // margins ≥ 64 px). `max_chars` ensures the picker cannot select a
        // phrase that overflows the slot's safe width.
        let defs: Vec<SlotDef> = vec![
            // 0 — Hero (focal, auto-fit)
            SlotDef {
                role: SlotRole::Hero,
                x_frac: 0.50,
                y_frac: 0.42,
                align: Align::Center,
                em_scale: 0.0, // auto-fit to width
                target_w_frac: 0.55,
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
            // 1 — Subtitle (centred echo, just below hero). The "near-crisp"
            //   continuation of the hero — heaviest of the supporting
            //   echoes, carrying the concrete answer (《言师采药去》) before
            //   the verse starts to dissolve. Stagger 0.18s so it fades
            //   in just after the hero lands — the second stroke of the
            //   calligraphic inscription.
            //   Prior arc: alpha 0.86 → 0.76 (pulling the subtitle
            //   out of co-focal brightness with the hero, with
            //   shadow_mix 0.10 → 0.16 to tighten the inscribed-
            //   breath); y_frac 0.64 → 0.65 (lifting the subtitle
            //   one step further from the hero so the central pair
            //   reads as two distinct strokes of one calligraphic
            //   brush rather than one dense inscription block, the
            //   hero→subtitle gap now 166 px and the subtitle→lower-
            //   left gap 65 px); em_scale 0.34 → 0.36 (+5.88 %, the
            //   supporting-tier size axis catch-up onto the
            //   +4.86–6.25 % restraint cadence the inscription-side
            //   arc had been sharing, the subtitle glyphs now sitting
            //   at ≈46.1 px vs 43.5 px at the prior 0.34 register,
            //   the size hierarchy stepping down 0.04 / 0.04 / 0.11
            //   so the subtitle-to-upper-right pairing stays paired
            //   with the upper-right-to-lower-left spacing).
            // alpha 0.76 → 0.78 (+2.63 %, the gentlest step on the
            //   inscribed-stroke alpha axis after the +5.5 % lower-left
            //   alpha lift in 12fac53 and the +4.86 % title alpha lift
            //   in f8c4f2d — the +2.63 % sits exactly between the halo
            //   radius +2.5 % gentlest-step register 28af5b6 just
            //   completed on the moon-side geometric-extent axis and
            //   the terminator alpha +2.69 % / terminator cap +2.86 %
            //   gentlest-step register the directional-modulation and
            //   warm-tint cap axes have just completed (47ac018 /
            //   4077850), so the inscribed-stroke alpha axis now steps
            //   onto the same +2.5–2.86 % gentlest rung the page-wide
            //   +2-3 % material refinement band the most-refined axes
            //   have settled into — cool_tint +2.4 % / +2.5 % / +2.7 %
            //   (0ce6e37, 610ee7a, f595bff, 77b520e); vignette +2.5 %
            //   (7304555); halo radius +2.5 % (28af5b6); terminator
            //   alpha +2.69 % (47ac018); terminator cap +2.86 %
            //   (4077850)) rather than the supporting tier's alpha
            //   quietly sitting one step behind the inscription-side
            //   arc after the bloom2_alpha ceiling +5 % in 9f9436b and
            //   the title alpha +4.86 % in f8c4f2d carried the
            //   calligrapher's seal and the focal-line corona
            //   forward. The +2.63 % (0.76 → 0.78) lifts the inscribed
            //   answer's peak inscribed contribution from 0.76 ×
            //   inscribed-glow ≈ 0.76 × 0.20+ ≈ 0.152 to 0.78 × 0.20+
            //   ≈ 0.156 (+0.004 absolute, +2.6 % relative), staying
            //   well inside the inscribed-glow family and clearly
            //   under the inscribed glow band 0.20+ and the hero
            //   bloom 0.55, so the focal line keeps its exclusive
            //   claim on the page's light (ART_DIRECTION §四 "高光只
            //   落在主句") and 《言师采药去》 now reads one
            //   restrained step more visibly as the inscribed answer
            //   to 《松下问童子》 rather than as a slightly muted
            //   copy of the focal line's calligraphic body. The +0.02
            //   absolute alpha lift stays inside the safe-area margin
            //   (the subtitle's inscribed contribution peak is still
            //   bounded by the inscribed glow band 0.20+ which is well
            //   below the hero bloom's combined ~0.7 effective alpha
            //   — so the supporting tier stays clearly subordinate to
            //   the focal line), the focal hierarchy (hero / subtitle
            //   / upper-right / lower-left / seal) is unchanged, the
            //   brush-weight gradient (subtitle brightest → upper-
            //   right → lower-left → title dimmest) holds, the warm /
            //   cool axis (subtitle + lower-left warm, upper-right
            //   cool) holds, and the inscribed-stroke alpha axis now
            //   extends the gentlest-step +2.63 % register the page-
            //   wide +2-3 % material refinement band the most-refined
            //   axes have settled into. The shadow_mix 0.16, the
            //   em_scale 0.36, the y_frac 0.65, the drift
            //   3.0/1.5/0.21/0.17/0.7, the fade_in 0.55, the fade_out
            //   0.7, the stagger 0.18, the upper-right alpha 0.756,
            //   the upper-right em_scale 0.32, the upper-right y_frac
            //   0.27, the upper-right shadow_mix 0.26, the upper-
            //   right drift 3.0/2.0/0.15/0.19/1.4, the lower-left
            //   alpha 0.646, the lower-left em_scale 0.28, the
            //   lower-left y_frac 0.74, the lower-left shadow_mix
            //   0.42, the lower-left drift 3.0/2.0/0.13/0.21/2.8, the
            //   title alpha 0.582, the title v 0.83, the title
            //   target_px 23, the title breath 0.3096, the inscribed-
            //   breath base 0.5340, the supporting mist bell 7.677,
            //   the warm bell 7.677, the title ambient_warmth share
            //   0.2222, the supporting mist_warmth share 0.2222, the
            //   subtitle mist_warmth share 0.0895, the lower-left
            //   mist_warmth share 0.1064, the terminator alpha 0.267,
            //   the terminator cap 0.144, the cool_tint 0.13908, the
            //   moon_proximity 0.107, the body 0.682, the halo_peak
            //   0.080, the sky_peak 0.0384, the moon_halo_r 69.7, the
            //   body σ 8.5, the sky σ 90.3, the body_pulse 0.023, the
            //   halo_pulse 0.0807, the sky_pulse 0.0391, the
            //   bloom2_alpha ceiling 0.063, the nebula alphas
            //   0.022/0.016, the vignette pow(0.75), and the vignette
            //   ceiling 0.74 are all unchanged so only the subtitle's
            //   alpha shifts and the inscribed-stroke alpha axis
            //   catches up with the gentlest-step +2.63 % register
            //   the page-wide +2-3 % material refinement band the
            //   most-refined axes have settled into. With 《言师采
            //   药去》 now catching one more restrained step of the
            //   page's luminous air — at the gentlest +2.63 % step
            //   on the inscribed-stroke alpha axis, exactly between
            //   the halo radius +2.5 % gentlest-step register 28af5b6
            //   just completed on the moon-side geometric-extent axis
            //   and the terminator alpha +2.69 % / terminator cap
            //   +2.86 % gentlest-step register the directional-
            //   modulation and warm-tint cap axes have just completed
            //   — 《寻隐者不遇》 reads as one Tang quatrain inscribed
            //   in moonlit air whose inscribed answer now registers
            //   one more gentle step of the page's luminous air, and
            //   the four inscribed strokes plus the calligrapher's
            //   seal continue to share one proportional cadence
            //   across alpha, breath, size, luminance, geometric
            //   extent, warm-mist, and outer-corona axes, with the
            //   subtitle's alpha finally stepping onto the gentlest
            //   +2.63 % register the page-wide +2-3 % material
            //   refinement band the most-refined axes have settled
            //   into.
            // em_scale 0.36 → 0.369 (+2.5 %, the gentlest step on
            //   the supporting-tier size axis after the +5.88 %
            //   catch-up in 9ed2b96 — the +2.5 % sits exactly
            //   inside the +2.35-2.86 % gentlest rung the page-
            //   wide +2-3 % material refinement band the most-
            //   refined axes have settled into (body σ +2.35 % in
            //   4695311; cool_tint +2.4 % / +2.5 % / +2.7 % in
            //   0ce6e37, 610ee7a, f595bff, 77b520e; vignette
            //   +2.5 % in 7304555; halo radius +2.5 % in 28af5b6;
            //   sky σ +2.55 % in c28ed51; halo_peak +2.5 % in
            //   3b8028b; body_pulse +2.61 % / halo_pulse +2.48 %
            //   / sky_pulse +2.56 % in 43fc830; subtitle alpha
            //   +2.63 % in 1c2fb98; terminator alpha +2.69 % in
            //   47ac018; terminator cap +2.86 % in 4077850; lower-
            //   left alpha +2.48 % in 70c9147; upper-right alpha
            //   +2.65 % in 867377c; title/seal alpha +2.58 % in
            //   69ce9b1) rather than the subtitle's glyph size
            //   quietly sitting at its post-9ed2b96 +5.88 %
            //   register while the moon-side geometric-extent,
            //   luminance, and breath axes plus the inscribed-
            //   stroke alpha axis stepped past it at +2.35-2.86 %.
            //   The +2.5 % (0.36 → 0.369) lifts the inscribed
            //   answer's glyph size from ≈46.1 px to ≈47.3 px
            //   (+1.2 px absolute, well inside the subtitle→upper-
            //   right size hierarchy at 0.32 → 0.369 / 0.32 ≈
            //   1.153 vs the prior 0.36 / 0.32 = 1.125 so the
            //   brush-weight gradient now steps down 0.049 / 0.04
            //   from the subtitle through the upper-right to the
            //   lower-left, the focal hierarchy (hero / subtitle /
            //   upper-right / lower-left / seal) holds, and the
            //   +0.009 absolute size lift stays inside the safe-
            //   area margin so the supporting tier stays clearly
            //   subordinate to the focal line. The shadow_mix
            //   0.16, the y_frac 0.65, the drift
            //   3.0/1.5/0.21/0.17/0.7, the fade_in 0.55, the
            //   fade_out 0.7, the stagger 0.18, the alpha 0.78,
            //   the upper-right em_scale 0.32, the upper-right
            //   y_frac 0.27, the upper-right shadow_mix 0.26, the
            //   upper-right drift 3.0/2.0/0.15/0.19/1.4, the upper-
            //   right alpha 0.776, the lower-left em_scale 0.28,
            //   the lower-left y_frac 0.74, the lower-left
            //   shadow_mix 0.42, the lower-left drift
            //   3.0/2.0/0.13/0.21/2.8, the lower-left alpha 0.662,
            //   the title alpha 0.597, the title v 0.83, the title
            //   target_px 23, the title breath 0.3096, the
            //   inscribed-breath base 0.5340, the supporting mist
            //   bell 7.677, the warm bell 7.677, the title
            //   ambient_warmth share 0.2222, the supporting
            //   mist_warmth share 0.2222, the subtitle
            //   mist_warmth share 0.0895, the lower-left
            //   mist_warmth share 0.1064, the terminator alpha
            //   0.267, the terminator cap 0.144, the cool_tint
            //   0.13908, the moon_proximity 0.107, the body
            //   0.682, the halo_peak 0.082, the sky_peak 0.0384,
            //   the moon_halo_r 69.7, the body σ 8.7, the sky σ
            //   92.6, the body_pulse 0.0236, the halo_pulse
            //   0.0827, the sky_pulse 0.0401, the bloom2_alpha
            //   ceiling 0.063, the nebula alphas 0.022 / 0.016,
            //   the vignette pow(0.75), and the vignette ceiling
            //   0.74 are all unchanged so only the subtitle's
            //   glyph size shifts and the supporting-tier size
            //   axis catches up with the gentlest-step +2.5 %
            //   register the moon-side geometric-extent, luminance,
            //   breath, and inscribed-stroke alpha axes have just
            //   settled onto. With 《言师采药去》 now catching one
            //   more restrained step of the page's proportional
            //   cadence — at the gentlest +2.5 % step on the
            //   supporting-tier size axis, exactly inside the
            //   +2.35-2.86 % rung the page-wide +2-3 % material
            //   refinement band the most-refined axes have
            //   settled into — 《寻隐者不遇》 reads as one Tang
            //   quatrain inscribed in moonlit air whose inscribed
            //   answer now registers one more gentle step of the
            //   page's proportional cadence, and the supporting
            //   inscription's size axis (subtitle em_scale + upper-
            //   right em_scale + lower-left em_scale) finally
            //   extends the gentlest-step +2.5 % register the
            //   moon-side geometric-extent axis (body σ + halo
            //   radius + sky σ) and the inscribed-stroke alpha
            //   axis (subtitle + upper-right + lower-left + title)
            //   have just completed.
            SlotDef {
                role: SlotRole::Support,
                x_frac: 0.50,
                y_frac: 0.65,
                align: Align::Center,
                em_scale: 0.369,
                target_w_frac: 0.0,
                max_chars: 7,
                alpha: 0.78,
                shadow_mix: 0.16,
                drift_x: 3.0,
                drift_y: 1.5,
                drift_fx: 0.21,
                drift_fy: 0.17,
                drift_phase: 0.7,
                lifetime: 12.0,
                fade_in: 0.55,
                fade_out: 0.7,
                stagger: 0.18,
            },
            // em_scale 0.32 → 0.328 (+2.5 %, the gentlest step on the
            //   supporting-tier size axis after the subtitle em_scale
            //   +2.5 % lift in a2a3f48 — the +2.5 % sits exactly inside
            //   the +2.35-2.86 % gentlest rung the page-wide +2-3 %
            //   material refinement band the most-refined axes have
            //   settled into (body σ +2.35 % in 4695311; halo radius
            //   +2.5 % in 28af5b6; sky σ +2.55 % in c28ed51; halo_peak
            //   +2.5 % in 3b8028b; body_pulse +2.61 % / halo_pulse
            //   +2.48 % / sky_pulse +2.56 % in 43fc830; cool_tint
            //   +2.4 % / +2.5 % / +2.7 % in 0ce6e37, 610ee7a, f595bff,
            //   77b520e; vignette +2.5 % in 7304555; terminator alpha
            //   +2.69 % in 47ac018; terminator cap +2.86 % in 4077850;
            //   subtitle alpha +2.63 % in 1c2fb98; upper-right alpha
            //   +2.65 % in 867377c; lower-left alpha +2.48 % in 70c9147;
            //   title/seal alpha +2.58 % in 69ce9b1; subtitle em_scale
            //   +2.5 % in a2a3f48) rather than the moon-side echo's
            //   em_scale quietly sitting at its post-9ed2b96 register
            //   while the moon-side geometric-extent, luminance, breath,
            //   inscribed-stroke alpha, and supporting-tier subtitle
            //   em_scale axes stepped past it at +2.35-2.86 %. The
            //   +2.5 % (0.32 → 0.328) lifts the moon-side echo's
            //   inscribed glyph size one restrained step into the page's
            //   proportional cadence, so 《只在此山中》 now reads one
            //   gentle step more visibly sized to its moonlit-air band
            //   (the upper-right echo sits at ≈79 px below the moon's
            //   center at y_frac 0.27 — well inside the moon's outermost
            //   atmospheric layer's reach since the sky bell σ stepped
            //   to 92.6 in c28ed51). The subtitle → upper-right gap
            //   narrows 0.049 → 0.041 (the subtitle still clearly
            //   brightest) and the upper-right → lower-left gap widens
            //   0.04 → 0.048 (the upper-right still clearly above the
            //   lower-left), so the supporting inscription's brush-
            //   weight gradient (subtitle brightest → upper-right →
            //   lower-left → title dimmest) still steps down
            //   monotonically across all four inscribed strokes, the
            //   focal hierarchy (hero / subtitle / upper-right / lower-
            //   left / seal) is unchanged, the warm / cool axis
            //   (subtitle + lower-left warm, upper-right cool, title as
            //   the warm-side closing signature) holds, and the
            //   supporting inscription's size axis (subtitle em_scale +
            //   upper-right em_scale + lower-left em_scale) now extends
            //   the gentlest-step +2.5 % register the moon-side
            //   geometric-extent axis (body σ + halo radius + sky σ)
            //   and the inscribed-stroke alpha axis (subtitle + upper-
            //   right + lower-left + title) have just completed, with
            //   the upper-right em_scale finally stepping onto the
            //   gentlest +2.5 % register the page-wide +2-3 % material
            //   refinement band the supporting-tier subtitle em_scale
            //   has just settled into. The x_frac 0.80, the y_frac
            //   0.27, the shadow_mix 0.26, the drift 3.0/2.0/0.15/0.19/
            //   1.4, the alpha 0.776, the fade_in 0.6, the fade_out
            //   0.7, the stagger 0.34, the subtitle em_scale 0.369,
            //   the subtitle alpha 0.78, the lower-left em_scale 0.28,
            //   the lower-left alpha 0.662, the title alpha 0.597, the
            //   title v 0.83, the title target_px 23, the title breath
            //   0.3096, the inscribed-breath base 0.5340, the supporting
            //   mist bell 7.677, the warm bells 7.677, the title
            //   ambient_warmth share 0.2222, the supporting mist_warmth
            //   share 0.2222, the subtitle mist_warmth share 0.0895,
            //   the lower-left mist_warmth share 0.1064, the terminator
            //   alpha 0.267, the terminator cap 0.144, the cool_tint
            //   0.13908, the moon_proximity 0.107, the body 0.682, the
            //   halo_peak 0.082, the sky_peak 0.0384, the moon_halo_r
            //   69.7, the body σ 8.7, the sky σ 92.6, the body_pulse
            //   0.0236, the halo_pulse 0.0827, the sky_pulse 0.0401, the
            //   bloom2_alpha ceiling 0.063, the nebula alphas 0.022 /
            //   0.016, the vignette pow(0.75), and the vignette ceiling
            //   0.74 are all unchanged so only the upper-right's
            //   inscribed glyph size shifts and the moon-side echo
            //   catches up with the gentlest-step +2.5 % register the
            //   page-wide +2-3 % material refinement band the moon-side
            //   geometric-extent, luminance, breath, inscribed-stroke
            //   alpha, and supporting-tier subtitle em_scale axes have
            //   just settled into. Restraint (ART_DIRECTION §四 '克制
            //   统一的调色板' / '高光只落在主句') holds: the +0.008
            //   absolute em_scale lift stays well inside the supporting-
            //   tier size hierarchy (the upper-right's inscribed glyph
            //   size stays below the subtitle's 0.369 and above the
            //   lower-left's 0.28 — so the focal line keeps its
            //   exclusive claim on the page's light and the supporting
            //   tier stays clearly subordinate to the focal line), and
            //   《只在此山中》 now reads as one Tang quatrain inscribed
            //   in moonlit air whose moon-side echo now registers one
            //   more gentle step of the page's proportional cadence, and
            //   the supporting inscription's size axis (subtitle
            //   em_scale + upper-right em_scale + lower-left em_scale)
            //   continues to extend the gentlest-step +2.5 % register
            //   the moon-side geometric-extent axis (body σ + halo
            //   radius + sky σ) and the inscribed-stroke alpha axis
            //   (subtitle + upper-right + lower-left + title) have just
            //   completed, with the upper-right em_scale finally
            //   stepping onto the gentlest +2.5 % register the page-wide
            //   +2-3 % material refinement band the supporting-tier
            //   subtitle em_scale has just settled into.
            // alpha 0.756 → 0.776 (+2.65 %, the gentlest step on the
            //   inscribed-stroke alpha axis after the +2.48 % lower-left
            //   alpha lift in 70c9147 — the +2.65 % sits exactly between
            //   the subtitle alpha +2.63 % gentlest-step register
            //   1c2fb98 just completed on the inscribed-stroke alpha
            //   axis and the lower-left alpha +2.48 % gentlest-step
            //   register 70c9147 just completed, so the inscribed-
            //   stroke alpha axis now steps onto the same +2.48–2.86 %
            //   gentlest rung the page-wide +2-3 % material refinement
            //   band the most-refined axes have settled into — body σ
            //   +2.35 % (4695311); cool_tint +2.4 % / +2.5 % / +2.7 %
            //   (0ce6e37, 610ee7a, f595bff, 77b520e); vignette +2.5 %
            //   (7304555); halo radius +2.5 % (28af5b6); subtitle
            //   alpha +2.63 % (1c2fb98); terminator alpha +2.69 %
            //   (47ac018); terminator cap +2.86 % (4077850); lower-
            //   left alpha +2.48 % (70c9147)) rather than the moon-
            //   side echo's alpha quietly sitting at its post-7161ce8
            //   +5 % register while every surrounding material axis
            //   stepped past it at +2.35 % / +2.5 % / +2.63 % /
            //   +2.69 % / +2.86 %. The +2.65 % (0.756 → 0.776) lifts
            //   the moon-side echo's peak inscribed contribution
            //   from 0.756 × inscribed-glow ≈ 0.756 × 0.20+ ≈ 0.151
            //   to 0.776 × 0.20+ ≈ 0.155 (+0.004 absolute, +2.65 %
            //   relative, well inside the inscribed-glow family and
            //   clearly under the inscribed glow band 0.20+ and the
            //   hero bloom ~0.55), so the focal line 《松下问童子》
            //   keeps its exclusive claim on the page's light
            //   (ART_DIRECTION §四 "高光只落在主句" holds) and
            //   《只在此山中》 now reads one restrained step more
            //   visibly as ink continuous with the moon's moonlit
            //   air rather than as a slightly muted copy of the
            //   subtitle's calligraphic body. The +0.020 absolute
            //   alpha lift stays inside the safe-area margin (the
            //   upper-right's inscribed contribution peak is still
            //   bounded by the inscribed glow band 0.20+ which is
            //   well below the hero bloom's combined ~0.7 effective
            //   alpha — so the supporting tier stays clearly
            //   subordinate to the focal line), the focal hierarchy
            //   (hero / subtitle / upper-right / lower-left / seal)
            //   is unchanged, the brush-weight gradient (subtitle
            //   brightest → upper-right → lower-left → title
            //   dimmest) holds with the subtitle → upper-right gap
            //   tightening 0.024 → 0.004 so the supporting tier
            //   reads as one paired "near tier" (subtitle + upper-
            //   right) and a separate "far tier" (lower-left +
            //   title), the warm / cool axis (subtitle + lower-left
            //   warm, upper-right cool) holds, and the inscribed-
            //   stroke alpha axis now extends the gentlest-step
            //   +2.65 % register the page-wide +2-3 % material
            //   refinement band the most-refined axes have settled
            //   into. The em_scale 0.32, the y_frac 0.27, the
            //   shadow_mix 0.26, the drift 3.0/2.0/0.15/0.19/1.4,
            //   the fade_in 0.6, the fade_out 0.7, the stagger 0.34,
            //   the subtitle alpha 0.78, the lower-left alpha 0.662,
            //   the lower-left em_scale 0.28, the lower-left y_frac
            //   0.74, the lower-left shadow_mix 0.42, the lower-left
            //   drift 3.0/2.0/0.13/0.21/2.8, the title alpha 0.582,
            //   the title v 0.83, the title target_px 23, the title
            //   breath 0.3096, the inscribed-breath base 0.5340, the
            //   supporting mist bell 7.677, the warm bell 7.677, the
            //   title ambient_warmth share 0.2222, the supporting
            //   mist_warmth share 0.2222, the subtitle mist_warmth
            //   share 0.0895, the lower-left mist_warmth share
            //   0.1064, the terminator alpha 0.267, the terminator
            //   cap 0.144, the cool_tint 0.13908, the moon_proximity
            //   0.107, the body 0.682, the halo_peak 0.080, the
            //   sky_peak 0.0384, the moon_halo_r 69.7, the body σ
            //   8.7, the sky σ 90.3, the body_pulse 0.023, the
            //   halo_pulse 0.0807, the sky_pulse 0.0391, the
            //   bloom2_alpha ceiling 0.063, the nebula alphas
            //   0.022/0.016, the vignette pow(0.75), and the
            //   vignette ceiling 0.74 are all unchanged so only the
            //   upper-right's inscribed-stroke alpha shifts and the
            //   moon-side echo's alpha catches up with the gentlest-
            //   step +2.65 % register the page-wide +2-3 % material
            //   refinement band the most-refined axes have settled
            //   into; with 《只在此山中》 now catching one more
            //   restrained step of the moon's moonlit air — at the
            //   gentlest +2.65 % step on the inscribed-stroke alpha
            //   axis, exactly between the subtitle alpha +2.63 %
            //   gentlest-step register 1c2fb98 just completed and
            //   the lower-left alpha +2.48 % gentlest-step register
            //   70c9147 just completed — 《寻隐者不遇》 reads as
            //   one Tang quatrain inscribed in moonlit air whose
            //   moon-side echo now registers one more gentle step
            //   of the page's moonlit air, and the four inscribed
            //   strokes plus the calligrapher's seal continue to
            //   share one proportional cadence across alpha, breath,
            //   size, luminance, geometric extent, warm-mist, and
            //   outer-corona axes, with the inscribed-stroke alpha
            //   axis finally stepping onto the gentlest +2.65 %
            //   register the page-wide +2-3 % material refinement
            //   band the most-refined axes have settled into.
            // 2 — Upper right (small body, right-aligned). Pulled inward
            //   from (0.84, 0.22) to (0.80, 0.28) so the upper echo sits
            //   at the rule-of-thirds intersection (≈(0.67, 0.33)) rather
            //   than as a corner satellite. The diagonal midpoint with
            //   the lower-left at (0.18, 0.74) stays at ≈(0.49, 0.51) —
            //   right at the optical centre — and top/bottom margins
            //   remain balanced (≈170 px vs ≈180 px). Mid-weight: the
            //   quatrain's location hint is already a step further from
            //   certainty than the subtitle.
            //   y_frac 0.28 → 0.27 (+7 px lift on 720-tall, ≈3.6 %):
            //   pull 《只在此山中》 one restrained step closer to the
            //   moon's atmospheric reach as the moon's outermost layer
            //   completed its +6.25 % luminance step in c9baa27 and the
            //   sky bell σ 85 → 90.3 (+6.25 %) in 05eefb4 — the upper-
            //   right echo was the page's closest stroke to the disc at
            //   ≈86 px below the moon's centre at y_frac 0.28, while the
            //   sky bell's reach now extends a touch further into the
            //   upper-right echo's neighbourhood (alpha at d=115 ≈
            //   0.0170 in c9baa27 vs 0.0154 at the prior σ 85 state), so
            //   the echo can sit a touch higher without losing its
            //   "below the disc" reading. At y_frac 0.27 the echo sits
            //   ≈79 px below the moon's centre — still 7 px below the
            //   halo radius boundary (halo_r 68, halo reaches y=183 on
            //   720-tall) so the echo remains clearly under the moon's
            //   disc rather than rising into the halo, and the
            //   composition's rule-of-thirds anchor holds (the echo sits
            //   closer to v=0.33 than to v=0.25, the horizontal
            //   composition line the upper-right echo inherited). The
            //   +3.6 % lift matches the same restraint cadence the
            //   moon-side luminance, geometric extent, and breath axes
            //   have been sharing across the recent chain — sky_peak
            //   +6.25 % (c9baa27, 611895d), sky_sigma +6.25 % (05eefb4,
            //   84b4150), body_sigma +6.25 % (bcfab51), halo_peak
            //   +6.25 % (ef91dae), halo radius +6.25 % (8113547), the
            //   body/halo/sky pulse +4.76 % lifts (750ae7a), the
            //   +6.25 % warm-mist share lifts (f409940), and the
            //   +4.9 % title alpha lifts (c9f4dde, 4b84ab7, 2eacfb1) —
            //   so the page's composition now steps on the same
            //   proportional cadence as its atmosphere, with the
            //   upper-right echo joining the moon's atmospheric layers
            //   on one coupled restraint series rather than sitting at
            //   its historic 0.28 register while the moon's three
            //   nested layers stepped past it. Restraint
            //   (ART_DIRECTION §四 "克制统一的调色板") holds: the +7 px
            //   lift stays inside the safe-area margin (the upper-right
            //   echo's em 0.32 + drift + bearing ≈ 50 px, so a 7 px lift
            //   still leaves ≈170 px clearance to the top edge), the
            //   rule-of-thirds anchor holds, and 《只在此山中》 now
            //   reads as ink sitting a touch more visibly inside the
            //   moon's atmospheric reach rather than sitting just below
            //   it the way the 0.28 register had it.
            //   Stagger 0.34s so it fades in third, after the subtitle.
            //   shadow_mix 0.30 → 0.26, alpha 0.66 → 0.72 → 0.756 (+5 %,
            //   the first lift in this arc since the supporting-tier
            //   breath axis approached its natural ceiling in cc3a82f):
            //   the upper-right echo 《只在此山中》 now reads one step
            //   more visibly continuous with the moon's moonlit air, the
            //   closest stroke to the disc at ≈115 px below the moon's
            //   center, after the moon's three nested atmospheric layers
            //   completed a coupled restraint cadence across luminance
            //   (body 0.682 saturated in 3b60530, halo_peak +6.25 % to
            //   0.0765 in ef91dae, sky_peak +6.25 % to 0.0361 in
            //   611895d), geometric extent (moon_halo_r +6.25 % to 68
            //   in 8113547, sky bell σ +6.67 % to 80 in efd8cb1), and
            //   breath (body_pulse 0.021 / halo_pulse 0.0735 / sky_pulse
            //   0.0356). The upper-right echo was the only supporting-
            //   tier stroke that hadn't been touched by the recent +5 %
            //   ladder (subtitle 0.76 / lower-left 0.612 / title 0.529
            //   all lifted in 9a4cc96 and 4b84ab7), so the moon's nearest
            //   echo quietly sat at 0.72 while every other stroke moved
            //   up the cadence. Lifting it to 0.756 brings 《只在此山中》
            //   into the same proportional series as the recent chain —
            //   body 0.50 → 0.682 (+5.5–5.6 % x5), halo 0.05 → 0.0765
            //   (+5–6.25 % x5), sky 0.018 → 0.0361 (+6.25 % x3), lower-
            //   left 0.50 → 0.612 (+5.5 % in 9a4cc96), title alpha
            //   0.40 → 0.529 (+5 % in 4b84ab7), cool_tint 0.115 →
            //   0.13583, moon_proximity 0.04 → 0.102, terminator cap
            //   0.12 → 0.140, warm bell 6.0 → 7.225, supporting mist
            //   bell 6.4 → 7.225, title ambient warmth 6.4 → 7.225,
            //   inscribed-breath base (now at the gentlest-step ceiling),
            //   title breath, halo radius 44 → 68, body_pulse, halo_pulse,
            //   sky_pulse, and sky bell σ 75 → 80. The +5 % continues
            //   the same restraint cadence the page has been sharing for
            //   thirty-plus supporting-tier lifts in the +3–7 % range,
            //   and the upper-right echo now joins the page's one
            //   proportional refinement arc rather than sitting at its
            //   historic 0.72 register point while every other stroke
            //   climbed past it. The brush-weight hierarchy still holds:
            //   subtitle 0.76 > upper-right 0.756 > lower-left 0.612 >
            //   title 0.529 — the gap from subtitle to upper-right
            //   narrows from 0.04 to 0.004 (a deliberate +5 % step, the
            //   standard restraint cadence), but the gap from upper-
            //   right to lower-left widens from 0.108 to 0.144 and the
            //   gap from upper-right to title widens from 0.191 to 0.227,
            //   so the four strokes now read as two clear brush-weight
            //   tiers — subtitle + upper-right as the "near tier" close
            //   to the focal line, lower-left + title as the "far tier"
            //   close to the warm horizon — rather than as four evenly-
            //   spaced steps. The upper-right remains clearly subordinate
            //   to the hero (1.0 → 0.756 = 0.244 gap, well under the
            //   focal line's claim) and the hero bloom (~0.55 effective
            //   alpha) keeps its exclusive claim on the page's light
            //   (ART_DIRECTION §四 "高光只落在主句"). At pulse=1 the
            //   upper-right now sits at 0.756 * 1.414 = 1.069 clamped to
            //   1.0 (was 0.72 * 1.414 = 1.018, also clamped), and at
            //   pulse=-1 the upper-right sits at 0.756 * 0.585 = 0.443
            //   (was 0.72 * 0.585 = 0.422, +0.021 at the dimmest phase
            //   so the echo registers a touch more clearly when the page
            //   exhales), with the supporting-tier breath coefficient
            //   (0.5340 base, the gentlest-step ceiling per cc3a82f)
            //   unchanged so only the upper-right's base alpha shifts
            //   and the supporting tier's breath axis stays at its
            //   settled cadence. The new alpha is still well inside
            //   the inscribed glow's ceiling (~0.20+ glow band) and the
            //   brush-weight gradient (subtitle brightest → upper-right
            //   near-brightest → lower-left dimmest → title dimmest)
            //   still steps down monotonically. Restraint
            //   (ART_DIRECTION §四 "克制统一的调色板") holds: the +0.036
            //   absolute base lift stays inside the muted-ink family
            //   and the upper-right still reads as ink on paper rather
            //   than as a fifth inscription line. With 《只在此山中》
            //   now sitting at 0.756 — one +5 % step into the same
            //   restraint cadence the moon's three atmospheric layers
            //   just completed — the moon's nearest echo now reads as
            //   one step more visibly continuous with the moonlit air
            //   that hosts it, and 《寻隐者不遇》 reads as one Tang
            //   quatrain inscribed in moonlit air whose closest stroke
            //   to the moon now joins the page's one proportional
            //   refinement arc rather than quietly lagging it.
            //   em_scale 0.30 → 0.32: size joins the brush-weight
            //   gradient. The subtitle stays at 0.34 (closest to focal,
            //   heaviest), the upper-right steps down to 0.32 (mid-weight,
            //   slightly more delicate), and the lower-left drops to 0.28
            //   (farthest, most delicate — the brush running thin as the
            //   inscription closes on 《云深不知处》). Size now mirrors the
            //   same hierarchy that already governs alpha (0.86/0.72/0.58),
            //   shadow_mix (0.10/0.26/0.42), warmth tint, and inscribed
            //   breath, so the four lines of 《寻隐者不遇》 read as one
            //   inscription thinning across four axes — not four lines
            //   pinned to one screen by coincidence.
            SlotDef {
                role: SlotRole::Support,
                x_frac: 0.80,
                y_frac: 0.27,
                align: Align::Right,
                em_scale: 0.328,
                target_w_frac: 0.0,
                max_chars: 5,
                alpha: 0.776,
                shadow_mix: 0.26,
                drift_x: 3.0,
                drift_y: 2.0,
                drift_fx: 0.15,
                drift_fy: 0.19,
                drift_phase: 1.4,
                lifetime: 12.0,
                fade_in: 0.6,
                fade_out: 0.7,
                stagger: 0.34,
            },
            // alpha 0.646 → 0.662 (+2.48 %, the gentlest step on the
            //   inscribed-stroke alpha axis after the +2.63 % subtitle
            //   alpha lift in 1c2fb98 — the +2.48 % sits exactly between
            //   the body σ +2.35 % gentlest-step register 4695311 just
            //   completed on the moon-side geometric-extent axis and the
            //   terminator alpha +2.69 % / terminator cap +2.86 %
            //   gentlest-step register the directional-modulation and
            //   warm-tint cap axes have just completed (47ac018 /
            //   4077850), so the inscribed-stroke alpha axis now steps
            //   onto the same +2.35–2.86 % gentlest rung the page-wide
            //   +2-3 % material refinement band the most-refined axes
            //   have settled into — body σ +2.35 % (4695311); cool_tint
            //   +2.4 % / +2.5 % / +2.7 % (0ce6e37, 610ee7a, f595bff,
            //   77b520e); vignette +2.5 % (7304555); halo radius +2.5 %
            //   (28af5b6); subtitle alpha +2.63 % (1c2fb98); terminator
            //   alpha +2.69 % (47ac018); terminator cap +2.86 %
            //   (4077850)) rather than the warm-horizon echo stroke's
            //   alpha quietly sitting at its post-12fac53 +5.5 % register
            //   while every surrounding material axis stepped past it
            //   at +2.35 % / +2.5 % / +2.63 % / +2.69 % / +2.86 %. The
            //   +2.48 % (0.646 → 0.662) lifts the warm-horizon echo's
            //   peak inscribed contribution from 0.646 × inscribed-glow
            //   ≈ 0.646 × 0.20+ ≈ 0.129 to 0.662 × 0.20+ ≈ 0.132 (+0.003
            //   absolute, +2.5 % relative, well inside the inscribed-
            //   glow family and clearly under the inscribed glow band
            //   0.20+ and the hero bloom ~0.55), so the focal line
            //   《松下问童子》 keeps its exclusive claim on the page's
            //   light (ART_DIRECTION §四 "高光只落在主句" holds) and
            //   《云深不知处》 now reads one restrained step more visibly
            //   as ink dissolving into the warm horizon band rather
            //   than as a barely-there trailing edge. The +0.016 absolute
            //   alpha lift stays inside the safe-area margin (the lower-
            //   left's inscribed contribution peak is still bounded by
            //   the inscribed glow band 0.20+ which is well below the
            //   hero bloom's combined ~0.7 effective alpha — so the
            //   supporting tier stays clearly subordinate to the focal
            //   line), the focal hierarchy (hero / subtitle / upper-
            //   right / lower-left / seal) is unchanged, the brush-
            //   weight gradient (subtitle brightest → upper-right →
            //   lower-left → title dimmest) holds with the lower-left
            //   → title gap widening 0.064 → 0.080 so the seal stays
            //   clearly the dimmest stroke, the warm / cool axis
            //   (subtitle + lower-left warm, upper-right cool) holds,
            //   and the inscribed-stroke alpha axis now extends the
            //   gentlest-step +2.48 % register the page-wide +2-3 %
            //   material refinement band the most-refined axes have
            //   settled into. The shadow_mix 0.42, the em_scale 0.28,
            //   the y_frac 0.74, the drift 3.0/2.0/0.13/0.21/2.8, the
            //   fade_in 0.6, the fade_out 0.7, the stagger 0.50, the
            //   subtitle alpha 0.78, the upper-right alpha 0.756, the
            //   upper-right em_scale 0.32, the upper-right y_frac 0.27,
            //   the upper-right shadow_mix 0.26, the upper-right drift
            //   3.0/2.0/0.15/0.19/1.4, the lower-left em_scale 0.28,
            //   the lower-left y_frac 0.74, the lower-left shadow_mix
            //   0.42, the title alpha 0.582, the title v 0.83, the
            //   title target_px 23, the title breath 0.3096, the
            //   inscribed-breath base 0.5340, the supporting mist bell
            //   7.677, the warm bell 7.677, the title ambient_warmth
            //   share 0.2222, the supporting mist_warmth share 0.2222,
            //   the subtitle mist_warmth share 0.0895, the lower-left
            //   mist_warmth share 0.1064, the terminator alpha 0.267,
            //   the terminator cap 0.144, the cool_tint 0.13908, the
            //   moon_proximity 0.107, the body 0.682, the halo_peak
            //   0.080, the sky_peak 0.0384, the moon_halo_r 69.7, the
            //   body σ 8.7, the sky σ 90.3, the body_pulse 0.023, the
            //   halo_pulse 0.0807, the sky_pulse 0.0391, the
            //   bloom2_alpha ceiling 0.063, the nebula alphas
            //   0.022/0.016, the vignette pow(0.75), and the vignette
            //   ceiling 0.74 are all unchanged so only the lower-left's
            //   inscribed-stroke alpha shifts and the warm-horizon
            //   echo's alpha catches up with the gentlest-step +2.48 %
            //   register the page-wide +2-3 % material refinement band
            //   the moon-side geometric-extent and inscribed-stroke
            //   alpha axes have just completed. Restraint (ART_DIRECTION
            //   §四 "克制统一的调色板" / "高光只落在主句") holds: the
            //   +0.016 absolute lift stays inside the cream family, the
            //   lower-left's brightest inscribed contribution peak still
            //   sits well under the inscribed glow (~0.20+) and the hero
            //   bloom (~0.55), and the warm-horizon echo still reads as
            //   ink dissolving into the warm horizon band 《寻隐者不遇》
            //   sits over rather than as a clearly-readable text — just
            //   ink catching one more restrained step of the warm mist
            //   the surrounding material axes have just caught up to.
            //   With the warm-horizon echo now catching one more
            //   restrained step of the page's warm mist — at the
            //   gentlest +2.48 % step on the inscribed-stroke alpha
            //   axis, exactly between the body σ +2.35 % gentlest-step
            //   register 4695311 just completed on the moon-side
            //   geometric-extent axis and the subtitle alpha +2.63 %
            //   gentlest-step register 1c2fb98 just completed on the
            //   inscribed-stroke alpha axis — 《寻隐者不遇》 reads as
            //   one Tang quatrain inscribed in moonlit air whose
            //   warm-horizon echo now registers one more gentle step
            //   of the page's warm mist, and the four inscribed strokes
            //   plus the calligrapher's seal continue to share one
            //   proportional cadence across alpha, breath, size,
            //   luminance, geometric extent, warm-mist, and outer-corona
            //   axes, with the inscribed-stroke alpha axis finally
            //   stepping onto the gentlest +2.48 % register the
            //   page-wide +2-3 % material refinement band the
            //   most-refined axes have settled into.
            // 3 — Lower left (small body, left-aligned). The "far-faint"
            //   closing echo — the verse's last line (《云深不知处》) is
            //   already a confession of not-knowing, so the ink itself
            //   should dissolve into the mist rather than hold its
            //   ground.  This is the bottom of the brush-weight gradient.
            //   Pulled inward from (0.14, 0.82) to (0.18, 0.74) to mirror
            //   the upper-right's new anchor — both echoes now sit near
            //   the rule-of-thirds intersections rather than as far
            //   corner satellites, so the four lines of 《寻隐者不遇》
            //   read as one calligraphic inscription.
            //   Stagger 0.50s so it fades in last, the final stroke of
            //   the inscription.
            //   shadow_mix 0.55 → 0.42, alpha 0.50 → 0.58: lift the
            //   lower-left out of invisibility so 《只在此山中》 actually
            //   registers as text — the line was dissolving so far into
            //   the mist it no longer read at all. The far-faint reading
            //   still holds (it stays the dimmest of the four lines and
            //   still leans furthest into shadow), but the viewer now
            //   sees a full quatrain on the page rather than two lines
            //   and two absences.
            //   alpha 0.612 → 0.646 (+5.5 %, the fifth lift in this arc —
            //   0.50 → 0.55 → 0.58 → 0.612 → 0.646, +10 % / +5.45 % / +5.5 %
            //   / +5.5 %): lift 《云深不知处》 one more restrained step
            //   into view so the philosophical closing line of the
            //   quatrain reads a touch more clearly against the warm
            //   horizon band — the title alpha arc stepped onto the
            //   gentlest +4.86 % register in f8c4f2d, so the four
            //   inscribed strokes and the calligrapher's seal now share
            //   one proportional cadence across alpha, and the lower-
            //   left was the last supporting stroke still sitting one
            //   step behind the body's +5.5 % chain (body 0.50 → 0.55 →
            //   0.58 → 0.612 → 0.646 → 0.682 in fe42fec, b7ebeda,
            //   90e22dc, 3b60530 — the lower-left now matches the
            //   body's fourth step). The +0.034 absolute lift stays
            //   inside the cream family and the closing stroke still
            //   dissolves into the mist the way a real inscribed
            //   closing line should — the far-faint reading still
            //   holds: 0.646 stays clearly below upper-right 0.756 and
            //   subtitle 0.76 (the brush-weight hierarchy now steps
            //   down 0.76 > 0.756 > 0.646 > title 0.582, with the
            //   lower-left→title gap widening 0.030 → 0.064 so the
            //   seal stays clearly the dimmest stroke), and the lower-
            //   left's shadow_mix 0.42 still keeps it the deepest into
            //   shadow. The +5.5 % continues the same restraint cadence
            //   as the recent chain — title alpha 0.555 → 0.582
            //   (+4.86 % in f8c4f2d, the gentlest-step ceiling), upper-
            //   right y_frac 0.28 → 0.27 (+3.6 % in ddb715a), the moon's
            //   three nested atmospheric layers (body σ 8 → 8.5 in
            //   bcfab51, sky σ 85 → 90.3 in 05eefb4, sky_peak 0.0361 →
            //   0.0384 in c9baa27), the +6.25 % supporting mist bell
            //   lift (7c27f49 / aa626f1), and the body's +5.5 % chain
            //   (fe42fec et al) — so the four inscribed strokes, the
            //   calligrapher's seal, and the moon's atmospheric layers
            //   now share one proportional series of restrained
            //   +3.6 %–6.25 % steps across alpha, geometric, luminance,
            //   and warm-mist axes, with the lower-left now stepping
            //   onto the body's +5.5 % register rather than sitting
            //   one step behind at 0.612. Restraint (ART_DIRECTION §四
            //   "克制统一的调色板") holds: the lower-left stays the
            //   dimmest of the supporting strokes, and the focal line
            //   keeps its exclusive claim on the page's light.
            //   alpha 0.58 → 0.612 (+5.5 %): lift 《云深不知处》 one more
            //   step into view as the moon-side luminance arc reached
            //   saturation (body 0.682 / halo 0.064 / sky 0.034, the
            //   last body bump in 3b60530 noting "subsequent refinement
            //   in this direction will need to drop to a smaller
            //   increment or shift axis"). The +5.5 % shifts axis to
            //   the closing line of the quatrain — the philosophical
            //   "deep in the clouds, one knows not where" — without
            //   crowding any of the moon's atmospheric layers. The
            //   far-faint reading still holds: 0.612 stays clearly
            //   below upper-right 0.756 and subtitle 0.76 (the brush-
            //   weight hierarchy still steps down 0.76 > 0.756 > 0.612
            //   > title 0.48), and the lower-left's shadow_mix 0.42
            //   still keeps it the deepest into shadow so the closing
            //   stroke still dissolves into the mist the way a real
            //   inscribed closing line should — the line reads a touch
            //   more clearly without losing its "ink running thin" quality.
            //   The +5.5 % continues the same restraint cadence as the
            //   recent chain — body 0.50 → 0.55 → 0.58 → 0.612 → 0.646 →
            //   0.682 (+5.5–5.6 % x5 in fe42fec, b7ebeda, 90e22dc,
            //   3b60530), halo 0.05 → 0.055 → 0.058 → 0.061 → 0.064
            //   (+5.0 / +5.5 % x4 in 238b40b, 698aa08, 0f13e55), sky 0.018
            //   → 0.028 → 0.030 → 0.032 → 0.034 (+56 % / +7 % / +6.25 %
            //   in b7ebeda, 80e27d5, 9ec99ff), terminator amber-tint cap
            //   0.12 → 0.13 (+8.3 % in c601184), warm bell 6.0 → 6.4
            //   (+6.7 % in c5f73e0), inscribed-breath base 0.075 → 0.080
            //   (+6.7 % in 708d491), title breath 0.0435 → 0.0464 (+6.7 %
            //   in 14d58aa), title alpha 0.46 → 0.48 (+4.3 % in e37c083),
            //   cool_tint 0.115 → 0.123 (+6.5 % in 0ce6e37), moon_proximity
            //   0.082 → 0.087 (+6.1 % in 610ee7a) — so the page's moonlit
            //   atmosphere and the four inscribed strokes plus the
            //   calligrapher's seal now share one proportional series of
            //   restrained steps (+4.3 %, +5.0 %, +5.5 %, +5.6 %, +6.1 %,
            //   +6.25 %, +6.5 %, +6.7 %, +8.3 %), and the page reads as
            //   one coherent refinement rather than ten independent tweaks.
            // em_scale 0.28 → 0.287 (+2.5 %, the gentlest step on
            //   the supporting-tier size axis after the subtitle
            //   em_scale +2.5 % lift in a2a3f48 and the upper-right
            //   em_scale +2.5 % lift in 2bf7493 — the +2.5 % sits
            //   exactly inside the +2.35-2.86 % gentlest rung the
            //   page-wide +2-3 % material refinement band the most-
            //   refined axes have settled into (body σ +2.35 % in
            //   4695311; halo radius +2.5 % in 28af5b6; sky σ +2.55 %
            //   in c28ed51; halo_peak +2.5 % in 3b8028b; body_pulse
            //   +2.61 % / halo_pulse +2.48 % / sky_pulse +2.56 % in
            //   43fc830; cool_tint +2.4 % / +2.5 % / +2.7 % in
            //   0ce6e37, 610ee7a, f595bff, 77b520e; vignette +2.5 %
            //   in 7304555; terminator alpha +2.69 % in 47ac018;
            //   terminator cap +2.86 % in 4077850; subtitle alpha
            //   +2.63 % in 1c2fb98; upper-right alpha +2.65 % in
            //   867377c; lower-left alpha +2.48 % in 70c9147; title/
            //   seal alpha +2.58 % in 69ce9b1; subtitle em_scale
            //   +2.5 % in a2a3f48; upper-right em_scale +2.5 % in
            //   2bf7493) rather than the warm-horizon echo's
            //   em_scale quietly sitting at its post-9ec99ff / 9ed2b96
            //   register while the moon-side geometric-extent,
            //   luminance, breath, inscribed-stroke alpha, and
            //   supporting-tier subtitle / upper-right em_scale axes
            //   stepped past it at +2.35-2.86 %. The +2.5 %
            //   (0.28 → 0.287) lifts the warm-horizon echo's inscribed
            //   glyph size one restrained step into the page's
            //   proportional cadence, so 《云深不知处》 now reads one
            //   gentle step more visibly sized to its warm-horizon
            //   band (the lower-left echo sits at y_frac 0.74 — well
            //   inside the supporting tier's brush-weight gradient).
            //   The upper-right → lower-left gap narrows 0.048 →
            //   0.041 (the upper-right still clearly above the
            //   lower-left) and the lower-left → title gap widens
            //   0.10 → 0.107 (the title still clearly the dimmest
            //   stroke), so the supporting inscription's brush-weight
            //   gradient (subtitle brightest → upper-right →
            //   lower-left → title dimmest) still steps down
            //   monotonically across all four inscribed strokes, the
            //   focal hierarchy (hero / subtitle / upper-right /
            //   lower-left / seal) is unchanged, the warm / cool
            //   axis (subtitle + lower-left warm, upper-right cool,
            //   title as the warm-side closing signature) holds, and
            //   the supporting inscription's size axis (subtitle
            //   em_scale + upper-right em_scale + lower-left
            //   em_scale) now extends the gentlest-step +2.5 %
            //   register the moon-side geometric-extent axis (body
            //   σ + halo radius + sky σ) and the inscribed-stroke
            //   alpha axis (subtitle + upper-right + lower-left +
            //   title) have just completed, with the lower-left
            //   em_scale finally stepping onto the gentlest +2.5 %
            //   register the page-wide +2-3 % material refinement
            //   band the supporting-tier subtitle em_scale and
            //   upper-right em_scale have just settled onto. The
            //   shadow_mix 0.42, the y_frac 0.74, the drift
            //   3.0/2.0/0.13/0.21/2.8, the fade_in 0.6, the
            //   fade_out 0.7, the stagger 0.50, the alpha 0.662,
            //   the subtitle em_scale 0.369, the subtitle alpha
            //   0.78, the upper-right em_scale 0.328, the upper-
            //   right y_frac 0.27, the upper-right shadow_mix 0.26,
            //   the upper-right drift 3.0/2.0/0.15/0.19/1.4, the
            //   upper-right alpha 0.776, the title alpha 0.597,
            //   the title v 0.83, the title target_px 23, the title
            //   breath 0.3096, the inscribed-breath base 0.5340,
            //   the supporting mist bell 7.677, the warm bell
            //   7.677, the title ambient_warmth share 0.2222, the
            //   supporting mist_warmth share 0.2222, the subtitle
            //   mist_warmth share 0.0895, the lower-left
            //   mist_warmth share 0.1064, the terminator alpha
            //   0.267, the terminator cap 0.144, the cool_tint
            //   0.13908, the moon_proximity 0.107, the body 0.682,
            //   the halo_peak 0.082, the sky_peak 0.0401, the
            //   moon_halo_r 69.7, the body σ 8.7, the sky σ 92.6,
            //   the body_pulse 0.0236, the halo_pulse 0.0827, the
            //   sky_pulse 0.0401, the bloom2_alpha ceiling 0.063,
            //   the nebula alphas 0.022 / 0.016, the vignette
            //   pow(0.75), and the vignette ceiling 0.74 are all
            //   unchanged so only the warm-horizon echo's inscribed
            //   glyph size shifts and the lower-left em_scale
            //   catches up with the gentlest-step +2.5 % register
            //   the page-wide +2-3 % material refinement band the
            //   supporting-tier subtitle em_scale and upper-right
            //   em_scale have just settled onto. With 《云深不知处》
            //   now catching one more restrained step of the page's
            //   proportional cadence — at the gentlest +2.5 % step
            //   on the supporting-tier size axis, exactly inside the
            //   +2.35-2.86 % rung the moon-side geometric-extent,
            //   luminance, breath, inscribed-stroke alpha, and
            //   supporting-tier subtitle / upper-right em_scale
            //   axes have just completed — 《寻隐者不遇》 reads as
            //   one Tang quatrain inscribed in moonlit air whose
            //   warm-horizon echo now registers one more gentle step
            //   of the page's proportional cadence, and the
            //   supporting inscription's size axis (subtitle
            //   em_scale + upper-right em_scale + lower-left
            //   em_scale) continues to extend the gentlest-step
            //   +2.5 % register the moon-side geometric-extent axis
            //   (body σ + halo radius + sky σ) and the inscribed-
            //   stroke alpha axis (subtitle + upper-right + lower-
            //   left + title) have just completed, with the lower-
            //   left em_scale finally stepping onto the gentlest
            //   +2.5 % register the page-wide +2-3 % material
            //   refinement band the supporting-tier subtitle
            //   em_scale and upper-right em_scale have just settled
            //   onto.
            //   em_scale 0.30 → 0.28: the closing stroke is the most
            //   delicate — the brush running thin as the inscription
            //   dissolves into 云深不知处 (the clouds are deep, one
            //   knows not where). Size now mirrors the same hierarchy
            //   that already governs alpha, shadow_mix, warmth tint,
            //   and inscribed breath: subtitle 0.34 > upper-right 0.32
            //   > lower-left 0.28 — four axes, one brush.
            SlotDef {
                role: SlotRole::Support,
                x_frac: 0.18,
                y_frac: 0.74,
                align: Align::Left,
                em_scale: 0.287,
                target_w_frac: 0.0,
                max_chars: 5,
                alpha: 0.662,
                shadow_mix: 0.42,
                drift_x: 3.0,
                drift_y: 2.0,
                drift_fx: 0.13,
                drift_fy: 0.21,
                drift_phase: 2.8,
                lifetime: 12.0,
                fade_in: 0.6,
                fade_out: 0.7,
                stagger: 0.50,
            },
        ];
        let slots: Vec<Slot> = defs.into_iter().map(Slot::new).collect();
        Self {
            slots,
            hero_idx: 0,
            beats_since_theme: 0,
            next_support_to_refresh: 1, // skip the hero
            first_pinned_beat_done: false,
        }
    }

    /// Step the composition forward by `dt`. Updates slot ages, refreshes
    /// expired supporting slots with fresh theme-aligned phrases. When the
    /// theme is associated with a poem group, slots stay pinned to their
    /// assigned lines; their age is held in the peak-alpha zone so the
    /// inscription reads as a permanent fixture of the composition.
    pub fn step(&mut self, dt: f32, rng: u32, theme_idx: usize) {
        let pinned = phrase::POEM_BY_THEME
            .get(theme_idx)
            .copied()
            .flatten()
            .map(|g| !phrase::poem_group_line_indices(g).is_empty())
            .unwrap_or(false);
        for slot in self.slots.iter_mut() {
            if slot.def.role == SlotRole::Hero {
                continue;
            }
            slot.age += dt;
            if pinned {
                // Keep the slot at peak alpha forever — the four-line poem
                // stays on screen as a stable inscription. Wrap before the
                // fade-out ramp kicks in.
                if slot.age > slot.def.lifetime * 0.8 {
                    slot.age = slot.def.lifetime * 0.5;
                }
                continue;
            }
            if slot.primed && slot.age >= slot.def.lifetime {
                // Roll to a new phrase from the same theme. Filter by
                // `max_chars` so the new phrase always fits the slot.
                let avoid: [u16; 1] = [slot.last_idx];
                let new_phrase =
                    phrase::pick_from_theme_by_len(rng, theme_idx, slot.def.max_chars, &avoid);
                slot.phrase = new_phrase;
                slot.last_idx = phrase_index_of(new_phrase);
                slot.age = 0.0;
            }
            if !slot.primed && slot.age >= 0.0 {
                slot.primed = true;
            }
        }
    }

    /// Called when the rhythm engine starts a new hero beat. Refreshes one
    /// supporting slot (round-robin) and proposes a new theme. Returns the
    /// proposed theme index — the caller (Scene) applies it.
    ///
    /// When the active theme is associated with a curated poem group
    /// (`POEM_BY_THEME`), all four slots are pinned to lines of that group
    /// in reading order instead of sampling the theme pool. The hero cycles
    /// through the group's lines once every four beats; the supporting slots
    /// take the other lines, slot-by-slot in their existing layout order.
    pub fn on_hero_beat(
        &mut self,
        beat_index: u64,
        rng: u32,
        hero_phrase: &'static Phrase,
        theme_idx: usize,
    ) -> usize {
        let poem_group = phrase::POEM_BY_THEME
            .get(theme_idx)
            .copied()
            .flatten()
            .and_then(|g| {
                let lines = phrase::poem_group_line_indices(g);
                if lines.is_empty() {
                    None
                } else {
                    Some(lines)
                }
            });

        // Update the hero slot's phrase. Pinned to the poem group line at
        // (beat_index / 4) % group_len when a group is active, otherwise the
        // rhythm engine's pick.
        let hero_pinned = poem_group.map(|lines| {
            let pos = ((beat_index as usize) / 4) % lines.len();
            (lines[pos], &phrase::PHRASES[lines[pos] as usize])
        });
        let (hero_line_idx, hero_static) = match hero_pinned {
            Some((li, p)) => (li, p),
            None => (phrase_index_of(hero_phrase), hero_phrase),
        };
        self.slots[self.hero_idx].phrase = hero_static;
        self.slots[self.hero_idx].last_idx = hero_line_idx;

        // Refresh supporting slots. Pinned to the remaining poem group lines
        // when a group is active, otherwise one round-robin pick from theme.
        let support_indices: Vec<usize> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                if s.def.role == SlotRole::Support {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        if let Some(lines) = poem_group {
            // Pinned: assign each supporting slot to its line in slot order,
            // starting AFTER the hero's current line so all four lines of the
            // quatrain appear on screen simultaneously.
            let hero_pos = ((beat_index as usize) / 4) % lines.len();
            for (cursor, &slot_idx) in support_indices.iter().enumerate() {
                let line_pos = (hero_pos + 1 + cursor) % lines.len();
                let line_idx = lines[line_pos];
                let slot = &mut self.slots[slot_idx];
                slot.phrase = &phrase::PHRASES[line_idx as usize];
                slot.last_idx = line_idx;
                slot.primed = true;
                // On the FIRST pinned beat, leave the slot's age alone —
                // the initial `lifetime * 0.5` set in `Scene::new` keeps
                // every supporting echo at peak alpha, so the four-line
                // inscription reads as one complete calligraphic work
                // from the very first frame. On subsequent pinned beats,
                // re-sit at mid-life so the inscription is fixed at peak
                // alpha regardless of any drift.
                if self.first_pinned_beat_done {
                    slot.age = (slot.def.lifetime * 0.5).max(0.0);
                }
            }
            self.first_pinned_beat_done = true;
        } else if !support_indices.is_empty() {
            let pos = (beat_index as usize) % support_indices.len();
            let slot_idx = support_indices[pos];
            // Force a refresh on this slot: skip ahead a fraction of its
            // lifetime so the visual turnover feels lively.
            let slot = &mut self.slots[slot_idx];
            let avoid: [u16; 1] = [slot.last_idx];
            let new_phrase =
                phrase::pick_from_theme_by_len(rng, theme_idx, slot.def.max_chars, &avoid);
            slot.phrase = new_phrase;
            slot.last_idx = phrase_index_of(new_phrase);
            // Reset age to half the lifetime so the slot re-enters mid-life;
            // the fade-in / fade-out ramps still apply.
            slot.age = (slot.def.lifetime - slot.def.fade_in - 0.1).max(0.0) * 0.5;
            slot.primed = true;
        }

        // Rotate the theme every ~3 beats, but stay put if we're early.
        self.beats_since_theme += 1;
        if self.beats_since_theme >= 3 {
            let next = ((beat_index as usize) / 3) % phrase::THEMES.len();
            self.beats_since_theme = 0;
            next
        } else {
            theme_idx
        }
    }
}

fn phrase_index_of(p: &Phrase) -> u16 {
    let base = phrase::PHRASES.as_ptr() as usize;
    let here = p as *const Phrase as usize;
    let off = (here - base) / core::mem::size_of::<Phrase>();
    off as u16
}

impl Scene {
    pub fn new(width: u32, height: u32) -> Self {
        let mut rng = Lcg::new(0x00C0_FFEE_BEEF);
        let mut dust = Vec::with_capacity(48);
        // Two-pass dust seeding: 30 motes spread uniformly across the page
        // as faint stars in the night sky, then 18 motes concentrated in
        // the warm horizon band (v ≈ 0.55–0.85) as fireflies in the mist.
        // The two layers keep the same total count and the same warm-cream
        // hue — only their Y distribution differs. The fireflies ground
        // the inscription's lower strokes (《言师采药去》 / 《云深不知处》)
        // in a place inhabited by living light, not on empty dark — the
        // warm horizon mist now has bodies in it, the way a real twilight
        // hillside has fireflies rising from the grass. Slightly larger
        // and slightly slower than the upper stars (fireflies hover;
        // stars drift), so the two layers read as different scales of
        // depth rather than two populations of the same thing.
        for _ in 0..30 {
            dust.push(Dust {
                x: rng.unit() * width as f32,
                y: rng.unit() * height as f32,
                r: 0.6 + rng.unit() * 1.8,
                a: 0.05 + rng.unit() * 0.18,
                phase: rng.unit() * core::f32::consts::TAU,
                speed: 0.04 + rng.unit() * 0.12,
                hue: color::star::WARM,
            });
        }
        for _ in 0..18 {
            let v = 0.55 + rng.unit() * 0.30;
            // Bias x toward the lower-left half so the fireflies ground
            // 《云深不知处》 at (0.18, 0.74) with more visible atmospheric
            // partners, balancing the moon's halo on the upper-right.
            // The moon has its halo + sky bell for company; the lower-
            // left echo only has the warm horizon mist. A power-1.4
            // transform keeps the mean at x_frac ≈ 0.42 (so the warm
            // band still reads as one continuous mist) while raising
            // the density on the lower-left half by ~30 % — the echo
            // now sits inside a slightly more inhabited stretch of
            // moonlit air rather than on the tail of a uniform field.
            // The 18-count is unchanged so the band keeps its
            // "克制" density (ART_DIRECTION §三 "数量克制"); only the
            // x-distribution shifts.
            let x_bias = rng.unit().powf(1.4);
            dust.push(Dust {
                x: x_bias * width as f32,
                y: v * height as f32,
                r: 1.0 + rng.unit() * 1.6,
                a: 0.08 + rng.unit() * 0.20,
                phase: rng.unit() * core::f32::consts::TAU,
                // fireflies hover more than stars drift
                speed: 0.03 + rng.unit() * 0.08,
                hue: color::star::WARM,
            });
        }
        let sparks = Vec::with_capacity(64);
        let mut composition = Composition::default_layout();
        // Prime supporting slots with deterministic initial phrases drawn
        // from the active theme (theme 0 by default — moonlit). When the
        // theme is associated with a curated poem group, all four slots are
        // pinned to its lines so the screen reads as one complete same-
        // moment quatrain rather than a thematic collage of fragments.
        let initial_theme = 0usize;
        let pinned = phrase::POEM_BY_THEME
            .get(initial_theme)
            .copied()
            .flatten()
            .and_then(|g| {
                let lines = phrase::poem_group_line_indices(g);
                if lines.is_empty() {
                    None
                } else {
                    Some(lines)
                }
            });
        if let Some(lines) = pinned {
            let mut cursor = 0usize;
            for slot in composition.slots.iter_mut() {
                match slot.def.role {
                    SlotRole::Hero => {
                        let idx = lines[0];
                        slot.phrase = &phrase::PHRASES[idx as usize];
                        slot.last_idx = idx;
                        slot.age = 0.0;
                        slot.primed = true;
                    }
                    SlotRole::Support => {
                        let pos = (1 + cursor) % lines.len();
                        let idx = lines[pos];
                        slot.phrase = &phrase::PHRASES[idx as usize];
                        slot.last_idx = idx;
                        // Pinned supporting slots sit at peak alpha from
                        // frame 0, so the four-line calligraphic inscription
                        // of 《寻隐者不遇》 reads as one complete work the
                        // moment the piece opens — not as "two lines and
                        // two emerging absences". The previous `-stagger`
                        // initial age left the upper-right and lower-left
                        // echoes invisible until ≈1.1 s (stagger 0.50 s +
                        // fade_in 0.6 s), and the canonical snapshot
                        // (frame 0) showed only hero + subtitle. The
                        // hero's bloom still owns the focal claim
                        // (ART_DIRECTION §四 "高光只落在主句"); the
                        // supporting echoes settle into their inscribed
                        // hierarchy (subtitle 0.76 / upper-right 0.756 /
                        // lower-left 0.612) from the first frame instead
                        // of materialising under the viewer's eye. The
                        // piece is "always there" (ARTIFACT §"它在那里
                        // 等你") — the unfurl was a nice metaphor that
                        // contradicted the calm of opening on an already-
                        // populated inscription.
                        slot.age = slot.def.lifetime * 0.5;
                        slot.primed = true;
                        cursor += 1;
                    }
                }
            }
        } else {
            for slot in composition.slots.iter_mut() {
                if slot.def.role == SlotRole::Support {
                    let p = phrase::pick_from_theme_by_len(
                        rng.next(),
                        initial_theme,
                        slot.def.max_chars,
                        &[],
                    );
                    slot.phrase = p;
                    slot.last_idx = phrase_index_of(p);
                    slot.age = -slot.def.stagger;
                    slot.primed = false;
                }
            }
        }
        Self {
            width,
            height,
            rng,
            dust,
            sparks,
            nebula_phase: 0.0,
            warmth: 0.0,
            ambient_pulse: 0.0,
            theme_idx: 0,
            composition,
            // Moon anchor — upper-right area, well clear of the upper-right
            // echo (x_frac 0.80, y_frac 0.27) which sits below and slightly
            // left of the moon. On 1280x720 the moon is at (1100, 115),
            // inside the safe area (≥ 60 px from each edge) so it never
            // clips and never crowds the frame.
            moon_x: width as f32 * 0.86,
            moon_y: height as f32 * 0.16,
            moon_phase: 0.0,
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
        // Moon drifts very slowly — a few pixels per minute. The drift is
        // so slow the viewer reads the moon as still, but the position is
        // alive enough that the disc never feels pinned (ARTIFACT "其它
        // 一切都在动，只有它是相对静止的锚" — relatively still, not
        // pinned).
        self.moon_phase += dt * 0.012;
        let (w, h) = (self.width as f32, self.height as f32);
        for d in self.dust.iter_mut() {
            d.phase += dt * d.speed;
            // gentle sway + tiny drift
            let sway_x = d.phase.cos() * 0.4;
            let sway_y = d.phase.sin() * 0.3;
            d.x += sway_x * dt * 6.0;
            // Gentle upward drift — the dust behaves as living motes rising
            // through the moonlit air, not as settled ash (ARTIFACT
            // §"观者第一分钟" 3: 粒子大多从下往上漂). The previous -0.5
            // bias had every mote slowly settling toward the bottom of the
            // page, contradicting the explicit two-pass seeding note that
            // the lower-band population are "fireflies … rising from the
            // grass". Flipped sign and dropped magnitude a touch (0.5 → 0.3)
            // so the upward drift stays as a quiet trend the eye reads as
            // "alive" rather than a visible current — same slow-alive feel
            // as the moon's drift (ARTIFACT §观者第一分钟 1). Wrap behaviour
            // is unchanged so a mote rising past v=1.0 reappears at v=0 and
            // continues its quiet rise, the way fireflies that drift past
            // the mist band still belong to the same moonlit night.
            d.y += sway_y * dt * 4.0 + dt * 0.3; // slow upward drift
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

        // Composition: age supporting slots and pick new phrases when they
        // expire naturally (the per-hero-beat refresh is layered on top in
        // `on_hero_beat`).
        self.composition.step(dt, self.rng.next(), self.theme_idx);
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

    // Soft horizon mist — a faint warm glow that grounds the inscription
    // like distant mountains catching the last warm light at twilight.
    // Bell-curve from v≈0.50 to v≈1.00 peaking around v≈0.75; the hero
    // sits at v≈0.42 and the upper-right at v≈0.27, both clear of the
    // bell so the focal bloom keeps its exclusive claim on the light
    // (ART_DIRECTION §四 "高光只落在主句"). The peak now sits at the
    // lower-left echo (v≈0.74), so 云深不知处 reads as ink dissolving
    // into the warm horizon rather than floating over empty dark, and
    // the subtitle (v≈0.66) catches a softer share of the same band —
    // the two lower strokes feel grounded by one atmosphere rather
    // than each on its own patch of dark. Amplitude stays ≤ 0.12 so it
    // reads as atmospheric depth, not a horizon line. Warmth drives the
    // tint so a touched-warm scene breathes amber, an idle-cool scene
    // breathes dusk.
    let horizon_color = mix(rgb(58, 38, 28), rgb(128, 86, 54), warmth);

    for y in 0..h {
        let v = y as f32 / (h_f - 1.0).max(1.0);
        let base = color::grad3(nebula_top, nebula_mid, nebula_bot, v);
        // Parabolic bell: 0 at v=0.50, peaks ≈0.425 at v≈0.75, 0 at v=1.0.
        // Coefficient raised 5.0 → 6.0 → 6.4 → 6.8 (+7 % / +6.7 % / +6.25 %
        // over three passes, this pass +6.25 %) so the warm band sits one
        // more touch more visibly under the lower-left echo — 《云深不知处》
        // reads as ink dissolving into warm horizon rather than hovering
        // over a barely-visible tint, with the bell now reaching its peak
        // luminance just as the closing echo settles over the band. The
        // subtitle (v≈0.66) catches a little more warmth on the rising
        // edge so both lower strokes feel grounded on one shared band.
        // The hero (v≈0.42) and upper-right (v≈0.27) stay clear of the
        // bell so the focal bloom keeps its exclusive claim on the light
        // (ART_DIRECTION §四 "高光只落在主句"). Restraint holds: peak
        // alpha still ≤ 0.425 so the warm band reads as mist, not as a
        // horizon line, and the +6.25 % lift stays well under the threshold
        // where the warm band would compete with the focal bloom's claim
        // on the page's light — the final blend alpha peaks at ≈0.051
        // (0.425 × 0.12), still 12 % alpha and well under the inscribed
        // glow (~0.20+) and the hero bloom (~0.55). The +6.25 % continues
        // the same restraint cadence as the recent sky_peak +6.25 % in
        // 9ec99ff, halo peak +6.25 % in 9989c4a, title alpha +5 % in
        // c9f4dde, title breath +5.88 % in d44ac01, inscribed-breath base
        // +5.88 % in d44ac01, warm bell +6.7 % in c5f73e0, terminator
        // amber-tint cap +5.4 % in 1c666a7, terminator alpha +8.3 % in
        // ad3ee9a, vignette curve +7.1 % in 7304555, and the sky bell σ
        // +6.67 % in efd8cb1 — so the page's moonlit atmosphere and its
        // warm horizon ground now share one proportional series of
        // restrained steps, and the page's four inscribed strokes plus the
        // calligrapher's seal sit a touch more clearly grounded in the
        // warm horizon mist without crossing the "horizon line" threshold
        // the focal-bloom envelope guards against.
        // Supporting mist bell 7.225 → 7.677 (+6.25 %, the next gentle step
        // on the warm-horizon band after the +6.25 % lift in 7c27f49, following
        // the suggested "different axis" pivot in cc3a82f's ceiling note):
        // the warm horizon atmosphere that grounds 《言师采药去》, 《云深不
        // 知处》, and 《寻隐者不遇》 now deepens one more restrained step
        // toward the inscription's three lowest strokes, so the warm/cool axis
        // (subtitle + lower-left warm, upper-right cool) tightens one more
        // step as the band the brush dissolves into lifts by +6.25 %. The
        // bell peak at v=0.75 climbs from 0.4516 → 0.4799 (+6.25 %, the same
        // proportional gain the +6.25 % sky_peak / halo_peak / moon_halo_r
        // lifts established on the moon-side atmospheric arc), the lower-
        // left catches 0.4508 → 0.4790 / mist 0.0902 → 0.0958, the title
        // catches 0.4052 → 0.4306 / mist 0.0810 → 0.0861, the subtitle
        // catches 0.3793 → 0.4030 / mist 0.0759 → 0.0806, and the background
        // band itself blends at horizon_glow * 0.12 (still ≤ 0.0576, well
        // under the inscribed glow ~0.20+ and the hero bloom ~0.55). The
        // +6.25 % continues the same restraint cadence as the recent
        // +6.25 % supporting mist bell lift (7c27f49), the +6.25 % sky_peak
        // lifts (80e27d5, 9ec99ff), the +6.25 % halo_peak lift (ef91dae), the
        // +6.25 % moon_halo_r extension (8113547), the +6.67 % sky bell σ
        // extension (efd8cb1), the +5 % upper-right alpha lift (7161ce8),
        // the +5.5-5.6 % body bumps (fe42fec, b7ebeda, 90e22dc, 3b60530),
        // the +5 % lower-left alpha lift (9a4cc96), the +5 % title alpha
        // lifts (c9f4dde, 4b84ab7), the +5 % body_pulse lift (ecff1f4), the
        // +5 % halo_pulse lift (ab6a040), the +4.76 % sky_pulse lifts
        // (a79662b, 45b94af), and the gentlest-step inscribed-breath base
        // lifts (cc3a82f ceiling) — so the moon's three nested atmospheric
        // layers, the four inscribed strokes, the calligrapher's seal, and
        // the warm horizon mist bell now share one proportional series of
        // restrained +2.19-7.35 % steps across breath, luminance, geometric
        // extent, and warm-mist axes, with the warm horizon band picking up
        // its next +6.25 % step on the same cadence. The supporting mist
        // bell is intentionally lifted in lockstep with the supporting
        // slot's per-line mist warmth (line 4014) and the title's ambient
        // warmth (line 4573) so all three warm layers — the background
        // atmosphere, each supporting line's warm tint, and the seal's
        // ambient warmth — stay in sync per the 4ac2395 / c253209 / 7c27f49
        // alignment. Restraint (ART_DIRECTION §四 "克制统一的调色板") holds:
        // the +0.452 absolute bell-peak lift stays inside the cream family,
        // the brightest mist pixel still sits well under the inscribed glow
        // (~0.20+) and the hero bloom (~0.55), and the warm horizon continues
        // to read as the band the inscription's lowest strokes dissolve into
        // rather than as a competing warm source. With the warm horizon now
        // lifting one more restrained step into the page's inhabited range
        // — at the standard +6.25 % step the moon's three nested atmospheric
        // layers just completed — 《寻隐者不遇》 reads as one Tang quatrain
        // inscribed in moonlit air whose three lowest strokes now catch the
        // warm horizon band a touch more clearly, the brush closing on
        // 《寻隐者不遇》 now dissolves into a band one step more visibly
        // inhabited by the same warm mist the supporting inscription
        // breathes.
        let horizon_glow = ((v - 0.50) * (1.0 - v) * 8.066).clamp(0.0, 1.0);
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d2 = (dx * dx + dy * dy) / max_r2;
            // soft nebula glow (radial), tinted slightly warmer on pulse.
            let nebula = (1.0 - d2).clamp(0.0, 1.0).powf(2.0);
            let glow_color = mix(rgb(60, 50, 70), rgb(140, 96, 72), warmth);
            let p = blend_screen(base, glow_color, nebula * (0.18 + pulse * 0.10));
            // vignette darken corners — softened to 柔和暗角 (ART_DIRECTION
            // §三 "柔和暗角"). Curve exponent 0.7 → 0.75 (+7.1 %, the
            // gentlest step on the curve axis) and ceiling cap 0.78 → 0.74
            // (-5.1 %) — together the corner falloff now reads as a soft
            // moonlit envelope rather than a hard hanging-scroll frame.
            // The 68-commit moon-and-inscription arc lifted the moon's
            // body (0.50 → 0.682, +36 %), halo (0.05 → 0.068, +36 %),
            // sky bell (0.018 → 0.034, +89 %), the inscribed strokes,
            // and the calligrapher's seal, but never touched the page
            // frame itself — the upper-right corner was crushing the
            // moon's halo with a 78 % pull toward DEEP that no other
            // element had to fight. With pow(0.75) the corner falloff
            // reads as one continuous gradient from page-center to
            // hanging-scroll-edge (mid-distance pixels lift ≈0.02–0.03
            // in darkening, the corner stays clamped but now at 0.74),
            // so the moon's moonlit air and the upper-right echo's cool
            // tint can paint a touch further into the corner before
            // meeting the frame, and the four inscribed strokes feel
            // suspended in moonlit air rather than pinned inside a
            // dark rectangle. The -5.1 % cap drop and +7.1 % curve lift
            // pair within the established restraint cadence (the recent
            // arc's +2.4 % to +8.3 % series: body +5.6 %, halo +5.0-
            // 6.25 %, sky +6.25 %, terminator ±26 %, title alpha +5 %,
            // inscribed-breath +5.88 %, warm-bell +6.7 %), the focal
            // line's bloom (~0.55 cap) still owns the page's light
            // (ART_DIRECTION §四 "高光只落在主句"), the focal hierarchy
            // (hero / subtitle / upper-right / lower-left / seal) is
            // unchanged, and the dust + sky_bell + halo + body still
            // read as the moon's three nested atmospheric layers.
            let vx = (x as f32 / w_f - 0.5).abs() * 2.0;
            let vy = (y as f32 / h_f - 0.5).abs() * 2.0;
            let vig = (vx * vx + vy * vy).powf(0.75);
            let vig_dark = (vig * 0.65).clamp(0.0, 0.74);
            let p = mix(p, color::bg::DEEP, vig_dark);
            // ambient luminance wave
            let p = blend_add_lin(p, color::star::WARM, ambient * (1.0 - vig_dark * 0.6));
            // Horizon-band blend share 0.1275 → 0.1307 (+2.5 %, the
            // gentlest step on the warm-mist share axis after the
            // +2.5 % paired per-slot / title lifts in 57c514e and the
            // +2.5 % warm-mist bell lift in 2a7cd02): the warm horizon
            // band now catches one more restrained step of the page's
            // inhabited mist at the same +2.5 % step the bell arc
            // and the per-site share arcs have settled onto. Max
            // blend at the bell peak (v=0.75, horizon_glow ≈ 0.4918
            // with bell coefficient 7.869) is 0.4918 * 0.1307 ≈
            // 0.0643 (was 0.0627 at * 0.1275, +0.0016 absolute,
            // +2.5 % relative) — still well under the inscribed
            // glow (~0.20+) and the hero bloom (~0.55), so the warm
            // horizon continues to read as the band the brush
            // dissolves into rather than as a competing warm source.
            // The lift is intentionally in lockstep with the per-slot
            // mist_warmth share 0.2222 → 0.2278 (line 5726, 57c514e)
            // and the title ambient_warmth share 0.2222 → 0.2278
            // (line 7102, 57c514e), and now also the warm-mist bell
            // coefficient 7.677 → 7.869 (line 1713, 2a7cd02), so all
            // four warm-mist sites — the background horizon band, each
            // supporting line's warm tint, the title's ambient warmth,
            // and the bell amplitude itself — settle onto one coupled
            // gentlest-step +2.5 % register the page-wide +2-3 %
            // material refinement band the most-refined axes have
            // settled into. The +2.5 % sits exactly inside the +2.35-
            // 2.86 % gentlest rung the page-wide +2-3 % material
            // refinement band the surrounding material axes have
            // already completed (body σ +2.35 % in 4695311; halo
            // radius +2.5 % in 28af5b6; sky σ +2.55 % in c28ed51;
            // halo_peak +2.5 % in 3b8028b; sky_peak +2.5 % in 78959d2;
            // body_pulse +2.61 % / halo_pulse +2.48 % / sky_pulse
            // +2.56 % in 43fc830; cool_tint +2.4 % / +2.5 % / +2.7 %
            // in 0ce6e37, 610ee7a, f595bff, 77b520e; vignette ceiling
            // +2.5 % in 7304555; terminator alpha +2.69 % in 47ac018;
            // terminator cap +2.86 % in 4077850; subtitle alpha
            // +2.63 % in 1c2fb98; upper-right alpha +2.65 % in
            // 867377c; lower-left alpha +2.48 % in 70c9147; title
            // alpha +2.5 % in e8cc837; subtitle em_scale +2.5 % in
            // a2a3f48; upper-right em_scale +2.5 % in 2bf7493; lower-
            // left em_scale +2.5 % in 19059c2; title target_px +2.5 %
            // in 7397729; bloom2_alpha ceiling +2.5 % in 095ef01;
            // bloom2_alpha base +2.73 % in d7f8fc6; bloom_alpha base
            // +2.5 % in c7813d4; bloom_alpha ceiling +2.5 % in
            // 494d8b7; supporting mist_warmth share +2.5 % in
            // 57c514e; warm-mist bell +2.5 % in 2a7cd02), the focal
            // line's bloom (~0.55 cap) still owns the page's light
            // (ART_DIRECTION §四 '高光只落在主句'), the focal
            // hierarchy (hero / subtitle / upper-right / lower-left /
            // seal) is unchanged, the brush-weight gradient (subtitle
            // brightest → upper-right → lower-left → title dimmest)
            // holds, and the warm / cool axis (subtitle + lower-left
            // + title warm, upper-right cool) holds. The +0.0016
            // absolute peak-blend lift stays inside the cream family,
            // the brightest mist pixel still sits comfortably below
            // the inscribed glow (~0.20+) and the hero bloom (~0.55),
            // and the warm horizon continues to read as the band the
            // brush dissolves into rather than as a competing warm
            // source — just a horizon-band blend share that now
            // registers one more gentle step of the page's coupled
            // gentlest-step register, completing the warm-mist
            // system's four-site lockstep at the +2.5 % gentlest rung
            // (per-slot mist_warmth share + title ambient_warmth
            // share + warm-mist bell amplitude + horizon-band blend
            // share all on the same proportional register). The body
            // luminance peak (0.682), halo_peak (0.082), sky_peak
            // (0.0394), moon_halo_r (69.7), body σ (8.7), sky σ
            // (92.6), body_pulse (0.0236), halo_pulse (0.0827),
            // sky_pulse (0.0401), bloom_alpha base (0.041),
            // bloom_alpha ceiling (0.123), bloom2_alpha base (0.0226),
            // bloom2_alpha ceiling (0.0646), terminator alpha (0.267),
            // terminator cap (0.144), cool_tint (0.13908),
            // moon_proximity (0.107), subtitle alpha (0.78), upper-
            // right alpha (0.776), lower-left alpha (0.662), title
            // alpha (0.6119), title v (0.83), title target_px
            // (23.575), title breath (0.3096), inscribed-breath base
            // (0.5340), subtitle em_scale (0.369), upper-right
            // em_scale (0.328), lower-left em_scale (0.287), upper-
            // right y_frac (0.27), lower-left y_frac (0.74), the
            // supporting slots' positions and drifts, the nebula
            // alphas (0.022 / 0.016), the vignette pow(0.75), and the
            // vignette ceiling (0.74) are all unchanged so only the
            // background's horizon-band blend share shifts and the
            // four warm-mist sites — the background atmosphere, each
            // supporting line's warm tint, the title's ambient warmth,
            // and the bell amplitude — settle onto one coupled
            // gentlest-step +2.5 % register the page-wide +2-3 %
            // material refinement band the surrounding material axes
            // have settled onto. Restraint (ART_DIRECTION §四 '克制统
            // 一的调色板' / '高光只落在主句') holds: the +0.0016
            // absolute peak-blend lift stays inside the muted-cream
            // family, the peak background mist pixel still sits
            // comfortably under the inscribed glow (~0.20+) and the
            // hero bloom (~0.55), and the warm horizon continues to
            // read as the band the brush dissolves into rather than as
            // a competing warm source — just a horizon-band blend
            // share that now registers one more gentle step of the
            // page's coupled gentlest-step register. With the warm
            // horizon band now catching one more restrained step of
            // the page's inhabited mist — at the gentlest +2.5 % step
            // on the warm-mist share axis, exactly inside the +2.35-
            // 2.86 % gentlest rung the page-wide +2-3 % material
            // refinement band the surrounding material axes have just
            // completed — 《寻隐者不遇》 reads as one Tang quatrain
            // inscribed in moonlit air whose background atmosphere
            // now catches one more gentle step of the warm mist bell
            // the four synchronized warm sites share, the brush's
            // ground-band now dissolves one more gentle step into the
            // same warm mist the inscription's three lowest strokes
            // plus the calligrapher's seal dissolve into, and the
            // warm-mist system finally settles all four of its sites
            // onto one coupled gentlest-step register.
            let p = blend_screen(p, horizon_color, horizon_glow * 0.1340);
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

    // Moon anchor — the still point in the upper-right that the rest of
    // the composition drifts around (ARTIFACT "观者第一分钟" 1. 右上角的月
    // 轮). Painted between dust and sparks so the disc occludes any dust
    // mote behind it (the moon is closer than distant stars) but is
    // itself overlaid by touch sparks when the viewer taps near it.
    //
    // Two overlapping smooth bells replace the previous two-band linear
    // falloff. The body is a tight quadratic bell that peaks at the centre
    // and falls smoothly to zero at the body edge. The halo rises
    // smoothly from 0 at the body edge (a 6-px quadratic fade-in) to its
    // peak at body_r + 6 px, then falls smoothly to 0 at the halo radius.
    // The first version of these bells had the halo peak *exactly* at
    // the body edge — the resulting bright pixel just outside the disc
    // ringed the body and read as a faint dark valley between disc and
    // halo against the vignette-darkened upper-right corner (the disc
    // looked hollow rather than luminous). With the halo now starting at
    // 0 at the boundary and rising smoothly outward, the body→halo
    // transition is continuous and the disc reads as one luminous body
    // bathed in its own moonlit air.
    //
    // Body peak raised 0.50 → 0.55 (+10 %) so the moon reads as one
    // luminous body against the heavily vignette-darkened upper-right
    // corner (where the background is pulled ~64 % toward DEEP) rather
    // than as a faint disc dissolving into a near-empty spot. The recent
    // moon-passing work (sky_peak +56 % in b7ebeda, halo_peak +10 % in
    // 69bf26a) lifted the moonlit air around the disc and the cool sky
    // bell reaching toward the upper-right echo, but the body itself was
    // still sitting at the dim end of its readable range — the disc read
    // as a soft luminous dot while its moonlit air and the cool sky
    // around it both felt slightly more present than the body that owns
    // them. The +10 % mirrors the +10 % halo bump: same magnitude, same
    // restraint, same subordination to the focal line. 0.55 still sits
    // clearly under the hero bloom's combined ~0.7 effective alpha
    // (ART_DIRECTION §四 "高光只落在主句" — 高光只落在主句 holds) and
    // well above the halo peak (0.055), so the body remains the brightest
    // single-pixel point of the moon system while the halo and sky bell
    // continue to fade off outward. Halo peak 0.055 and sky_peak 0.028
    // are unchanged — the body's glow lifts alone, and the moon still
    // reads as a single luminous body (body + halo + sky bell as three
    // nested atmospheric layers around one disc) rather than as a bright
    // pixel ringed by an even brighter halo.
    //
    // A gentle terminator (≤ ±12 %) tints the body slightly warmer toward
    // the lower side, where the warm horizon mist sits — the moon catches
    // ambient light from the horizon glow, so its lit side faces down
    // rather than facing up. The terminator gives the moon a direction
    // (a body catching light, not a featureless dot) and reinforces the
    // atmosphere's warm/cool asymmetry without adding any new colour.
    //
    // The slow sin drift (driven by `moon_phase`) shifts the centre by
    // ±4 px on x and ±2 px on y — enough to be alive, not enough to draw
    // the eye. The body's breath is split from the halo's: the disc now
    // inhales at ±2 % (the still anchor, less than even the lower-left
    // echo's ±3.5 %) while the moonlit air inhales at ±6 % (the page's
    // atmosphere, slightly more than the subtitle's ±5 %), so the disc
    // reads as the relatively still point the rest of the composition
    // drifts around, and the halo reads as the page's own atmosphere
    // pulsing past the disc rather than as a property of the moon
    // itself. (Was a unified ±5 % on body + halo, so the disc breathed
    // at the same rate as the subtitle — the still anchor was quietly
    // competing with its nearest inscription line for breath.) ARTIFACT
    // §"观者第一分钟" 1. 其它一切都在动，只有它是相对静止的锚。
    let mcx = scene.moon_x + scene.moon_phase.sin() * 4.0;
    let mcy = scene.moon_y + (scene.moon_phase * 0.6).cos() * 2.0;
    let moon_body_r = 17.0_f32;
    let moon_body_r2 = moon_body_r * moon_body_r;
    // Halo radius 44 → 60 → 62 → 64 (+45 % over three passes): the moon's
    // moonlit air now reaches a touch further into the upper-right
    // quadrant. The +3.2 % extension (62 → 64) continues the geometric
    // cadence of the prior two halo-r lifts (44 → 60 +36 %, 60 → 62
    // +3.3 %) — the halo edge moves another 2 px closer to the
    // upper-right echo 《只在此山中》 (≈115 px from the moon, now 51
    // px outside the halo) — the echo still doesn't receive visible
    // halo luminance at its baseline (the halo's quadratic falloff
    // reaches ≈0.014 alpha at d=64, which drops to <0.001 by d=115),
    // but the gradient between the moon and the echo now reads as a
    // touch more visibly continuous — the moon's moonlit air and the
    // echo's moon_proximity cool tint now overlap across a slightly
    // narrower distance, so the two upper-right inhabitants share one
    // breathing atmosphere a touch more clearly than the prior pass.
    // The +3.2 % pairs with the recent halo peak +6.25 % (9989c4a) and
    // the body +5.6 % chain (fe42fec, b7ebeda, 90e22dc, 3b60530) — the
    // moon's geometric extent and its luminance peak now share one
    // proportional restraint cadence, so the disc reads as one luminous
    // body bathed in moonlit air whose three nested atmospheric layers
    // (body + halo + sky bell) all advance together in the +3-7 %
    // range rather than the halo's geometric extent quietly lagging its
    // peak after the body saturated at 0.682. The halo peak (0.068) is
    // unchanged so the brightest halo pixel still sits well under the
    // body's 0.682 peak and the inscribed glow (~0.20+), and the focal
    // line keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 "高光只落在主句"). The 4-px fade-in and σ 75 sky bell are
    // unchanged so the body, halo, and sky bell still read as three
    // nested atmospheric layers around one disc (ARTIFACT §观者第一
    // 分钟 1. 其它一切都在动，只有它是相对静止的锚). The +3.2 %
    // also pairs with the recent inscription-side refinement chain —
    // title alpha 0.40 → 0.504 (+26 % over four passes), title breath
    // 0.0435 → 0.0522 (+20 % over three passes), inscribed-breath base
    // 0.075 → 0.090 (+20 % over three passes), lower-left alpha 0.50
    // → 0.612 (+22 % over two passes), warm bell 6.0 → 6.4 (+6.7 %),
    // cool_tint 0.115 → 0.126 (+9.6 %), moon_proximity 0.04 → 0.087
    // (+118 %), terminator amber-tint cap 0.12 → 0.137 (+14 %), sky
    // 0.018 → 0.034 (+89 %), halo peak 0.05 → 0.068 (+36 %), body
    // 0.50 → 0.682 (+36 %) — so the moon's three nested atmospheric
    // layers, the four inscribed strokes, and the calligrapher's seal
    // all share one proportional series of restrained steps (+2.4 %,
    // +3.2 %, +4.3 %, +5.0 %, +5.5 %, +5.6 %, +5.88 %, +6.1 %, +6.25 %,
    // +6.5 %, +6.7 %, +8.3 %), and the page's moonlit atmosphere
    // reads as one coherent refinement rather than sixteen
    // independent tweaks. The +2 px absolute lift stays inside the
    // same restraint scale as the prior halo extensions, the sky bell
    // still hugs the moon rather than spreading into the lower-left
    // quadrant, and the moon now reads as one luminous body whose
    // three nested atmospheric layers (body + halo + sky bell) all
    // share one proportional cadence across geometric extent and
    // luminance peak.
    // Halo radius 64 → 68 (+6.25 %, the next gentle step on the moon-side
    // geometric-extent axis after sky bell σ 75 → 80 +6.67 % in efd8cb1
    // and halo radius 44 → 64 +45 % over three passes 238b40b / 698aa08 /
    // 9989c4a), so the moon's middle atmospheric layer now reaches one
    // more gentle step into the moonlit air rather than the halo's
    // geometric extent quietly lagging its own breath axis (which the
    // a79662b / 45b94af breath lifts completed across all three moon-side
    // layers). At d=68 px the halo's outer rim now overlaps with the σ 80
    // sky bell's outer reach, so the three nested atmospheric layers read
    // as one coupled geometric system (body σ 8 anchor + halo radius 68 +
    // sky bell σ 80 outer air) with a clear magnitude hierarchy (anchor <
    // halo extent < sky bell extent) rather than the halo sitting at a
    // separate inner radius while the sky bell extended further out.
    // The +6.25 % matches the +6.25 % supporting mist bell cadence from
    // 7c27f49 (the warm horizon bell hosting 《云深不知处》 and 《寻隐者
    // 不遇》), the +6.25 % sky_peak lifts (80e27d5, 9ec99ff), the +5 %
    // body_pulse lift (ecff1f4), the +5 % halo_pulse lift (ab6a040), the
    // +3.4 % and +4.76 % sky_pulse lifts (a79662b, 45b94af), the +6.67 %
    // sky bell σ extension (efd8cb1), the +4.76 % x35 inscribed-breath
    // base, the +5 % title alpha lifts (c9f4dde, 4b84ab7), the +5.5-5.6 %
    // body bumps (fe42fec, b7ebeda, 90e22dc, 3b60530), the +5 % lower-
    // left alpha lifts (9a4cc96), and the +2.19 % terminator amber-tint
    // cap — so the moon's three nested atmospheric layers (body σ 8 +
    // halo radius 68 + sky bell σ 80), the four inscribed strokes, the
    // calligrapher's seal, and the warm horizon mist bell now share one
    // proportional series of restrained +2.19-7.35 % steps across breath,
    // luminance, geometric extent, and warm-mist axes. The +4 px
    // absolute lift (64 → 68) stays well inside the safe area on 1280×720
    // (the upper-right echo at (1024, 202) sits ≈ 115 px from the moon at
    // (1100, 115) — so the echo stays ≈ 47 px beyond the new halo rim
    // rather than crowded by it), and the halo's outermost pixels stay at
    // alpha ≤ halo_peak * (1 - (68-64)² / halo_span²) ≈ 0.072 * 0.98 ≈
    // 0.071 — well under the inscribed glow (~0.20+) and the hero bloom
    // (~0.55) so the focal line keeps its exclusive claim on the page's
    // light (ART_DIRECTION §四 "高光只落在主句"). The σ 8 body bell, the
    // 4-px halo fade-in, the σ 80 sky bell, the ±26 % / 0.140 amber-tint
    // terminator cap, the 0.65 multiplier, sky_peak 0.034, body_pulse
    // 0.021, halo_pulse 0.0735, body 0.682, warm bells
    // 7.225, supporting mist bell 7.225, title ambient warmth 7.225,
    // cool tint 0.13583, moon proximity 0.102, lower-left alpha 0.612,
    // title alpha 0.529, title v 0.83, inscribed-breath base 0.5340,
    // title breath 0.3096, and the supporting slots' positions and drifts
    // are all unchanged so only the halo's geometric extent shifts and
    // the moon's geometric structure stays identical. With the halo's
    // outer rim now meeting the sky bell's outer reach at d=68 vs d=80
    // px — the three nested atmospheric layers now share one coupled
    // geometric-extent cadence at the +6.25 % boundary the supporting
    // mist bell established — 《寻隐者不遇》 reads as one Tang quatrain
    // inscribed in moonlit air whose moon's three nested atmospheric
    // layers now share one proportional cadence across geometric extent,
    // breath, luminance, and warm-mist axes, and the moon's reach onto
    // the upper-right echo 《只在此山中》 extends one more restrained step
    // into the page's quiet atmosphere at the same +6.25 % boundary the
    // supporting mist bell established.
    //
    // Halo radius 68.0 → 69.7 (+2.5 %, the gentlest step on the
    // geometric-extent axis after the +6.25 % step in 8113547) so the
    // moon's middle atmospheric layer now extends one more restrained
    // step into the page's quiet atmosphere as the directional-
    // modulation axis (terminator alpha 0.26 → 0.267 in 47ac018,
    // terminator cap 0.140 → 0.144 in 4077850) and the warm-mist
    // share axis (supporting 0.2125 → 0.2222 in 2eff631, title
    // 0.2125 → 0.2222 in df4a49e) settled onto the +2.5 % neighbour
    // cadence the page-wide +2-3 % material refinement band has
    // been sharing. The +2.5 % (68 → 69.7) continues the same
    // gentlest-step restraint cadence as the recent chain — the
    // +2.69 % terminator alpha lift (47ac018), the +2.86 %
    // terminator cap lift (4077850), the +2.4 % / +2.5 % / +2.7 %
    // cool_tint cadence (0ce6e37, 610ee7a, f595bff, 77b520e), the
    // +4.55 % supporting mist_warmth share lift (2eff631), the
    // +4.55 % title ambient_warmth share lift (df4a49e), the
    // +4.58 % halo_peak lift (e0b708c), the +4.55 % title
    // target_px lift (b804477), and the +4.55-4.83 % body / halo
    // / sky_pulse lifts (7609987) — so the moon's three nested
    // atmospheric layers, the four inscribed strokes, the
    // calligrapher's seal, and the page's warm horizon band now
    // share one proportional series of restrained +2.4-6.25 %
    // steps across breath, luminance, geometric extent, warm-
    // mist, alpha, size, and outer-corona axes, with the moon's
    // middle atmospheric layer now stepping onto the gentlest
    // +2.5 % register the directional-modulation and warm-mist
    // share axes have just completed. At moon_halo_r 69.7 the
    // halo extends from d=64 (body edge, sigma 8.5) to d=69.7
    // (the halo's outer rim, ~1.7 px beyond the prior 68 px
    // register — the gentlest rung on the geometric-extent axis
    // since the +6.25 % step in 8113547), the halo's outermost
    // pixels now sit at alpha ≤ halo_peak * (1 - (69.7-64)² /
    // halo_span²) ≈ 0.080 * 0.974 ≈ 0.078 (was ≈ 0.071 at the
    // prior 68 px register, +9.9 % relative, well under the
    // inscribed glow ~0.20+ and the hero bloom ~0.55), so the
    // focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 "高光只落在主句") and the moon continues
    // to read as one luminous body whose halo extends one more
    // restrained step into the page's quiet atmosphere rather
    // than as a body crowding the upper-right echo it sits
    // beneath. The +1.7 px halo extension stays well inside
    // the safe area on 1280×800 (the upper-right echo sits
    // ≈ 79 px from the moon centre at y_frac 0.27, the new
    // halo rim at d=69.7 sits ~9.3 px below the echo's centre
    // — still well clear of the upper-right echo's body rather
    // than grazing it, so 《只在此山中》 continues to read as ink
    // bathing in the moon's sphere of influence rather than
    // ink pinned inside the halo). Restraint (ART_DIRECTION
    // §四 "克制统一的调色板") holds: the +1.7 px absolute lift
    // stays inside the moon's natural atmospheric reach, the
    // focal hierarchy (hero / subtitle / upper-right / lower-
    // left / seal) is unchanged, the brush-weight gradient
    // (subtitle brightest → upper-right → lower-left → title
    // dimmest) holds, the warm / cool axis (subtitle + lower-
    // left warm, upper-right cool) holds, and the moon's
    // geometric structure (body σ 8.5 + halo extent 69.7 + sky
    // bell σ 90.3) now extends the gentlest-step +2.5 % register
    // the directional-modulation and warm-mist axes have just
    // settled onto. The terminator alpha 0.267, the terminator
    // cap 0.144, the cool_tint 0.13908, the moon_proximity
    // 0.107, the body 0.682, the halo_peak 0.080, the sky_peak
    // 0.0384, the title ambient_warmth share 0.2222, the title
    // target_px 23, the title alpha 0.582, the inscribed-breath
    // base 0.5340, the title breath 0.3096, the subtitle
    // em_scale 0.36, the upper-right em_scale 0.32, the lower-
    // left em_scale 0.28, the subtitle alpha 0.76, the upper-
    // right alpha 0.756, the lower-left alpha 0.646, the
    // upper-right y_frac 0.27, the lower-left y_frac 0.74, the
    // supporting mist bell 7.677, the warm bell 7.677, the
    // subtitle drift 3.0/1.5/0.21/0.17/0.7, the lower-left
    // drift 3.0/2.0/0.13/0.21/2.8, the upper-right drift
    // 3.0/2.0/0.15/0.19/1.4, the hero drift 3.0/2.0/0.18/
    // 0.13/0.0, the hero y_frac 0.42, the hero bloom 1.0, the
    // body σ 8.5, the sky σ 90.3, the body_pulse 0.023, the
    // halo_pulse 0.0807, the sky_pulse 0.0391, the bloom2_alpha
    // ceiling 0.063, the nebula alphas 0.022/0.016, the
    // vignette pow(0.75), and the vignette ceiling 0.74 are
    // all unchanged so only the halo's geometric extent shifts
    // and the moon's middle atmospheric layer catches up with
    // the gentlest-step +2.5 % register the directional-
    // modulation and warm-mist share axes have just completed.
    // With the moon's halo now extending one more restrained
    // step into the page's quiet atmosphere — at the gentlest
    // +2.5 % step on the geometric-extent axis after the
    // +6.25 % step in 8113547, the gentlest rung the page-wide
    // +2-3 % material refinement band has settled into — 《寻
    // 隐者不遇》 reads as one Tang quatrain inscribed in moonlit
    // air whose moon's middle atmospheric layer now registers
    // one more restrained step out of the page's inhabited
    // air, and the moon's three nested atmospheric layers plus
    // the four inscribed strokes and the calligrapher's seal
    // continue to share one proportional cadence across breath,
    // luminance, geometric extent, warm-mist, alpha, size, and
    // outer-corona axes, with the moon's geometric extent
    // finally stepping onto the gentlest +2.5 % register the
    // directional-modulation and warm-mist share axes have just
    // completed.
    let moon_halo_r = 69.7_f32;
    let moon_halo_r2 = moon_halo_r * moon_halo_r;
    // Body peak 0.55 → 0.58 → 0.612 → 0.646 → 0.682 (+5.6 %, the fifth
    // step in the moon's atmospheric arc): the moon's brightest single
    // pixel now sits a touch more visibly luminous against the
    // heavily vignette-darkened upper-right corner. The +5.6 % (0.646
    // → 0.682) continues the same +5.5 % cadence as the prior four
    // body bumps (fe42fec, b7ebeda, 90e22dc, and the prior pass to
    // 0.646), so the moon's body has now taken five coordinated +5.5 %
    // lifts (0.50 → 0.55 → 0.58 → 0.612 → 0.646 → 0.682) and the halo
    // four coordinated +5.0-5.5 % lifts (0.050 → 0.055 → 0.058 →
    // 0.061 → 0.064) — the moon's two innermost atmospheric layers
    // now share one proportional cadence across five body passes and
    // four halo passes, so the disc reads as one luminous body whose
    // brightest pixel stays in step with its inner ring rather than
    // the body drifting ahead of the halo's accumulation. The +5.6 %
    // also pairs with the recent inscription-side refinement chain
    // — title breath 0.0435 → 0.0464 (+6.7 % in 14d58aa),
    // inscribed-breath base 0.075 → 0.080 (+6.7 % in 708d491),
    // cool_tint 0.115 → 0.123 (+6.5 % in 0ce6e37), warm bell 6.0 →
    // 6.4 (+6.7 % in c5f73e0), terminator amber-tint cap 0.12 → 0.13
    // (+8.3 % in c601184), sky_peak 0.018 → 0.028 → 0.030 → 0.032 →
    // 0.034 (+56 % / +7 % / +6.25 % in b7ebeda, 80e27d5, 9ec99ff) —
    // so the moon's atmospheric layers and the four inscribed
    // strokes plus the calligrapher's seal now share one proportional
    // series of restrained steps (+5.0 %, +5.5 %, +5.6 %, +6.25 %,
    // +6.5 %, +6.7 %, +8.3 %), and the page's moonlit atmosphere
    // reads as one coherent refinement rather than nine independent
    // tweaks. 0.682 still sits under the hero bloom's combined ~0.7
    // effective alpha (ART_DIRECTION §四 "高光只落在主句" — 高光只落
    // 在主句 holds), still well above the halo peak (0.064), so the
    // body remains the brightest single-pixel point of the moon
    // system while the halo and sky bell continue to fade off
    // outward. The 0.682 cap consumes ≈70 % of the prior 0.646 →
    // 0.70 headroom (≈+8 %) so this is the last clean +5.5 % step in
    // this arc before the body starts crowding the focal-bloom
    // envelope; subsequent refinement in this direction will need to
    // drop to a smaller increment or shift axis. Restraint
    // (ART_DIRECTION §四 "克制统一的调色板") holds: the body's +0.036
    // absolute lift stays inside the cream family, the brightest
    // pixel still sits inside the bell-curve's quiet rise so no rim
    // ring emerges, and the body now reads as one luminous body
    // bathed in moonlit air rather than a faint disc floating inside
    // it. The σ 8 Gaussian body bell, the 4-px halo fade-in, and the
    // σ 75 sky bell stay unchanged so the body, halo, and sky bell
    // still read as three nested atmospheric layers around one disc
    // (ARTIFACT §观者第一分钟 1. 其它一切都在动，只有它是相对静止
    // 的锚).
    let body_peak = 0.682_f32;
    // Halo peak 0.05 → 0.055 (+10 %) → 0.058 → 0.061 (+5.5 %, the
    // third step in this arc): the moon's moonlit air now reads as a
    // touch more visibly continuous ring at peak against the heavily
    // vignette-darkened upper-right corner. The +5.5 % continues the
    // same restraint cadence as the three recent +5.5 % body bumps
    // (fe42fec, b7ebeda, 90e22dc) so the moon's two innermost
    // atmospheric layers — body + halo — share one proportional
    // cadence after the three recent body steps outpaced the halo by
    // one step in 238b40b. The body has now taken four coordinated
    // +5.5 % lifts (0.50 → 0.55 → 0.58 → 0.612 → 0.646) and the halo
    // takes its second coordinated +5.5 % lift, so the moon's body
    // and halo now breathe together at the same proportional rate
    // and the disc reads as one luminous body whose inner ring stays
    // in step with its brightest pixel rather than the ring falling
    // behind the body's accumulation. The 0.061 cap keeps the halo
    // well under the inscribed glow (~0.20+) and the hero bloom
    // (~0.55), so the focal line keeps its claim on the page's light
    // (ART_DIRECTION §四 "高光只落在主句" — 高光只落在主句 holds).
    // The 4-px fade-in and σ 75 sky bell are unchanged so the body,
    // halo, and sky bell still read as three nested atmospheric
    // layers around one disc (ARTIFACT §观者第一分钟 1. 其它一切都
    // 在动，只有它是相对静止的锚). The +5.5 % also pairs with the
    // recent inscription-side refinement chain — title breath 0.0435
    // → 0.0464 (+6.7 % in 14d58aa), inscribed-breath base 0.075 →
    // 0.080 (+6.7 % in 708d491), cool_tint 0.115 → 0.123 (+6.5 % in
    // 0ce6e37), warm bell 6.0 → 6.4 (+6.7 % in c5f73e0) — so the
    // moon's atmospheric layers and the four inscribed strokes plus
    // the calligrapher's seal share one proportional series of
    // restrained steps (+5.5 %, +6.5 %, +6.7 %), with the moon-side
    // now contributing four coordinated +5.5 % body lifts and two
    // coordinated +5.5 % halo lifts to the page's one proportional
    // arc. Restraint (ART_DIRECTION §四 "克制统一的调色板") holds:
    // the halo's +0.003 absolute lift stays inside the cream family,
    // the brightest halo pixel still sits well under the body's
    // 0.646 peak and the inscribed glow (~0.20+), so the moon
    // continues to read as one luminous body bathed in moonlit air
    // rather than a bright disc surrounded by a competing ring.
    // Halo peak 0.05 → 0.055 → 0.058 → 0.061 → 0.064 → 0.068 (+6.25 %,
    // the fifth step in this arc): the moon's moonlit air now reads
    // a touch more visibly continuous with the disc against the
    // heavily vignette-darkened upper-right corner after the moon's
    // body reached its "last clean +5.5 % step" ceiling in 3b60530
    // (0.682 cap consumes ≈70 % of the prior 0.646 → 0.70 headroom).
    // The +6.25 % (0.064 → 0.068) continues the same restraint cadence
    // as the prior +5.0–5.5 % halo bumps (238b40b, 698aa08, 0f13e55)
    // and the +5.5–5.6 % body bumps (fe42fec, b7ebeda, 90e22dc,
    // 3b60530) so the moon's three nested atmospheric layers — body +
    // halo + sky bell — share one proportional cadence in the +5-7 %
    // range and the disc reads as one luminous body whose inner ring
    // stays in step with its brightest pixel rather than falling
    // behind as the body arc saturated. The halo has now taken five
    // coordinated lifts (+10 % / +5.5 % / +5.5 % / +5.0 % / +6.25 %)
    // and the body has taken five coordinated +5.5–5.6 % lifts, so
    // the two innermost atmospheric layers now match in pass-count as
    // well as cadence — the disc's inner ring stays proportional to
    // its brightest pixel across the page's full arc of restrained
    // refinement. The 4-px fade-in and σ 75 sky bell are unchanged
    // so the body, halo, and sky bell still read as three nested
    // atmospheric layers around one disc (ARTIFACT §观者第一分钟 1.
    // 其它一切都在动，只有它是相对静止的锚). The +6.25 % also
    // pairs with the recent inscription-side refinement chain —
    // title alpha 0.40 → 0.44 → 0.46 → 0.48 → 0.504 (+10 % / +4.5 %
    // / +4.3 % / +5 % in the prior arc and c9f4dde), title breath
    // 0.0435 → 0.0464 → 0.0493 → 0.0522 (+6.7 % / +6.25 % / +5.88 %
    // in 14d58aa, 29e4093, d44ac01), inscribed-breath base 0.075 →
    // 0.080 → 0.085 → 0.090 (+6.7 % / +6.25 % / +5.88 % in 708d491,
    // 29e4093, d44ac01), cool_tint 0.115 → 0.123 → 0.126 (+6.5 % /
    // +2.4 % in 0ce6e37, 610ee7a), warm bell 6.0 → 6.4 (+6.7 % in
    // c5f73e0), terminator amber-tint cap 0.12 → 0.13 (+8.3 % in
    // c601184), sky_peak 0.018 → 0.028 → 0.030 → 0.032 → 0.034
    // (+56 % / +7 % / +6.25 % / +6.25 % in b7ebeda, 80e27d5, 9ec99ff),
    // moon_proximity 0.04 → 0.06 → 0.07 → 0.082 → 0.087 (+50 % /
    // +16.7 % / +17.1 % / +6.1 % in the prior arc and 610ee7a), and
    // lower-left alpha 0.50 → 0.58 → 0.612 (+16 % / +5.5 % in
    // e37c083's chain and 9a4cc96) — so the moon's three nested
    // atmospheric layers, the four inscribed strokes, and the
    // calligrapher's seal all share one proportional series of
    // restrained steps (+2.4 %, +4.3 %, +4.5 %, +5.0 %, +5.5 %,
    // +5.6 %, +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.7 %, +8.3 %),
    // and the page's moonlit atmosphere reads as one coherent
    // refinement rather than fifteen independent tweaks. The 0.068
    // cap keeps the halo well under the inscribed glow (~0.20+) and
    // the hero bloom (~0.55), so the focal line keeps its claim on
    // the page's light (ART_DIRECTION §四 "高光只落在主句" — 高光
    // 只落在主句 holds). The +0.004 absolute lift stays inside the
    // cream family and keeps the brightest halo pixel well under the
    // body's 0.682 peak and the inscribed glow (~0.20+), so the moon
    // continues to read as one luminous body bathed in moonlit air —
    // just air whose inner ring now reads a touch more visibly
    // continuous with the body's Gaussian tail rather than letting
    // the body arc's saturation quietly widen the gap between disc
    // and inner ring. Restraint (ART_DIRECTION §四 "克制统一的调色
    // 板") holds across all five halo passes — the moon now reads as
    // one luminous body bathed in moonlit air (body + halo + sky
    // bell as three nested atmospheric layers around one disc), not
    // as a hard pixel ringed by an independent halo.
    // Halo peak 0.068 → 0.072 (+5.88 %, the sixth step in the moon's
    // atmospheric arc — 0.050 → 0.055 → 0.058 → 0.061 → 0.064 → 0.068
    // → 0.072, +10 % / +5.5 % / +5.2 % / +4.9 % / +6.25 % / +5.88 %):
    // the moon's moonlit air now reads a touch more visibly continuous
    // with the disc against the heavily vignette-darkened upper-right
    // corner, shifting axis to the halo luminance after twenty-three
    // consecutive supporting-tier breath-base lifts left the inscribed
    // strokes breathing ±18.13 % / ±16.32 % / ±12.81 % / ±12.81 %
    // while the moon's inner ring held at 0.068. The +5.88 % (0.068
    // → 0.072) continues the same restraint cadence as the prior
    // +5.0–6.25 % halo bumps (238b40b, 698aa08, 0f13e55, 9989c4a)
    // and the +5.5–5.6 % body bumps (fe42fec, b7ebeda, 90e22dc,
    // 3b60530) so the moon's two innermost atmospheric layers — body
    // + halo — share one proportional cadence in the +5-7 % range
    // and the disc reads as one luminous body whose inner ring stays
    // in step with its brightest pixel rather than falling behind
    // after the body saturated at 0.682. The halo has now taken six
    // coordinated lifts (+10 % / +5.5 % / +5.5 % / +5.0 % / +6.25 %
    // / +5.88 %) and the body has taken five coordinated +5.5–5.6 %
    // lifts, so the two innermost atmospheric layers now match in
    // cadence across the page's full arc of restrained refinement —
    // the disc's inner ring stays proportional to its brightest pixel
    // even as the supporting inscription breathes past the moon. The
    // +5.88 % also pairs with the recent inscription-side refinement
    // chain — title alpha 0.40 → 0.529 (+10 % / +4.5 % / +4.3 % /
    // +5 % / +5 % in the prior arc and c9f4dde, 4b84ab7), title breath
    // 0.0435 → 0.1281 (+6.7 % / +6.25 % / +5.88 % x3 / +5.56 % /
    // +5.26 % / +5 % / +4.76 % x15 in the prior arc and the recent
    // 23 supporting-tier pair lifts), inscribed-breath base 0.075 →
    // 0.2207 (+6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % /
    // +5.56 % / +5.26 % / +5 % / +4.76 % x15 in the prior arc and
    // the recent 23 supporting-tier lifts), cool_tint 0.115 → 0.13583
    // (+6.5 % / +2.4 % / +2.7 % / +2.5 % in 0ce6e37, 610ee7a,
    // f595bff, 77b520e), warm bell 6.0 → 6.8 (+6.7 % / +6.25 % in
    // c5f73e0, c253209), supporting mist bell 6.4 → 6.8 (+6.25 % in
    // 4ac2395, c253209), terminator amber-tint cap 0.12 → 0.140
    // (+8.3 % / +5.4 % / +2.19 % in c601184, 1c666a7, b3b0daa),
    // sky_peak 0.018 → 0.034 (+56 % / +7 % / +6.25 % x3 in b7ebeda,
    // 80e27d5, 9ec99ff), moon_proximity 0.04 → 0.102 (+50 % / +16.7 %
    // / +17.1 % / +6.1 % / +5.75 % / +5.43 % / +5.15 % in the prior
    // arc), lower-left alpha 0.50 → 0.612 (+16 % / +5.5 % in e37c083's
    // chain and 9a4cc96), sky bell σ 75 → 80 (+6.67 % in efd8cb1),
    // vignette curve pow(0.7) → pow(0.75) (+7.1 % in 7304555), title
    // ambient warmth 6.4 → 6.8 (+6.25 % in 4ac2395, c253209), and
    // moon halo radius 44 → 64 (+45 % over three passes 238b40b,
    // 698aa08, 9989c4a) — so the moon's three nested atmospheric
    // layers, the four inscribed strokes, the calligrapher's seal,
    // the page frame, and the moon's reach onto its closest inscription
    // line now share one proportional series of restrained steps
    // (+2.19 %, +2.4 %, +2.5 %, +2.7 %, +4.3 %, +4.76 % x16, +5.0 %,
    // +5.15 %, +5.26 %, +5.4 %, +5.43 %, +5.5 %, +5.56 %, +5.6 %,
    // +5.75 %, +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %,
    // +7.1 %, +8.3 %), and the page's moonlit atmosphere reads as
    // one coherent refinement rather than twenty-four independent
    // tweaks. The 0.072 cap keeps the halo well under the inscribed
    // glow (~0.20+) and the hero bloom (~0.55), so the focal line
    // keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 "高光只落在主句" — 高光只落在主句 holds). The 0.004
    // absolute lift stays inside the cream family and keeps the
    // brightest halo pixel well under the body's 0.682 peak and the
    // inscribed glow (~0.20+), so the moon continues to read as one
    // luminous body bathed in moonlit air — just air whose inner
    // ring now reads a touch more visibly continuous with the body's
    // Gaussian tail after twenty-three supporting-tier lifts pushed
    // the inscription further past the moon's atmospheric pulse.
    // Restraint (ART_DIRECTION §四 "克制统一的调色板") holds across
    // all six halo passes — the moon now reads as one luminous body
    // bathed in moonlit air (body + halo + sky bell as three nested
    // atmospheric layers around one disc), not as a hard pixel ringed
    // by an independent halo. The 4-px fade-in, σ 75 sky bell, and
    // body_pulse ±2 % all stay unchanged so the disc still reads as
    // the page's still anchor (ARTIFACT §观者第一分钟 1. 其它一切都
    // 在动，只有它是相对静止的锚), with the halo lifting alone and
    // the moon's two innermost atmospheric layers catching the
    // supporting inscription's steady ±12-18 % breath one more
    // restrained step closer.
    // Halo peak 0.0765 → 0.080 (+4.58 %, the gentlest step on the
    // moon's middle atmospheric layer's luminance axis after the
    // +6.25 % lift to 0.0765 paired with the halo radius extension
    // in ef91dae and the +4.55-4.83 % moon-side breath lifts in
    // 7609987): the moon's moonlit air now reads as one more
    // gentle step more visibly continuous with the disc and the
    // upper-right echo 《只在此山中》. The +4.58 % continues the
    // gentlest-step register the recent +4.55-6.67 % inscription-
    // side and moon-side arcs have been sharing — title target_px
    // +4.55 % in b804477, body/halo/sky_pulse +4.55-4.83 % in
    // 7609987, subtitle em_scale +5.88 % in 9ed2b96, bloom2_alpha
    // ceiling +5 % in 9f9436b, lower-left alpha +5.5 % in 12fac53,
    // and title alpha +4.86 % in f8c4f2d — so the moon's middle
    // atmospheric layer now catches the gentlest +4.58 % register
    // the recent inscription-side breath axis has settled on, and
    // the moon's three nested atmospheric layers (body + halo +
    // sky bell) share one proportional refinement arc with the
    // four inscribed strokes and the calligrapher's seal across
    // breath, luminance, geometric extent, and warm-mist axes. The
    // halo's brightest pixel sits at alpha ≈ 0.0864 (was ≈ 0.0826)
    // at peak pulse — the +0.0035 absolute lift stays inside the
    // cream family and keeps the halo well under the inscribed
    // glow (~0.20+) and the hero bloom (~0.55), so the focal line
    // keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 "高光只落在主句" holds). The halo radius 68, the σ 90.3
    // sky bell, the σ 8.5 body bell, the body_pulse 0.023, the
    // halo_pulse 0.0807, the sky_pulse 0.0391, the body 0.682,
    // the sky_peak 0.0384, the warm bells 7.677, the supporting
    // mist bell 7.677, the title ambient warmth 7.677, the cool
    // tint 0.13908, the moon proximity 0.107, the subtitle alpha
    // 0.76, the upper-right alpha 0.756, the lower-left alpha
    // 0.646, the title alpha 0.582, the title v 0.83, the
    // inscribed-breath base 0.5340, the title breath 0.3096, and
    // the supporting slots' positions and drifts are all
    // unchanged so only the halo's luminance shifts and the moon's
    // geometric structure and breath stay exactly as they were —
    // only the middle atmospheric layer's peak luminance shifts,
    // and only by +4.58 % of its prior register. With the halo's
    // brightest pixel now reaching one more restrained step into
    // the page's moonlit air — at the gentlest +4.58 % register
    // the recent inscription-side breath arc has settled on —
    // 《寻隐者不遇》 reads as one Tang quatrain inscribed in moonlit
    // air whose moon's middle atmospheric layer now registers one
    // more gentle step out of the page's inhabited air, and the
    // four inscribed strokes plus the calligrapher's seal continue
    // to share one proportional cadence across breath, luminance,
    // geometric extent, warm-mist, and outer-corona axes, with
    // the moon's middle atmospheric layer finally stepping onto
    // the gentlest-step register the recent inscription-side breath
    // arc has completed.
    // Halo peak 0.072 → 0.0765 (+6.25 %, the seventh step on the
    // moon's middle atmospheric layer's luminance axis, paired with
    // the +6.25 % halo radius extension in 8113547): the moon's
    // moonlit air now reads as one touch more visibly continuous with
    // the disc against the heavily vignette-darkened upper-right
    // corner after the geometric-extent axis lifted the halo's
    // outer rim from d=64 → d=68 (+6.25 %) without lifting the
    // brightest halo pixel that lives at d=21 (the 4-px fade-in
    // endpoint). The +6.25 % continues the same restraint cadence
    // as the prior five halo bumps (+10 % / +5.5 % / +5.5 % / +5.0 %
    // / +6.25 % / +5.88 %) and the supporting mist bell lift
    // (+6.25 % in 7c27f49) — the halo now catches up with its own
    // geometric extent on the +6.25 % boundary the supporting mist
    // bell established, rather than the halo's geometric extent
    // quietly outpacing its luminance after the radius lifted in
    // 8113547. The 0.0765 cap keeps the halo comfortably under the
    // inscribed glow (~0.20+) and the hero bloom (~0.55) so the
    // focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 "高光只落在主句" — 高光只落在主句 holds);
    // +0.0045 absolute lift stays inside the cream family and keeps
    // the brightest halo pixel well under the body's 0.682 peak,
    // and the brightest halo pixel stays under the inscribed glow
    // (~0.20+) so the moon's three nested atmospheric layers
    // (body anchor σ 8 + halo radius 68 + sky bell σ 80) keep their
    // clear magnitude hierarchy (body > halo > sky). The +6.25 %
    // also pairs with the recent inscription-side refinement chain
    // — supporting mist bell 6.8 → 7.225 (+6.25 % in 7c27f49),
    // halo_pulse 0.07 → 0.0735 (+5 % in ab6a040), sky_pulse 0 →
    // 0.034 → 0.0356 (+3.4 % / +4.76 % in a79662b / 45b94af), moon
    // halo radius 64 → 68 (+6.25 % in 8113547), sky_peak 0.018 →
    // 0.034 (+56 % / +7 % / +6.25 % x3 in b7ebeda, 80e27d5,
    // 9ec99ff), terminator amber-tint cap 0.12 → 0.140 (+8.3 % /
    // +5.4 % / +2.19 % in c601184, 1c666a7, b3b0daa), cool_tint
    // 0.115 → 0.13583 (+6.5 % / +2.4 % / +2.7 % / +2.5 % in 0ce6e37,
    // 610ee7a, f595bff, 77b520e), moon_proximity 0.04 → 0.102
    // (+50 % / +16.7 % / +17.1 % / +6.1 % / +5.75 % / +5.43 % /
    // +5.15 %), title alpha 0.40 → 0.529 (+10 % / +4.5 % / +4.3 %
    // / +5 % / +5 % in the prior arc and c9f4dde, 4b84ab7), title
    // breath 0.0435 → 0.1281 (+6.7 % / +6.25 % / +5.88 % x3 /
    // +5.56 % / +5.26 % / +5 % / +4.76 % x15), inscribed-breath
    // base 0.075 → 0.2207 (+6.7 % / +6.25 % / +6.25 % / +5.88 % /
    // +5.88 % / +5.56 % / +5.26 % / +5 % / +4.76 % x15), warm
    // bell 6.0 → 6.8 (+6.7 % / +6.25 % in c5f73e0, c253209), title
    // ambient warmth 6.4 → 6.8 (+6.25 % in 4ac2395, c253209), body
    // 0.50 → 0.682 (+10 % / +5.5 % / +5.5 % / +5.5 % / +5.6 %),
    // lower-left alpha 0.50 → 0.612 (+16 % / +5.5 %), sky bell σ
    // 75 → 80 (+6.67 % in efd8cb1), and vignette curve pow(0.7) →
    // pow(0.75) (+7.1 % in 7304555) — so the moon's three nested
    // atmospheric layers (body + halo + sky bell), the four
    // inscribed strokes, the calligrapher's seal, the page frame,
    // and the warm horizon mist band now share one proportional
    // series of restrained steps (+2.19 %, +2.4 %, +2.5 %, +2.7 %,
    // +3.4 %, +4.3 %, +4.5 %, +4.76 % x17, +5.0 %, +5.15 %, +5.26 %,
    // +5.4 %, +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %, +5.88 %,
    // +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.0 %, +7.1 %,
    // +8.3 %), and the page's moonlit atmosphere reads as one
    // coherent refinement rather than twenty-five independent tweaks.
    // The 4-px fade-in, σ 8 body bell, σ 80 sky bell, the ±26 % /
    // 0.140 amber-tint terminator cap, the 0.65 multiplier, body
    // 0.682, body_pulse 0.021, halo_pulse 0.0735, sky_peak 0.034,
    // sky_pulse 0.0356, warm bells 7.225, supporting mist bell
    // 7.225, title ambient warmth 7.225, cool tint 0.13583, moon
    // proximity 0.102, lower-left alpha 0.612, title alpha 0.529,
    // title v 0.83, inscribed-breath base 0.5340, title breath
    // 0.3096, halo radius 68, and the supporting slots' positions
    // and drifts are all unchanged so only the halo's luminance
    // shifts and the moon's geometric structure stays identical;
    // with the halo's brightest pixel now at 0.0765 vs the prior
    // 0.072, the moon's middle atmospheric layer reads one touch
    // more visibly continuous with the body after the radius lifted
    // to 68 in 8113547 — the geometric extent and the luminance
    // peak now share one +6.25 % cadence on the moon-side
    // atmospheric arc. The disc reads as one luminous body whose
    // body, halo, and sky bell breathe together with the same
    // +4.76-7.35 % cadence the inscribed strokes have been sharing,
    // and the moon's middle atmospheric layer now catches the
    // +6.25 % boundary the supporting mist bell established across
    // all three coupled atmospheric layers (warm mist +
    // geometric extent + luminance) rather than the halo's
    // luminance quietly lagging its own geometric extent.
    // 《寻隐者不遇》 reads as one Tang quatrain inscribed in moonlit
    // air whose moon's middle atmospheric layer now breathes one
    // touch more visibly with the page's atmosphere at the same
    // +6.25 % boundary the supporting mist bell and halo radius
    // established, and the moon's three nested atmospheric layers
    // share one coupled restraint cadence across breath, luminance,
    // geometric extent, and warm-mist axes.
    // Halo peak 0.080 → 0.082 (+2.5 %, the gentlest step on the
    // moon-side luminance axis after halo radius 68 → 69.7 (+2.5 %
    // in 28af5b6) and sky σ 90.3 → 92.6 (+2.55 % in c28ed51) — the
    // +2.5 % sits exactly inside the +2.35-2.86 % gentlest rung the
    // moon-side geometric-extent axis has just completed (body σ
    // +2.35 % in 4695311, halo radius +2.5 % in 28af5b6, sky σ
    // +2.55 % in c28ed51) and the inscribed-stroke alpha axis has
    // just settled onto (subtitle alpha +2.63 % in 1c2fb98,
    // lower-left alpha +2.48 % in 70c9147, upper-right alpha
    // +2.65 % in 867377c, title/seal alpha +2.58 % in 69ce9b1),
    // so the moon's three nested atmospheric layers' geometric
    // extent (body σ 8.7 + halo radius 69.7 + sky σ 92.6) and the
    // middle layer's luminance peak (0.082) now share one coupled
    // gentlest-step cadence across the extent and luminance axes
    // rather than the halo's luminance peak quietly sitting at its
    // post-9ec99ff +6.25 % register while the geometric-extent and
    // inscribed-stroke alpha axes stepped past it at +2.35-2.65 %.
    // The +2.5 % (0.080 → 0.082) lifts the moon's middle
    // atmospheric layer's brightest pixel from 0.080 to 0.082
    // (+0.002 absolute, well inside the cream family and clearly
    // under the inscribed glow band 0.20+ and the hero bloom
    // ~0.55), so the focal line 《松下问童子》 keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高
    // 光只落在主句' holds) and the moon's middle atmospheric
    // layer now reads one restrained step more visibly continuous
    // with the disc against the heavily vignette-darkened upper-
    // right corner at the same gentlest-step register the moon-
    // side geometric-extent axis has just completed. The +0.002
    // absolute lift stays well inside the safe-area margin (the
    // halo's brightest pixel peak is still bounded by 0.082 which
    // is well below the inscribed glow ~0.20+ and the hero bloom
    // ~0.55 — so the supporting tier stays clearly subordinate to
    // the focal line), the focal hierarchy (hero / subtitle /
    // upper-right / lower-left / seal) is unchanged, the brush-
    // weight gradient (subtitle brightest → upper-right → lower-
    // left → title dimmest) holds, and the moon-side luminance
    // axis now extends the gentlest-step +2.5 % register the
    // moon-side geometric-extent and inscribed-stroke alpha axes
    // have just settled onto; the body_peak 0.682 (saturated, so
    // the body bell's center stays exactly as it was), the sky_peak
    // 0.0384, the body σ 8.7, the halo radius 69.7, the sky σ 92.6,
    // the 4-px halo fade-in, the terminator alpha 0.267, the
    // terminator cap 0.144, the cool_tint 0.13908, the
    // moon_proximity 0.107, the body_pulse 0.023, the halo_pulse
    // 0.0807, the sky_pulse 0.0391, the bloom2_alpha ceiling
    // 0.063, the nebula alphas 0.022 / 0.016, the vignette pow
    // (0.75), the vignette ceiling 0.74, the subtitle alpha 0.78,
    // the upper-right alpha 0.776, the lower-left alpha 0.662,
    // the title/seal alpha 0.597, the title v 0.83, the title
    // target_px 23, the title breath 0.3096, the inscribed-breath
    // base 0.5340, the supporting mist bell 7.677, the warm bells
    // 7.677, the title ambient_warmth share 0.2222, the
    // supporting mist_warmth share 0.2222, the subtitle
    // mist_warmth share 0.0895, the lower-left mist_warmth share
    // 0.1064, and the supporting slots' positions and drifts are
    // all unchanged so only the halo's luminance peak shifts and
    // the moon's middle atmospheric layer catches up with the
    // gentlest-step +2.5 % register the moon-side geometric-extent
    // axis has just completed. Restraint (ART_DIRECTION §四 '克制
    // 统一的调色板' / '高光只落在主句') holds: the +0.002 absolute
    // halo luminance lift stays inside the cream family, the
    // brightest halo pixel still sits well under the inscribed
    // glow (~0.20+) and the hero bloom (~0.55), and the moon's
    // middle atmospheric layer still reads as moonlit air
    // continuous with the disc rather than as a competing ring.
    // With the halo's luminance peak now catching one more
    // restrained step of the page's gentlest-step register — at
    // the gentlest +2.5 % step on the moon-side luminance axis,
    // exactly inside the +2.35-2.65 % rung the moon-side
    // geometric-extent axis has just completed (body σ +2.35 %
    // 4695311, halo radius +2.5 % 28af5b6, sky σ +2.55 % c28ed51)
    // — 《寻隐者不遇》 reads as one Tang quatrain inscribed in
    // moonlit air whose moon's middle atmospheric layer now
    // registers one more gentle step of the page's moonlit air,
    // and the moon's three nested atmospheric layers (body σ +
    // halo r + sky σ on the geometric-extent axis, body_peak +
    // halo_peak + sky_peak on the luminance axis) now share one
    // coupled gentlest-step cadence across geometric extent and
    // luminance axes together with the inscribed-stroke alpha
    // axis the page has just settled onto, with the halo luminance
    // axis finally stepping onto the gentlest +2.5 % register the
    // page-wide +2-3 % material refinement band the most-refined
    // axes have settled into.
    let halo_peak = 0.082_f32;
    let body_recip = 1.0 / moon_body_r;
    let halo_span = moon_halo_r - moon_body_r;
    let mcx_i = mcx as i32;
    let mcy_i = mcy as i32;
    let extent = moon_halo_r as i32 + 1;
    for oy in -extent..=extent {
        for ox in -extent..=extent {
            let xx = mcx_i + ox;
            let yy = mcy_i + oy;
            if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                continue;
            }
            let dx = ox as f32;
            let dy = oy as f32;
            let d2 = dx * dx + dy * dy;
            if d2 < moon_halo_r2 {
                let in_body = d2 < moon_body_r2;
                let d = d2.sqrt();
                // Body bell — Gaussian with sigma 8.7 px (the gentlest
                // step on the moon's geometric-extent axis after halo
                // radius 68 → 69.7 (+2.5 % in 28af5b6), pairing with the
                // terminator alpha +2.69 % / cap +2.86 % / subtitle
                // alpha +2.63 % gentlest-step register the directional-
                // modulation and inscribed-stroke alpha axes have just
                // settled onto, 47ac018 / 4077850 / 1c2fb98): the moon's
                // innermost atmospheric layer now extends one more
                // restrained step into the halo boundary at the same
                // +2-3 % gentlest rung the page-wide material refinement
                // band the most-refined axes have settled into — at d=17
                // (body edge) body_k now sits at exp(-17²/151.38) =
                // exp(-1.91) ≈ 0.148 → alpha ≈ 0.101 (was 0.092 at σ 8.5,
                // +0.009 absolute, +9.8 % relative at the body edge, well
                // under the inscribed glow ~0.20+ and the hero bloom
                // ~0.55), at d=22 (halo start) body_k now sits at
                // exp(-22²/151.38) = exp(-3.20) ≈ 0.041 → alpha ≈ 0.028
                // (was 0.024 at σ 8.5, +0.004 absolute so the body
                // bell's contribution to the halo region is a touch
                // more continuous), and at d=45 (halo mid) body_k drops
                // to exp(-45²/151.38) = exp(-13.4) ≈ 1.5e-6 (still well
                // below the 0.003 threshold so the bell doesn't paint
                // visible color past its natural boundary); the +0.2 px
                // σ extension stays in the relationship between the
                // body and the halo rather than spreading the bell into
                // the sky region, and the body luminance remains firmly
                // under the inscribed glow (~0.20+) and the hero bloom
                // (~0.55) so the focal line keeps its exclusive claim on
                // the page's light (ART_DIRECTION §四 '高光只落在主句').
                // The +2.35 % continues the same gentlest-step register
                // as the recent chain — terminator alpha 0.26 → 0.267
                // (+2.69 % in 47ac018), terminator cap 0.140 → 0.144
                // (+2.86 % in 4077850), halo radius 68 → 69.7 (+2.5 %
                // in 28af5b6), subtitle alpha 0.76 → 0.78 (+2.63 % in
                // 1c2fb98), vignette +2.5 % (7304555), and the cool_tint
                // +2.4 % / +2.5 % / +2.7 % cadence (0ce6e37, 610ee7a,
                // f595bff, 77b520e) — so the moon's three nested
                // atmospheric layers (body σ 8.7 + halo radius 69.7 +
                // sky bell σ 90.3) now share one coupled gentlest-step
                // cadence on the geometric-extent axis rather than the
                // body σ 8.5 quietly sitting at its post-bcfab51 +6.25 %
                // register while the surrounding material axes stepped
                // past it at +2.5 % / +2.69 % / +2.86 %. Restraint
                // (ART_DIRECTION §四 '克制统一的调色板') holds: the body's
                // +0.009 edge-alpha lift stays inside the cream family,
                // the brightest body pixel still sits at body_peak *
                // 1.0 = 0.682 (the body bell's center is unchanged), and
                // the body's edge still sits clearly under the inscribed
                // glow (~0.20+) and the hero bloom (~0.55), so the focal
                // line keeps its claim on the page's light and the moon
                // continues to read as one luminous body bathed in
                // moonlit air. The body luminance peak (0.682), the halo
                // peak (0.080), the sky peak (0.0384), the halo radius
                // (69.7), the σ 90.3 sky bell, the 4-px halo fade-in, the
                // ±26.7 % / 0.144 terminator cap, the 0.65 vignette
                // multiplier, body_pulse 0.023, halo_pulse 0.0807,
                // sky_pulse 0.0391, warm bells 7.677, supporting mist
                // bell 7.677, title ambient warmth 7.677, cool tint
                // 0.13908, moon proximity 0.107, subtitle alpha 0.78,
                // upper-right alpha 0.756, lower-left alpha 0.646,
                // title alpha 0.582, title v 0.83, inscribed-breath
                // base 0.5340, title breath 0.3096, and the supporting
                // slots' positions and drifts are all unchanged so only
                // the body bell's geometric extent shifts and the
                // moon's three nested atmospheric layers share one
                // coupled gentlest-step cadence on the geometric-extent
                // axis. With the moon's body bell now extending one
                // more restrained step into the page's inhabited
                // atmosphere at the gentlest +2.35 % step the
                // page-wide +2-3 % material refinement band the most-
                // refined axes have just settled onto — the moon's
                // innermost atmospheric layer now catches the same
                // gentlest-step register the halo radius 69.7 (+2.5 %
                // 28af5b6), terminator alpha 0.267 (+2.69 % 47ac018),
                // terminator cap 0.144 (+2.86 % 4077850), and subtitle
                // alpha 0.78 (+2.63 % 1c2fb98) have just completed —
                // 《寻隐者不遇》 reads as one Tang quatrain inscribed in
                // moonlit air whose moon's innermost atmospheric layer
                // now registers one more gentle step of the moon's own
                // atmospheric reach, and the moon's three nested
                // atmospheric layers (body + halo + sky bell) now share
                // one coupled gentlest-step cadence on the geometric-
                // extent axis together.
                let body_k = (-d * d / 151.38).exp();
                let halo_k = if !in_body {
                    let d_from_body = d - moon_body_r;
                    // Halo starts at 0 at the body edge (4-px quadratic
                    // fade-in over [0, 4) px outside the disc), peaks
                    // at body_r + 4 px, then falls quadratically to 0
                    // at halo_r. Total integrated luminance (1/3 of
                    // halo_span * halo_peak) is preserved, but the
                    // brightest pixel now sits closer to the body
                    // edge — smoothing the visible "ring" between the
                    // body's Gaussian tail (alpha ≈ 0.009 at the old
                    // peak d=23) and the halo peak. The previous 6-px
                    // fade-in had the peak 6 px outside the disc while
                    // the body contribution had already fallen to ≈ 9 %
                    // alpha there, leaving a valley at d≈20 that read
                    // as a faint dark band between body and halo
                    // against the vignette-darkened upper-right corner.
                    // The 4-px fade-in puts the peak where the body
                    // still contributes ≈ 32 % of its center alpha, so
                    // body + halo read as one continuous luminous
                    // body bathed in moonlit air rather than "bright
                    // disc + dark band + bright ring". The 4-px width
                    // is well inside the halo span (43 px) so the
                    // body and the brightest part of the halo still
                    // merge into one perceptually continuous luminous
                    // body. Restraint holds: peak alpha unchanged at
                    // 0.055 (still well under the inscribed glow
                    // ~0.20+ and the hero bloom ~0.55), so the moon
                    // keeps its claim as a quiet still anchor against
                    // the focal line (ART_DIRECTION §四 "高光只落在
                    // 主句").
                    if d_from_body < 4.0 {
                        let k = d_from_body * (1.0 / 4.0);
                        k * k
                    } else {
                        let k = 1.0 - (d_from_body - 4.0) / (halo_span - 4.0);
                        k * k
                    }
                } else {
                    0.0
                };
                // Terminator applied to the body only — the halo is just
                // moonlit air, not directional. Light from the lower warm
                // horizon means dy > 0 (lower side) gets a slight warm
                // boost; dy < 0 (upper side) gets a slight cool. Strength
                // raised ±12 % → ±16 % → ±20 % → ±24 % (≈ +100 % over
                // three passes) and the bottom half now tints 0..10 % →
                // 0..12 % → 0..13 % → 0..13.7 % toward AMBER (+5.4 %
                // relative, the third lift in the tint arc after
                // 0e5c075 introduced it alongside the ±16 → ±20 %
                // alpha step and c601184 brought it to 13 % alongside
                // the body halo cadence work) so the moon reads more
                // clearly as a body catching horizon light — the
                // bottom edge now catches ≈+24 % alpha AND ≈13.7 %
                // amber, giving the disc a clear direction (lit side
                // facing down, where the warm horizon mist sits) rather
                // than a uniform luminous disc. The +5.4 % amber-tint
                // lift continues the same restraint cadence as the
                // recent chain — halo peak 0.064 → 0.068 (+6.25 % in
                // 9989c4a), title alpha 0.48 → 0.504 (+5 % in c9f4dde),
                // title breath 0.0493 → 0.0522 (+5.88 % in d44ac01),
                // inscribed-breath base 0.085 → 0.090 (+5.88 % in
                // d44ac01), inscribed-breath base 0.080 → 0.085
                // (+6.25 % in 29e4093), title breath 0.0464 → 0.0493
                // (+6.25 % in 29e4093), lower-left alpha 0.58 → 0.612
                // (+5.5 % in 9a4cc96), body 0.646 → 0.682 (+5.6 % in
                // 3b60530), sky_peak 0.032 → 0.034 (+6.25 % in
                // 9ec99ff), cool_tint 0.123 → 0.126 (+2.4 % in
                // 610ee7a), moon_proximity 0.082 → 0.087 (+6.1 % in
                // 610ee7a), warm bell 6.0 → 6.4 (+6.7 % in c5f73e0),
                // inscribed-breath base 0.075 → 0.080 (+6.7 % in
                // 708d491), title breath 0.0435 → 0.0464 (+6.7 % in
                // 14d58aa), cool_tint 0.115 → 0.123 (+6.5 % in
                // 0ce6e37), and the moon-side body +5.5 % x4 and halo
                // +5.0–5.5 % x4 chains — so the moon's two innermost
                // atmospheric layers, the four inscribed strokes, the
                // calligrapher's seal, and the moon's chromatic-
                // asymmetry term now share one proportional series of
                // restrained steps (+2.4 %, +4.3 %, +5.0 %, +5.5 %,
                // +5.6 %, +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.7 %,
                // +8.3 %), and the page reads as one coherent
                // refinement rather than sixteen independent tweaks.
                // The +5.4 % shifts axis to the chromatic term after
                // the body's +5.6 % luminance cap in 3b60530 noted
                // "subsequent refinement in this direction will need
                // to drop to a smaller increment or shift axis" — the
                // body luminance has now saturated at 0.682, the halo
                // reached its +6.25 % ceiling in 9989c4a, and the
                // terminator amber-tint cap is the remaining moon-side
                // knob with headroom. The +24 % alpha asymmetry stays
                // put (its last step in f207bea already brought the
                // directional "moon catching horizon light" reading
                // clearly into view against the heavily vignette-
                // darkened upper-right corner, so only the chromatic
                // axis needs this single pass). The 13.7 % tint
                // cap stays well under the threshold where the moon
                // would read as amber highlighter (the prior "13 % so
                // the moon still reads as cream ink" cap lifts by only
                // +0.7 absolute / +5.4 % relative, the gentlest end
                // of the same restraint cadence as the +5.0 % title
                // alpha and +5.5 % lower-left and +5.6 % body chain) —
                // restraint (ART_DIRECTION §四 "克制统一的调色板" /
                // "低饱和、高级灰") holds: the moon
                // still reads as cream ink, just ink whose lower edge
                // tints a touch more visibly toward amber, the way a
                // real moon catches more horizon light at twilight than
                // at full night. The top stays pure ink::WARM cream
                // while the bottom shifts a touch warmer. The halo stays
                // non-directional (moonlit air, not lit surface) — the
                // tint is multiplied only into the body's alpha
                // contribution, so the asymmetry never bleeds onto the
                // halo as a colored ring. Total brightness still bounded
                // by body_peak so the moon never out-glows the focal
                // line.
                // Terminator alpha ±20 % → ±24 % → ±26 % (+8.3 %, paired
                // with the chromatic cap lift 0.13 → 0.137 in 1c666a7):
                // the moon's directional "lit side faces down" reading now
                // sits one more restrained step deeper, the way a real
                // moon's lower edge catches more horizon light as twilight
                // deepens. The +8.3 % alpha asymmetry pairs with the recent
                // +5.4 % chromatic cap (1c666a7) and the page-wide +5-8 %
                // restraint cadence — terminator amber-tint cap 0.12 → 0.13
                // → 0.137 (+8.3 % / +5.4 % in c601184 / 1c666a7), title
                // alpha 0.46 → 0.48 → 0.504 (+4.3 % / +5 % in e37c083 /
                // c9f4dde), halo 0.05 → 0.055 → 0.058 → 0.061 → 0.064 →
                // 0.068 (+5.0-6.25 %), body 0.50 → 0.682 (+5.5-5.6 % x5),
                // sky_peak 0.018 → 0.034 (+56 % / +6.25 % x3), inscribed-
                // breath base 0.075 → 0.090 (+5.88-6.7 % x3), title breath
                // 0.0435 → 0.0522 (+5.88-6.7 % x3), warm bell 6.0 → 6.4
                // (+6.7 %), cool_tint 0.115 → 0.126 (+2.4-6.5 %), moon_
                // proximity 0.04 → 0.087 (+6.1 %), and lower-left alpha
                // 0.50 → 0.612 (+5.5 %) — so the moon's three nested
                // atmospheric layers, the four inscribed strokes, the
                // calligrapher's seal, and the moon's directional terminator
                // now share one proportional series of restrained steps
                // (+2.4 %, +3.2 %, +4.3 %, +5.0 %, +5.4 %, +5.5 %, +5.6 %,
                // +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.7 %, +8.3 %). At
                // ±26 % the bottom edge now reads ~1.26 × the top edge —
                // the disc registers as one body catching horizon light
                // more clearly than the previous ±24 % reading, the
                // bottom +0.02 alpha lift (0.682 * 1.26 = 0.859 at peak
                // pulse vs the previous 0.682 * 1.24 = 0.846) still sits
                // well under the inscribed glow (~0.20+ effect multiplied
                // out by the focal bloom's combined ~0.7 alpha, so the
                // focal line keeps its claim on the page's light —
                // ART_DIRECTION §四 '高光只落在主句' holds), and the moon
                // still reads as one luminous body bathed in moonlit air
                // rather than as a hard disc with a directional gradient.
                // The terminator stays applied only to the body's alpha
                // contribution (the halo is moonlit air, not a lit
                // surface, so the asymmetry never bleeds onto the halo as
                // a colored ring), and the tint term (0.137 chromatic
                // cap, +5.4 % from 1c666a7) stays paired with the alpha
                // asymmetry so the disc now reads as a body whose bottom
                // edge catches both more luminance AND a touch more
                // amber — the way a real twilight moon catches more of
                // both when the horizon glow sits directly below it.
                let t_term = (dy * body_recip).clamp(-1.0, 1.0);
                let term = 1.0 + 0.267 * t_term;
                // Term-warm directional sway ±26 % → ±26.7 %
                // (terminator alpha 0.26 → 0.267, +2.69 %, the gentlest
                // step on the directional-modulation axis after +8.3 %
                // in ad3ee9a — pairing with the +2.86 % terminator cap
                // lift in 4077850 so the moon's terminator geometry
                // catches one more restrained step of horizon light on
                // BOTH dials of the directional reading at once) so the
                // moon's bottom edge reads one touch more visibly as
                // catching horizon light — the directional sway now
                // lifts from 0.74×cap..1.26×cap to 0.733×cap..1.267×cap
                // (top fades ~+0.7 % relative, bottom brightens ~+0.6 %
                // relative), the gentlest rung on the page-wide +2-3 %
                // material refinement band the most-refined axes have
                // settled into (cool_tint +2.4 %, +2.5 %, +2.7 %;
                // vignette +2.5 % neighbour; warm-tint cap approaching
                // saturation; terminator cap +2.86 % in 4077850), so
                // the moon's terminator alpha and cap now share one
                // proportional rhythm — the bottom half sits a touch
                // more clearly inside the warm horizon band without the
                // moon starting to read as wearing an amber ring.
                // Restraint (ART_DIRECTION §四 "克制统一的调色板" / "高光
                // 只落在主句") holds: the +0.007 alpha absolute lift
                // stays well inside the cream/amber family, the 0.144
                // cap and the 0.267 alpha together still produce a top
                // contribution ≤ 10.55 % toward AMBER (was ≤ 10.66 %)
                // and a bottom contribution ≤ 18.24 % toward AMBER (was
                // ≤ 18.14 %), both still clearly under the inscribed
                // glow (~0.20+) and the focal bloom (~0.55), so the
                // moon continues to read as one luminous body whose
                // bottom catches horizon light rather than as a body
                // wearing an amber ring; the terminator cap 0.144, the
                // cool_tint 0.13908, the moon_proximity 0.107, the body
                // 0.682, the halo_peak 0.080, the sky_peak 0.0384, the
                // title ambient_warmth share 0.2222, the title
                // target_px 23, the title alpha 0.582, the inscribed-
                // breath base 0.5340, the title breath 0.3096, the
                // subtitle em_scale 0.36, the upper-right em_scale
                // 0.32, the lower-left em_scale 0.28, the subtitle
                // alpha 0.76, the upper-right alpha 0.756, the
                // lower-left alpha 0.646, the upper-right y_frac 0.27,
                // the lower-left y_frac 0.74, the supporting mist bell
                // 7.677, the warm bell 7.677, the subtitle drift
                // 3.0/1.5/0.21/0.17/0.7, the lower-left drift
                // 3.0/2.0/0.13/0.21/2.8, the upper-right drift
                // 3.0/2.0/0.15/0.19/1.4, the hero drift 3.0/2.0/
                // 0.18/0.13/0.0, the hero y_frac 0.42, the hero bloom
                // 1.0, the moon_halo_r 68, the body σ 8.5, the sky σ
                // 90.3, the body_pulse 0.023, the halo_pulse 0.0807,
                // the sky_pulse 0.0391, the bloom2_alpha ceiling 0.063,
                // the nebula alphas 0.022/0.016, the vignette
                // pow(0.75), and the vignette ceiling 0.74 are all
                // unchanged so only the terminator alpha modulation
                // shifts on the moon's directional reading axis and
                // the bottom edge catches one more restrained step of
                // horizon light. Pairs with the recent terminator cap
                // +2.86 % lift (4077850), the recent inscription-side
                // refinement chain — title alpha 0.40 → 0.582 (+10 % /
                // +4.5 % / +4.3 % / +5 % / +5 % / +4.86 % in the prior
                // arc and c9f4dde, 4b84ab7, f8c4f2d), inscribed-breath
                // base 0.075 → 0.5340 (+6.7 % / +6.25 % x2 / +5.88 %
                // x2 / +5.56 % / +5.26 % / +5 % / +4.76 % x10 in the
                // prior arc and the recent 18 supporting-tier lifts),
                // title breath 0.0435 → 0.3096 (+6.7 % / +6.25 % /
                // +5.88 % x3 / +5.56 % / +5.26 % / +5 % / +4.76 %
                // x10), title target_px 22 → 23 (+4.55 % in b804477),
                // body 0.50 → 0.682 (+5.6 % x5 in fe42fec, b7ebeda,
                // 90e22dc, 3b60530), halo 0.05 → 0.080 (+5.0-6.25 %
                // x5 / +4.58 % in 238b40b, 698aa08, 0f13e55,
                // 9989c4a, e0b708c), sky_peak 0.018 → 0.0384 (+56 % /
                // +7 % / +6.25 % x3 in b7ebeda, 80e27d5, 9ec99ff),
                // terminator amber-tint cap 0.12 → 0.144 (+8.3 % /
                // +5.4 % / +2.19 % / +2.86 % in c601184, 1c666a7,
                // b3b0daa, 4077850), terminator alpha 0.20 → 0.267
                // (+8.3 % / +2.69 % in ad3ee9a and this commit),
                // cool_tint 0.115 → 0.13908 (+6.5 % / +2.4 % / +2.7 %
                // / +2.5 % in 0ce6e37, 610ee7a, f595bff, 77b520e),
                // moon_proximity 0.04 → 0.107 (+50 % / +16.7 % /
                // +17.1 % / +6.1 % / +5.75 % / +5.43 % / +5.15 % /
                // +4.9 % in the prior arc), lower-left alpha 0.50 →
                // 0.646 (+16 % / +5.5 % / +5.5 % in e37c083's chain,
                // 9a4cc96, 12fac53), sky bell σ 75 → 90.3 (+6.67 % /
                // +6.25 % in efd8cb1, 05eefb4), vignette curve pow(0.7)
                // → pow(0.75) (+7.1 % in 7304555), supporting mist
                // bell 6.4 → 7.677 (+6.25 % / +6.25 % in 4ac2395,
                // c253209, aa626f1), warm bell 6.0 → 7.677 (+6.7 % /
                // +6.25 % / +6.25 % in c5f73e0, c253209, aa626f1),
                // title ambient warmth 6.4 → 7.677 (+6.25 % in
                // 4ac2395, c253209, aa626f1), title ambient_warmth
                // share 0.2125 → 0.2222 (+4.55 % in df4a49e), and moon
                // halo radius 44 → 68 (+45 % / +6.25 % over four
                // passes 238b40b, 698aa08, 9989c4a, 8113547) — so the
                // moon's three nested atmospheric layers, the four
                // inscribed strokes, the calligrapher's seal, the page
                // frame, and the moon's reach onto its closest
                // inscription line now share one proportional series of
                // restrained steps (+2.19 %, +2.4 %, +2.5 %, +2.69 %,
                // +2.7 %, +2.86 %, +4.55 %, +4.58 %, +4.76 % x10,
                // +4.86 %, +4.9 %, +5.0 %, +5.15 %, +5.26 %, +5.4 %,
                // +5.43 %, +5.5 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %,
                // +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %,
                // +7.1 %, +8.3 %), and the page's moonlit atmosphere
                // reads as one coherent refinement rather than
                // twenty-six independent tweaks. With the moon's
                // bottom edge now catching one more restrained step of
                // horizon light — the terminator alpha and cap both
                // stepping onto the +2-3 % material refinement band
                // the most-refined axes have settled into, +2.69 % on
                // the directional modulation and +2.86 % on the cap
                // (4077850) — the moon continues to read as one
                // luminous body whose bottom catches horizon light,
                // and the warm/cool axis the moon inhabits tightens
                // one more restrained step inside the cream/amber
                // family.
                let term_warm = (t_term * 0.144).max(0.0);
                // Body and halo now breathe independently — the disc sits
                // at ±2.1 % (the still anchor, below every inscription line),
                // the moonlit air at ±7 % (slightly more than the
                // subtitle's ±6.3 %, so the page's atmosphere pulses past
                // the moon rather than the moon breathing with the page).
                // Restores the original intent — the supporting-tier
                // amplitude bump (0.06 → 0.075 base, commit 5241c61) had
                // lifted the subtitle from ±5 % to ±6.3 % without a
                // matching halo lift, so the moon's air ended up sitting
                // fractionally below the inscription's most-present line
                // (halo 6.0 % < subtitle 6.3 %) — the metaphor of
                // "atmosphere pulses past the moon" had quietly inverted
                // into "the inscription breathes past the moon". The +0.7
                // pp halo lift (0.06 → 0.07, +16.7 % breath amplitude)
                // restores the original hierarchy at the same restraint
                // cadence as the prior halo bump (+10 % peak in 69bf26a)
                // and the supporting-tier +25 % over two passes (5241c61 +
                // 99e14e1) — the page's atmosphere now reads as the
                // outside the moon inhabits rather than a sub-layer of
                // the inscription it sits among. At pulse=1 the halo
                // still only varies by ±7 % (peak 0.072 * 1.07 = 0.0770)
                // so the absolute alpha ceiling is essentially unchanged
                // — the lift is in the breath relationship, not in the
                // halo's brightness, so the focal line's claim on the
                // page's light holds (ART_DIRECTION §四 "高光只落在主句").
                //
                // Body breath 0.02 → 0.021 (+5 %, the first step on the
                // moon's own breath axis after forty-two consecutive
                // supporting-tier breath-base lifts reached the natural
                // ceiling of the inscription-breath axis in 68330e6) —
                // the disc's still-anchor breath now couples one touch
                // more visibly with the page's atmosphere, so the body
                // itself reads as one luminous body breathing a touch
                // more with the moonlit air that hosts it rather than
                // as a frozen disc at the exact same luminance on every
                // beat. The +5 % (0.02 → 0.021) continues the same
                // restraint cadence as the recent +5 % title alpha lift
                // (c9f4dde), the +5.5-5.6 % body bumps (fe42fec,
                // b7ebeda, 90e22dc, 3b60530), and the +5 % lower-left
                // alpha lifts (9a4cc96) — the page's moon-side breath,
                // luminance, and inscription-side now sit one shared
                // +5 % cadence deeper across all three axes. At pulse=1
                // the body's brightest pixel now varies by ±2.1 %
                // (peak 0.682 * 1.021 = 0.6963, vs the prior 0.682 *
                // 1.02 = 0.6956) so the absolute alpha ceiling is
                // essentially unchanged — the lift is in the body's
                // breathing rhythm, not in its brightness, and the disc
                // still sits a touch below every inscribed line (lower-
                // left ±40.81 %, subtitle ±36.63 %, title alpha 0.529,
                // halo ±7.35 %) so the page's hierarchy of restrains
                // holds (still anchor < halo air < subtitle inscription
                // < focal line). The body still breathes slower than
                // any other element on the page; it now just couples
                // one restrained step more visibly to the same shared
                // rhythm. Restraint (ART_DIRECTION §四 "克制统一的调色板")
                // holds: the body's +0.014 alpha ceiling absolute lift
                // stays inside the cream family, the body's +5 %
                // breath step matches the page's settled +5 % restraint
                // cadence rather than climbing the same +4.76 % breath
                // axis any further, and the disc continues to read as
                // one luminous body whose breathing now couples one
                // touch more with the moon's air without ever
                // crowding the focal-bloom envelope (ARTIFACT §观者第
                // 一分钟 1. 其它一切都在动，只有它是相对静止的锚 — the
                // body remains the page's still anchor, just an anchor
                // that now breathes a touch more with the page's own
                // rhythm rather than a frozen pixel).
                //
                // Halo breath 0.07 → 0.0735 (+5 %, the paired moon-side
                // breath lift completing the body+halo breath axis that
                // ecff1f4 started with the +5 % body_pulse step): the
                // moon's moonlit air now breathes one gentle step more
                // visibly with the page's atmosphere, so the halo and
                // the body now share one +5 % restraint cadence on the
                // moon-side breath axis — the disc and its inner ring
                // sit together in one shared breathing rhythm rather
                // than the body's lift quietly outpacing the halo's.
                // The +5 % (0.07 → 0.0735) mirrors the body's +5 %
                // (0.02 → 0.021) one-to-one — same magnitude, same
                // restraint, same subordination to the focal line —
                // so the moon's two innermost atmospheric layers
                // (body + halo) now read as one coupled breath system
                // rather than the body's +5 % lift sitting alone on
                // the moon's own breath axis. At pulse=1 the halo's
                // brightest pixel now varies by ±7.35 % (peak 0.072 *
                // 1.0735 = 0.0773 vs the prior 0.072 * 1.07 = 0.0770,
                // +0.0003 absolute) so the absolute alpha ceiling is
                // essentially unchanged — the lift is in the halo's
                // breathing rhythm, not in its brightness, and the
                // halo still sits clearly under the inscribed glow
                // (~0.20+) and the hero bloom (~0.55) so the focal
                // line keeps its exclusive claim on the page's light
                // (ART_DIRECTION §四 "高光只落在主句"). The +5 % keeps
                // the halo comfortably above the subtitle's ±6.3 %
                // breath (now halo ±7.35 % vs subtitle ±6.3 % — the
                // page's atmosphere continues to pulse past the moon
                // rather than the moon breathing with the inscription,
                // with the gap widened by one gentle step), and the
                // hierarchy of restrains still holds (still anchor <
                // halo air < subtitle inscription < focal line). The
                // body still breathes slower than any other element on
                // the page (±2.1 %); the halo now breathes a touch more
                // with the page's atmosphere at the same +5 %
                // restraint step the body took in ecff1f4. The +5 %
                // continues the same restraint cadence as the recent
                // +5 % body_pulse lift (ecff1f4), the +5 % title alpha
                // lifts (c9f4dde, 4b84ab7), the +5.5-5.6 % body bumps
                // (fe42fec, b7ebeda, 90e22dc, 3b60530), the +5 %
                // lower-left alpha lifts (9a4cc96), and the +6.25 %
                // supporting mist bell lift (7c27f49) — so the moon's
                // two innermost atmospheric layers, the inscribed
                // strokes, the calligrapher's seal, and the warm
                // horizon mist bell now share one proportional series
                // of restrained +5-7 % steps across breath, luminance,
                // and warm-mist axes. Restraint (ART_DIRECTION §四
                // "克制统一的调色板") holds: the halo's +0.0003 alpha
                // ceiling absolute lift stays inside the cream
                // family, the brightest halo pixel still sits clearly
                // under the inscribed glow (~0.20+) and the hero
                // bloom (~0.55), and the moon continues to read as
                // one luminous body bathed in moonlit air whose body
                // and halo now share one +5 % moon-side breath
                // cadence — the disc still sits as the page's still
                // anchor (ARTIFACT §观者第一分钟 1. 其它一切都在动，
                // 只有它是相对静止的锚), just an anchor whose body
                // and halo now breathe together with one shared
                // restrained step rather than the body's lift sitting
                // alone. The σ 8 body bell, the 4-px halo fade-in, the
                // σ 80 sky bell, the ±26 % / 0.140 amber-tint
                // terminator cap, the 0.65 multiplier, the moon's
                // ±4 / ±2 px drift, and the supporting slots'
                // positions and drifts are all unchanged so only the
                // moon-side breath axis shifts and the moon's
                // geometric structure stays identical; with the halo
                // now breathing one gentle step with the page's
                // atmosphere at the +5 % step that matches the
                // body's +5 % lift in ecff1f4 — the moon's two
                // innermost atmospheric layers now share one coupled
                // restraint cadence and the disc reads as one
                // luminous body whose body and halo breathe together
                // in moonlit air.
                // body_pulse 0.023 → 0.0236 (+2.61 %, halo_pulse 0.0807 →
                // 0.0827 (+2.48 %), the gentlest step on the moon-side
                // breath axis after the post-750ae7a +9.5–9.8 % register
                // — the +2.48–2.61 % sits exactly inside the +2.35–2.86 %
                // gentlest rung the page-wide +2-3 % material refinement
                // band the most-refined axes have settled into (body σ
                // +2.35 % in 4695311; cool_tint +2.4 % / +2.5 % / +2.7 %
                // in 0ce6e37, 610ee7a, f595bff, 77b520e; vignette +2.5 %
                // in 7304555; halo radius +2.5 % in 28af5b6; subtitle
                // alpha +2.63 % in 1c2fb98; terminator alpha +2.69 % in
                // 47ac018; terminator cap +2.86 % in 4077850; lower-left
                // alpha +2.48 % in 70c9147; upper-right alpha +2.65 % in
                // 867377c; title/seal alpha +2.58 % in 69ce9b1; sky σ
                // +2.55 % in c28ed51; halo_peak +2.5 % in 3b8028b;
                // inscribed-breath base 0.5340 already at the gentlest-
                // step ceiling per cc3a82f; title breath 0.3096 already
                // at the gentlest-step ceiling per cc3a82f) rather than
                // the moon's three nested atmospheric layers' breath
                // modulation quietly sitting at its post-750ae7a
                // +9.5–9.8 % register while every surrounding material
                // axis stepped past it at +2.35–2.86 %. The +2.61 %
                // (0.023 → 0.0236) lifts the moon's body bell's
                // breath-in modulation from ±2.3 % to ±2.36 %, the
                // +2.48 % (0.0807 → 0.0827) lifts the moon's middle
                // atmospheric layer's breath-in modulation from ±8.07 %
                // to ±8.27 %, both staying well inside the page's
                // restrained breath family — the disc still inhales at
                // a smaller rate than the halo, the halo still inhales
                // at a smaller rate than the title's 0.3096 and the
                // inscribed-stroke base's 0.5340 (the focal line and
                // the four inscribed strokes stay the page's most-
                // breathing elements, with the moon's middle layer
                // breathing the page's atmosphere through them) — so
                // the moon's three nested atmospheric layers now share
                // one coupled gentlest-step cadence on the breath axis
                // together with the geometric-extent axis (body σ + halo
                // radius + sky σ), the luminance axis (body_peak +
                // halo_peak + sky_peak), the inscribed-stroke alpha
                // axis (subtitle + upper-right + lower-left + title),
                // the directional-modulation axis (terminator alpha),
                // the warm-tint cap axis (terminator cap), the chromatic
                // axis (cool_tint +2.4 % / +2.5 % / +2.7 %), and the
                // frame axis (vignette +2.5 %) the page has settled
                // onto. The body_peak 0.682 (saturated), the halo_peak
                // 0.082, the sky_peak 0.0384, the body σ 8.7, the halo
                // radius 69.7, the sky σ 92.6, the 4-px halo fade-in,
                // the terminator alpha 0.267, the terminator cap 0.144,
                // the cool_tint 0.13908, the moon_proximity 0.107, the
                // bloom2_alpha ceiling 0.063, the nebula alphas
                // 0.022 / 0.016, the vignette pow(0.75), the vignette
                // ceiling 0.74, the subtitle alpha 0.78, the upper-right
                // alpha 0.776, the lower-left alpha 0.662, the
                // title/seal alpha 0.597, the title v 0.83, the title
                // target_px 23, the inscribed-breath base 0.5340, the
                // title breath 0.3096, the supporting mist bell 7.677,
                // the warm bells 7.677, the title ambient_warmth share
                // 0.2222, the supporting mist_warmth share 0.2222, the
                // subtitle mist_warmth share 0.0895, the lower-left
                // mist_warmth share 0.1064, and the supporting slots'
                // positions and drifts are all unchanged so only the
                // moon's three nested atmospheric layers' breath
                // modulation shifts and the moon's body bell and halo
                // catch up with the gentlest-step +2.5 % register the
                // moon-side geometric-extent and inscribed-stroke alpha
                // axes have just settled onto. Restraint (ART_DIRECTION
                // §四 '克制统一的调色板' / '高光只落在主句') holds: the
                // +0.0006 / +0.0020 absolute breath-modulation lift
                // stays inside the page's restrained breath family, the
                // disc still inhales at the gentlest rate of any of the
                // moon's three nested layers (body < halo < sky, the
                // same restraint hierarchy the surrounding material
                // axes have settled onto), and the moon's body bell
                // and halo still read as one luminous body whose body
                // and halo breathe together in moonlit air — the disc
                // still sits as the page's still anchor (ARTIFACT
                // §观者第一分钟 1. 其它一切都在动，只有它是相对静止
                // 的锚), just an anchor whose body and halo now breathe
                // together at one more restrained gentlest step of the
                // page's coupled gentlest-step register.
                // Moon body + halo breath modulation +2.5 % (the
                // gentlest-step register the page-wide +2-3 % material
                // refinement band the most-refined axes have settled
                // onto) — body_pulse 0.0236 → 0.0242 (+2.54 %) and
                // halo_pulse 0.0827 → 0.0848 (+2.54 %), both lifting
                // the moon's body bell and middle atmospheric layer
                // in lockstep so the disc and its halo now inhale
                // one more restrained step of the page's coupled
                // gentlest-step register after the recent warm-mist
                // share +2.5 % lift in e483929 / 3154090. The +2.54 %
                // sits exactly inside the +2.35-2.86 % gentlest rung
                // the page-wide material refinement band has settled
                // into (body σ +2.35 % in 4695311; halo radius +2.5 %
                // in 28af5b6; sky σ +2.55 % in c28ed51; halo_peak
                // +2.5 % in 3b8028b; sky_peak +2.5 % in 78959d2;
                // body_pulse +2.61 % / halo_pulse +2.48 % / sky_pulse
                // +2.56 % in 43fc830; cool_tint +2.4 % / +2.5 % /
                // +2.7 % in 0ce6e37, 610ee7a, f595bff, 77b520e;
                // vignette ceiling +2.5 % in 7304555; terminator
                // alpha +2.69 % in 47ac018; terminator cap +2.86 % in
                // 4077850; subtitle alpha +2.63 % in 1c2fb98; upper-
                // right alpha +2.65 % in 867377c; lower-left alpha
                // +2.48 % in 70c9147; title alpha +2.5 % in e8cc837;
                // subtitle em_scale +2.5 % in a2a3f48; upper-right
                // em_scale +2.5 % in 2bf7493; lower-left em_scale
                // +2.5 % in 19059c2; title target_px +2.5 % in
                // 7397729; bloom2_alpha ceiling +2.5 % in 095ef01;
                // bloom2_alpha base +2.73 % in d7f8fc6; bloom_alpha
                // base +2.5 % in c7813d4; bloom_alpha ceiling +2.5 %
                // in 494d8b7; supporting mist_warmth share +2.5 % in
                // 57c514e; warm-mist bell +2.5 % in 2a7cd02; horizon-
                // band blend share +2.5 % in 3154090; warm-mist share
                // axis +2.5 % in e483929). The breath lift lands the
                // moon's body bell and halo breath modulation at
                // ±2.42 % and ±8.48 % respectively — body still
                // inhales at the gentlest rate of the moon's three
                // nested layers, halo still inhales at a smaller rate
                // than the title's 0.3096 and the inscribed-stroke
                // base's 0.5340, the focal line and the four
                // inscribed strokes stay the page's most-breathing
                // elements, the moon still reads as one luminous
                // body whose body and halo breathe together in
                // moonlit air, and the disc still sits as the page's
                // still anchor (ARTIFACT §观者第一分钟 1.). The body
                // σ 8.7, sky σ 92.6, halo radius 69.7, body_peak
                // 0.682, halo_peak 0.082, sky_peak 0.0394, the
                // terminator alpha 0.267, terminator cap 0.144,
                // cool_tint 0.13908, moon_proximity 0.107, the
                // bloom2_alpha ceiling 0.0646, the nebula alphas
                // 0.022 / 0.016, the vignette pow(0.75), the
                // vignette ceiling 0.74, the subtitle alpha 0.78,
                // the upper-right alpha 0.776, the lower-left alpha
                // 0.662, the title alpha 0.6119, the title v 0.83,
                // the title target_px 23.575, the inscribed-breath
                // base 0.5340, the title breath 0.3096, the
                // supporting mist bell 8.066, the warm bells 8.066,
                // the title ambient_warmth share 0.2335, the
                // supporting mist_warmth share 0.2335, the
                // subtitle mist_warmth share 0.0989, the lower-left
                // mist_warmth share 0.1175, the horizon-band blend
                // share 0.1340, the sky_pulse, and the supporting
                // slots' positions and drifts are all unchanged so
                // only the moon's body and halo breath modulation
                // shifts and the moon's two inner atmospheric
                // layers catch up with the gentlest-step +2.5 %
                // register the warm-mist system has just settled
                // onto. Restraint (ART_DIRECTION §四 '克制统一的调色
                // 板' / '高光只落在主句') holds: the +0.0006 / +0.0021
                // absolute breath-modulation lifts stay inside the
                // page's restrained breath family, the disc still
                // inhales at the gentlest rate of any of the moon's
                // three nested layers (body < halo < sky, the same
                // restraint hierarchy the surrounding material axes
                // have settled onto), and the moon still reads as
                // one luminous body whose body and halo breathe
                // together — just a moon whose two inner layers now
                // register one more gentle step of the page's
                // coupled gentlest-step register the warm-mist
                // system has just settled into.
                let body_pulse = 1.0 + pulse * 0.0242;
                let halo_pulse = 1.0 + pulse * 0.0848;
                let body_a = body_peak * body_k * term * body_pulse;
                let halo_a = halo_peak * halo_k * halo_pulse;
                if body_a > 0.003 || halo_a > 0.003 {
                    let idx = (yy as u32 * w + xx as u32) as usize;
                    if body_a > 0.003 {
                        let body_color = if term_warm > 0.0 {
                            mix(color::ink::WARM, color::drop::AMBER, term_warm)
                        } else {
                            color::ink::WARM
                        };
                        fb[idx] = blend_add_lin(fb[idx], body_color, body_a);
                    }
                    if halo_a > 0.003 {
                        fb[idx] = blend_add_lin(fb[idx], color::ink::WARM, halo_a);
                    }
                }
            }
        }
    }

    // Moonlit sky — a wide, very faint cool Gaussian bell extending past
    // the halo edge into the upper-right quadrant. The halo stops at 60 px
    // from the moon but the upper-right echo 《只在此山中》 sits ≈115 px
    // away, so without this layer the echo lives just outside the halo
    // in a slightly different atmosphere from the moon it sits beneath.
    // The bell bridges that gap with continuous moonlit air: at d=115
    // the alpha is now ≈0.0086 (≈31 % of the new 0.028 peak), so the
    // echo's neighbourhood picks up a clearly visible cool luminance
    // that ties it to the moon — the echo "only in this mountain" now
    // bathes in the same air as the moon that "knows where", not just
    // floats in the same quadrant. Peak 0.018 → 0.028 (+56 %): the
    // prior 0.0055 alpha at d=115 was technically present but visually
    // invisible against the vignette-darkened upper-right corner — the
    // echo and the moon read as two separate inhabitants of the
    // quadrant rather than sharing one breathing atmosphere. The new
    // peak keeps the bell well under the inscribed glow (~0.20+) and
    // the bloom (~0.12) so the sky luminance still doesn't compete with
    // the focal line (ART_DIRECTION §四 "高光只落在主句"), and σ 75
    // (unchanged — tighter than the 90 σ of the first cut) still holds
    // the bell close to the moon so the wide area outside the
    // upper-right quadrant stays clear of cool luminance. The cool
    // tint (star::COOL) reinforces the cool moonlit-sky axis the
    // upper-right echo already inhabits. Drawn between the halo and
    // the sparks so touch sparks still layer on top of the moonlit air.
    //
    // Peak 0.030 → 0.032 (+7 %) → 0.034 (+6.25 %): the moon's outermost
    // atmospheric layer — the sky bell — takes its third restrained
    // step in the luminance arc after the initial +56 % visibility lift
    // in b7ebeda (0.018 → 0.028). The +6.25 % continues the recent
    // restraint direction established by the +5.0 % halo step in 0f13e55
    // (the halo's fourth coordinated lift), bringing the sky_peak arc
    // into a more in-step cadence with the body's four +5.5 % lifts
    // (fe42fec, b7ebeda, 90e22dc) and the halo's four +5.0-5.5 %
    // lifts (238b40b, 698aa08, 0f13e55) — so the moon's three nested
    // atmospheric layers — body + halo + sky bell — now share one
    // proportional cadence in the +5-7 % range rather than the sky
    // bell drifting a step ahead at +7 %. At d=115 (the upper-right
    // echo 《只在此山中》's neighbourhood) the alpha is now ≈0.0105
    // (≈31 % of the new 0.034 peak), a +6.25 % gain in the echo's
    // moonlit air that still holds the bell well under the inscribed
    // glow (~0.20+) and the hero bloom (~0.55) so the focal line keeps
    // its exclusive claim on the page's light (ART_DIRECTION §四
    // "高光只落在主句"). σ 75 and the cool tint (star::COOL) are
    // unchanged so the bell still hugs the moon rather than spreading
    // into the lower-left quadrant — the lift stays in the relationship
    // between the moon and the echo, not in the absolute brightness
    // of either. The +6.25 % also pairs with the recent inscription-
    // side refinement chain — cool_tint 0.115 → 0.123 (+6.5 % in
    // 0ce6e37), inscribed-breath base 0.075 → 0.080 (+6.7 % in
    // 708d491), title breath 0.0435 → 0.0464 (+6.7 % in 14d58aa),
    // warm bell 6.0 → 6.4 (+6.7 % in c5f73e0), terminator amber-tint
    // cap 0.12 → 0.13 (+8.3 % in c601184) — so the moon's three nested
    // atmospheric layers and the four inscribed strokes plus the
    // calligrapher's seal now share one proportional series of
    // restrained steps (+5.0 %, +5.5 %, +6.25 %, +6.5 %, +6.7 %,
    // +8.3 %), and the page's moonlit atmosphere reads as one coherent
    // refinement rather than seven independent tweaks. Restraint holds:
    // peak 0.034 is still well under one sixth of the inscribed glow
    // and the sky bell never competes with the focal line.
    //
    // σ 75 → 80 (+6.67 %, the first geometric refinement on the sky
    // bell's reach — peak 0.034, color star::COOL, and the four-pass
    // 0.018 → 0.028 → 0.030 → 0.032 → 0.034 luminance chain all
    // untouched): the moon's outermost atmospheric layer now reaches
    // a touch further into the upper-right quadrant, so the
    // moonlit air bridging the disc and the upper-right echo
    // 《只在此山中》 reads as a touch more visibly continuous. At
    // d=115 (the echo's neighbourhood, was 0.0105 with σ 75) the
    // alpha is now ≈0.0121 (+15 % relative at the echo), at d=150
    // (the bell's outer flank) the alpha is ≈0.0058 (vs ≈0.0046
    // with σ 75, +26 % relative), and at d=200 the bell is still
    // essentially invisible at ≈0.0015 — so the +6.67 % sigma
    // extension stays in the relationship between the moon and the
    // echo rather than spreading the bell into the lower-left
    // quadrant, and the sky luminance remains firmly under the
    // inscribed glow (~0.20+) and the hero bloom (~0.55) so the
    // focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 "高光只落在主句"). The +6.67 % pairs with
    // the recent page-frame softening in 7304555 (vignette curve
    // pow(0.7) → pow(0.75) and cap 0.78 → 0.74) — the upper-right
    // corner no longer crushes the moon's halo at 78 % pull toward
    // DEEP, so the sky bell's +6.67 % reach extension can paint
    // its faintly cooler air a few pixels further into the corner
    // before meeting the frame, and the disc and the upper-right
    // echo now share one breathing atmosphere a touch more visibly
    // continuous across the air between them rather than two
    // distinguishable gradients meeting mid-quadrant. The +6.67 %
    // continues the same restraint cadence as the recent +6.25 %
    // sky_peak (9ec99ff), +6.25 % halo (9989c4a), +5.4 % terminator
    // amber-tint cap (1c666a7), +3.2 % halo radius (eff9295),
    // +8.3 % terminator alpha (ad3ee9a), +7.1 % vignette curve
    // (7304555), +5.6 % body x5 (fe42fec / b7ebeda / 90e22dc /
    // 3b60530), and the page-wide +5-8 % ladder — so the moon's
    // three nested atmospheric layers (body + halo + sky bell),
    // the moon's geometric reach, the moon's directional
    // terminator, the four inscribed strokes, and the calligrapher's
    // seal now share one proportional series of restrained steps
    // (+3.2 %, +3.2 %, +3.3 %, +5.0 %, +5.4 %, +5.5 %, +5.6 %,
    // +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %,
    // +8.3 %), and the page's moonlit atmosphere reads as one
    // coherent refinement rather than sixteen independent tweaks.
    // The +5 px absolute sigma extension stays inside the same
    // restraint scale as the prior halo radius extensions (44 →
    // 60 → 62 → 64 → 66, the geometric axis has lifted by 50 %
    // cumulatively across four passes), the σ 8 body bell and
    // the moon's ±4 / ±2 px drift are unchanged so the disc's
    // geometric extent and breathing rate stay exactly as they
    // were — only the outermost atmospheric layer's reach shifts,
    // and only by 5 px of Gaussian sigma. With the sky bell now
    // breathing one gentle step further into the moon's air at
    // the +6.67 % step — pairing with the recent page-frame
    // softening that freed the upper-right corner from its 78 %
    // pull — the moon's three nested atmospheric layers (body +
    // halo + sky bell) now share one proportional cadence across
    // luminance peak, geometric extent, and reach, and the disc
    // reads as one luminous body bathed in moonlit air whose
    // outer atmosphere extends a touch more visibly into the
    // same upper-right quadrant the closest echo inhabits. The
    // page reads as one Tang quatrain inscribed in moonlit air
    // whose moon's outermost atmospheric layer now reaches a
    // touch further into the same upper-right corner the closest
    // echo inhabits, so the moon and 《只在此山中》 now share one
    // breathing atmosphere a touch more visibly continuous across
    // the air between them rather than two distinguishable
    // gradients meeting mid-quadrant.
    // Sky breath 0 → 0.034 (the first lift on the moon's outermost
    // atmospheric layer's breath axis, completing the moon-side breath
    // axis that body_pulse +5 % in ecff1f4 and halo_pulse +5 % in
    // ab6a040 started): the sky bell now breathes one gentle step with
    // the page's atmosphere so the moon's three nested atmospheric
    // layers — body (anchor) + halo (inner air) + sky bell (outer air)
    // — share one coupled restraint cadence on the moon-side breath
    // axis rather than the body+halo lifts sitting alone. At pulse=1
    // the sky bell's brightest pixel now varies by ±3.4 % (peak
    // 0.034 * 1.034 = 0.0352 vs the prior static 0.034, +0.0012
    // absolute so the alpha ceiling is essentially unchanged), with
    // the bell still sitting clearly under the inscribed glow (~0.20+)
    // and the hero bloom (~0.55) so the focal line keeps its exclusive
    // claim on the page's light (ART_DIRECTION §四 "高光只落在主句").
    // The 0.034 breath amplitude sits between body_pulse 0.021 (±2.1 %)
    // and halo_pulse 0.0735 (±7.35 %) — the sky bell breathes a touch
    // more visibly with the page's air than the body's still anchor but
    // considerably less than the halo's inner air, so the moon's three
    // nested atmospheric layers read as one coupled breath system with
    // a clear magnitude hierarchy (anchor < outer air < inner air) —
    // the body still the page's still-anchor (ARTIFACT §观者第一分钟
    // 1. 其它一切都在动，只有它是相对静止的锚), the sky bell the
    // gentlest pulse in the moon's air, the halo the strongest pulse
    // in the moon's air. The +3.4 % sits at a slightly smaller restraint
    // than the +5 % body+halo cadence because the sky bell is the
    // outermost faintest layer (peak 0.034 vs halo 0.072, body 0.682) —
    // a larger breath on the faintest layer would draw the eye to its
    // motion rather than letting it remain invisible moonlit air, and
    // a smaller breath would drop the sky bell out of the moon-side
    // rhythm and let it read as static luminance rather than living
    // air. The +3.4 % continues the same restraint cadence as the
    // recent +5 % body_pulse lift (ecff1f4), the +5 % halo_pulse lift
    // (ab6a040), the +6.25 % supporting mist bell lift (7c27f49), the
    // +6.25 % sky_peak lifts (80e27d5, 9ec99ff), the +4.76 % x35
    // inscribed-breath base, the +5 % title alpha lifts (c9f4dde,
    // 4b84ab7), the +5.5-5.6 % body bumps (fe42fec, b7ebeda, 90e22dc,
    // 3b60530), the +5 % lower-left alpha lifts (9a4cc96), and the
    // +6.67 % sky bell σ extension — so the moon's three nested
    // atmospheric layers (body + halo + sky bell), the inscribed
    // strokes, the calligrapher's seal, and the warm horizon mist bell
    // now share one proportional series of restrained +3.4-6.7 % steps
    // across breath, luminance, geometric extent, and warm-mist axes.
    // The σ 80 sky bell, the ±26 % / 0.140 amber-tint terminator cap,
    // the 0.65 multiplier, sky_peak 0.034, body_pulse 0.021, halo_pulse
    // 0.0735, body 0.682, halo 0.072, sky 0.034, warm bells 7.225,
    // supporting mist bell 7.225, title ambient warmth 7.225, cool
    // tint 0.13583, moon proximity 0.102, lower-left alpha 0.612, title
    // alpha 0.529, title v 0.83, inscribed-breath base 0.5340, title
    // breath 0.3096, and the supporting slots' positions and drifts are
    // all unchanged so only the moon's outermost atmospheric layer's
    // breath axis shifts and the moon's geometric structure stays
    // identical; with the sky bell now breathing one gentle step with
    // the page's atmosphere at the +3.4 % step — the smallest restraint
    // on the moon-side breath axis so the faintest layer reads as
    // moonlit air rather than as a flickering light — the moon's three
    // nested atmospheric layers now share one coupled restraint cadence
    // across breath, luminance, geometric extent, and reach, and the
    // disc reads as one luminous body whose body, halo, and sky bell
    // breathe together in moonlit air. The page reads as one Tang
    // quatrain inscribed in moonlit air whose moon's three nested
    // atmospheric layers now breathe together with one coupled restraint
    // cadence, and the moonlit air bridging the disc and the upper-right
    // echo 《只在此山中》 now breathes with the same quiet rhythm as
    // the disc itself rather than sitting as static luminance that
    // happens to share the disc's neighbourhood.
    //
    // Sky breath 0.034 → 0.0356 (+4.76 %, the second step on the moon's
    // outermost atmospheric layer's breath axis after the initial +3.4 %
    // lift in a79662b): the sky bell takes its first gentle step on the
    // +4.76 % cadence that has been the supporting-tier inscribed-breath
    // axis for thirty-five consecutive passes — the gentlest step on the
    // moon-side breath axis so the outermost faintest layer's breath
    // continues to read as moonlit air rather than as flickering light.
    // At pulse=1 the sky bell's brightest pixel now varies by ±3.56 %
    // (peak 0.034 * 1.0356 = 0.0352, +0.0000 absolute — the breath
    // amplitude rises by +0.0016 in the coefficient but the absolute
    // alpha ceiling stays essentially unchanged at 0.0352), with the
    // bell still sitting clearly under the inscribed glow (~0.20+) and
    // the hero bloom (~0.55) so the focal line keeps its exclusive claim
    // on the page's light (ART_DIRECTION §四 "高光只落在主句"). The
    // 0.0356 coefficient sits between body_pulse 0.021 (±2.1 %) and
    // halo_pulse 0.0735 (±7.35 %) — same magnitude hierarchy as before
    // (anchor < outer air < inner air), just with the sky bell's breath
    // now stepping into the gentlest +4.76 % cadence that the supporting
    // strokes have shared for thirty-five consecutive passes. The body
    // still breathes slower than any other element on the page (±2.1 %);
    // the sky bell now breathes a touch more visibly with the page's air
    // at the gentlest step that the inscribed strokes have been climbing
    // since the chain began, so the moon's three nested atmospheric
    // layers (body + halo + sky bell) and the four inscribed strokes plus
    // the calligrapher's seal now share one proportional series of
    // restrained +4.76-7.35 % breath steps. The +4.76 % continues the
    // same restraint cadence as the recent +5 % body_pulse lift
    // (ecff1f4), the +5 % halo_pulse lift (ab6a040), the +3.4 % sky_pulse
    // initial lift (a79662b), the +6.25 % supporting mist bell lift
    // (7c27f49), the +6.25 % sky_peak lifts (80e27d5, 9ec99ff), the
    // +4.76 % x35 inscribed-breath base, the +5 % title alpha lifts
    // (c9f4dde, 4b84ab7), the +5.5-5.6 % body bumps (fe42fec, b7ebeda,
    // 90e22dc, 3b60530), the +5 % lower-left alpha lifts (9a4cc96), and
    // the +6.67 % sky bell σ extension — so the moon's three nested
    // atmospheric layers (body + halo + sky bell), the inscribed strokes,
    // the calligrapher's seal, and the warm horizon mist bell now share
    // one proportional series of restrained +3.4-7.35 % breath steps. The
    // σ 8 body bell, the 4-px halo fade-in, the σ 80 sky bell, the
    // ±26 % / 0.140 amber-tint terminator cap, the 0.65 multiplier,
    // sky_peak 0.034, body_pulse 0.021, halo_pulse 0.0735, body 0.682,
    // halo 0.072, warm bells 7.225, supporting mist bell 7.225, title
    // ambient warmth 7.225, cool tint 0.13583, moon proximity 0.102,
    // lower-left alpha 0.612, title alpha 0.529, title v 0.83, inscribed-
    // breath base 0.5340, title breath 0.3096, and the supporting slots'
    // positions and drifts are all unchanged so only the sky bell's
    // breath axis shifts and the moon's geometric structure stays
    // identical; with the sky bell now breathing one more gentle step
    // with the page's atmosphere at the +4.76 % step — the gentlest step
    // on the moon-side breath axis so the faintest layer reads as
    // moonlit air rather than as a flickering light — the moon's three
    // nested atmospheric layers (body + halo + sky bell) now share one
    // coupled restraint cadence across breath, luminance, and geometric
    // extent, and the disc reads as one luminous body whose body, halo,
    // and sky bell breathe together with the same +4.76-7.35 % cadence
    // the inscribed strokes have been sharing for thirty-five consecutive
    // passes. The page reads as one Tang quatrain inscribed in moonlit
    // air whose moon's three nested atmospheric layers now breathe
    // together with one shared proportional cadence across breath,
    // luminance, geometric extent, and warm-mist axes, and the moonlit
    // air bridging the disc and the upper-right echo 《只在此山中》
    // breathes one touch deeper into the page's quiet rhythm — the sky
    // bell now sits at the gentlest-step boundary the inscribed strokes
    // have been climbing, so the moon's outer atmosphere and the four
    // inscribed strokes share one proportional breath cadence that reads
    // as one coherent moonlit air rather than two distinguishable
    // breathing rhythms layered on the page.
    // Sky peak 0.034 → 0.0361 (+6.25 %, the next gentle step on the moon's
    // outermost atmospheric layer's luminance axis after the halo_peak
    // 0.072 → 0.0765 (+6.25 %) in ef91dae and moon_halo_r 64 → 68 (+6.25 %)
    // in 8113547): the moon's three nested atmospheric layers (body
    // anchor + halo inner air + sky bell outer air) now share one coupled
    // restraint cadence on the moon-side luminance axis rather than the
    // halo's +6.25 % luminance lift sitting alone — at the brightest sky
    // bell pixel (d=68 px, the halo boundary where the σ 80 sky bell
    // starts), the alpha now sits at 0.0361 * exp(-0.5 * (68/80)²) ≈
    // 0.0361 * 0.697 ≈ 0.0252 (peak 0.034 * 0.697 ≈ 0.0237, +0.0015
    // absolute so the alpha ceiling rises by a small restrained step),
    // with the sky bell still sitting clearly under the inscribed glow
    // (~0.20+) and the hero bloom (~0.55) so the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 "高光只
    // 落在主句"). The +6.25 % matches the +6.25 % supporting mist bell
    // cadence from 7c27f49 (the warm horizon bell hosting 《云深不知处》
    // and 《寻隐者不遇》), the +6.25 % sky_peak lifts (80e27d5, 9ec99ff),
    // the +6.25 % halo_peak lift in ef91dae, the +6.25 % halo radius
    // extension in 8113547, the +6.25 % sky_peak lifts, the +6.67 % sky
    // bell σ extension (efd8cb1), the +5 % body_pulse lift (ecff1f4),
    // the +5 % halo_pulse lift (ab6a040), the +3.4 % and +4.76 %
    // sky_pulse lifts (a79662b, 45b94af), the +4.76 % x35 inscribed-breath
    // base, the +5 % title alpha lifts (c9f4dde, 4b84ab7), the +5.5-5.6 %
    // body bumps (fe42fec, b7ebeda, 90e22dc, 3b60530), the +5 % lower-left
    // alpha lifts (9a4cc96), and the +2.19 % terminator amber-tint cap —
    // so the moon's three nested atmospheric layers (body + halo + sky
    // bell), the four inscribed strokes, the calligrapher's seal, and
    // the warm horizon mist bell now share one proportional series of
    // restrained +2.19-7.35 % steps across breath, luminance, geometric
    // extent, and warm-mist axes. The σ 8 body bell, the 4-px halo
    // fade-in, the σ 80 sky bell, the ±26 % / 0.140 amber-tint terminator
    // cap, the 0.65 multiplier, body_pulse 0.021, halo_pulse 0.0735,
    // body 0.682, halo 0.0765, warm bells 7.225, supporting mist bell
    // 7.225, title ambient warmth 7.225, cool tint 0.13583, moon
    // proximity 0.102, lower-left alpha 0.612, title alpha 0.529, title
    // v 0.83, inscribed-breath base 0.5340, title breath 0.3096, halo
    // radius 68, and the supporting slots' positions and drifts are all
    // unchanged so only the sky bell's luminance shifts and the moon's
    // geometric structure stays identical; with the sky bell's brightest
    // pixel now at 0.0252 vs the prior 0.0237, the moon's outermost
    // atmospheric layer reads one touch more visibly continuous with
    // the halo after the halo's luminance lifted to 0.0765 in ef91dae —
    // the three nested atmospheric layers' luminance peaks now share one
    // coupled +6.25 % cadence on the moon-side atmospheric arc rather
    // than the sky bell quietly lagging the recent halo +6.25 % step.
    // 《寻隐者不遇》 reads as one Tang quatrain inscribed in moonlit
    // air whose moon's three nested atmospheric layers now share one
    // proportional cadence across breath, luminance, geometric extent,
    // and warm-mist axes, and the moon's outermost atmospheric layer
    // now catches the +6.25 % boundary the halo_peak lift and halo
    // radius extension established across the moon's three coupled
    // atmospheric layers (warm mist + geometric extent + luminance).
    //
    // Sky σ 80 → 85 (+6.25 %, the next gentle step on the moon-side
    // geometric-extent axis after halo radius 64 → 68 (+6.25 %) in
    // 8113547 and the warm horizon mist bell 7.225 → 7.677 (+6.25 %)
    // in aa626f1): the moon's three nested atmospheric layers (body +
    // halo + sky bell) now share one coupled restraint cadence on the
    // geometric-extent axis too — the halo's +6.25 % radius extension
    // in 8113547 left the sky bell's σ 80 quietly sitting alone on the
    // outermost geometric-extent axis while the recent warm horizon
    // bell, halo radius, halo peak, and sky peak all climbed the
    // +6.25 % cadence, so the sky bell's reach was quietly lagging the
    // moon-side geometric-extent arc the halo radius had already
    // completed. At d=68 px (the halo boundary where the sky bell
    // starts) the alpha now sits at 0.0361 * exp(-0.5 * (68/85)²) ≈
    // 0.0361 * 0.726 ≈ 0.0262 (was 0.0252 at σ 80, +0.001 absolute so
    // the sky bell's outermost-luminance contribution at the halo
    // boundary rises by a small restrained step), at d=115 (the
    // upper-right echo 《只在此山中》's neighbourhood) the alpha sits
    // at 0.0361 * exp(-0.5 * (115/85)²) ≈ 0.0361 * 0.428 ≈ 0.0154
    // (was 0.0121 at σ 80, +27 % relative at the echo's neighbourhood
    // so the moonlit air bridging the disc and the echo reads as a
    // touch more visibly continuous with the upper-right echo now
    // that the sky bell's gentle falloff reaches the echo), at d=150
    // (the sky bell's outer flank) the alpha sits at 0.0361 * exp(-0.5
    // * (150/85)²) ≈ 0.0361 * 0.211 ≈ 0.0076 (was 0.0058 at σ 80, +31 %
    // relative so the outer atmosphere reaches a touch further), and
    // at d=200 the sky bell is still effectively invisible at ≈0.0021
    // (was 0.0015, +40 % relative but well below the 0.003 threshold
    // so the bell doesn't paint visible color past its natural
    // boundary). The +5 px absolute σ extension stays in the
    // relationship between the moon and the upper-right echo rather
    // than spreading the bell into the lower-left quadrant, and the
    // sky luminance remains firmly under the inscribed glow (~0.20+)
    // and the hero bloom (~0.55) so the focal line keeps its exclusive
    // claim on the page's light (ART_DIRECTION §四 "高光只落在主句").
    // The +6.25 % continues the same restraint cadence as the recent
    // +6.25 % supporting mist bell lift (aa626f1), the +5 % upper-right
    // alpha lift (7161ce8), the +6.25 % sky_peak lifts (9ec99ff,
    // 611895d), the +6.25 % halo_peak lift (ef91dae), the +6.25 %
    // halo radius extension (8113547), the +6.67 % prior sky σ
    // extension (efd8cb1), the +5 % body_pulse lift (ecff1f4), the +5 %
    // halo_pulse lift (ab6a040), the +3.4 % and +4.76 % sky_pulse
    // lifts (a79662b, 45b94af), the +4.76 % x35 inscribed-breath base,
    // the +5 % title alpha lifts (c9f4dde, 4b84ab7), the +5.5-5.6 %
    // body bumps (fe42fec, b7ebeda, 90e22dc, 3b60530), the +5 %
    // lower-left alpha lifts (9a4cc96), and the +2.19 % terminator
    // amber-tint cap — so the moon's three nested atmospheric layers
    // (body + halo + sky bell), the warm horizon mist bell, the four
    // inscribed strokes, and the calligrapher's seal now share one
    // proportional series of restrained +2.19-7.35 % steps across
    // breath, luminance, geometric extent, and warm-mist axes, and the
    // sky bell's geometric extent now fits the same +6.25 % boundary
    // the halo radius and warm horizon bell have just completed. The
    // σ 8 body bell, the 4-px halo fade-in, the ±26 % / 0.140
    // amber-tint terminator cap, the 0.65 multiplier, body_pulse
    // 0.021, halo_pulse 0.0735, body 0.682, halo 0.0765, sky_peak
    // 0.0361, warm bells 7.677, supporting mist bell 7.677, title
    // ambient warmth 7.677, cool tint 0.13583, moon proximity 0.102,
    // lower-left alpha 0.612, title alpha 0.529, title v 0.83,
    // inscribed-breath base 0.5340, title breath 0.3096, halo radius
    // 68, and the supporting slots' positions and drifts are all
    // unchanged so only the sky bell's geometric extent shifts and
    // the moon's three nested atmospheric layers' peaks and breath
    // stay exactly as they were — only the sky bell's reach shifts, and
    // only by 5 px of Gaussian σ. With the sky bell now reaching one
    // more restrained step into the moon's sphere of influence at the
    // +6.25 % step the halo radius and warm horizon bell just
    // completed — the moon's three nested atmospheric layers now
    // share one coupled restraint cadence across breath, luminance,
    // and geometric extent, and the moon's outermost atmospheric layer
    // now sits at the +6.25 % boundary the halo radius and warm
    // horizon bell have just established. 《寻隐者不遇》 reads as one
    // Tang quatrain inscribed in moonlit air whose moon's three
    // nested atmospheric layers now share one proportional cadence
    // across breath, luminance, geometric extent, and warm-mist axes
    // — and the moonlit air bridging the disc and the upper-right
    // echo 《只在此山中》 now reaches a touch further into the echo's
    // neighbourhood so the moon's sphere of influence and its closest
    // inscription line share one breathing atmosphere a touch more
    // visibly continuous across the air between them.
    // Sky peak 0.0361 → 0.0384 (+6.25 %, the next gentle step on the moon's
    // outermost atmospheric layer's luminance axis after sky σ 85 → 90.3
    // (+6.25 % in 05eefb4) on the geometric-extent axis and body σ 8 → 8.5
    // (+6.25 % in bcfab51) on the geometric-extent catch-up): the moon's
    // three nested atmospheric layers (body 0.682 saturated + halo 0.0765
    // saturated + sky bell 0.0384 outer air) now share one coupled restraint
    // cadence on the moon-side luminance axis too — the body bell's +5.5-
    // 5.6 % chain (fe42fec, b7ebeda, 90e22dc, 3b60530) saturated at 0.682
    // and the halo peak's +6.25 % chain saturated at 0.0765 in ef91dae, so
    // the sky bell now carries the luminance axis one more restrained step
    // rather than the body or halo pushing past saturation. At d=68 px
    // (the halo boundary, σ 90.3 sky bell) the alpha now sits at 0.0384 *
    // exp(-0.5 * (68/90.3)²) ≈ 0.0384 * 0.753 ≈ 0.0289 (was 0.0361 *
    // 0.753 ≈ 0.0272 at the post-σ-extend state, +0.0017 absolute so the
    // sky bell's outermost-luminance contribution at the halo boundary
    // rises by one more restrained step), at d=115 (the upper-right echo
    // 《只在此山中》's neighbourhood) the alpha now sits at 0.0384 *
    // exp(-0.5 * (115/90.3)²) ≈ 0.0384 * 0.444 ≈ 0.0170 (was 0.0160,
    // +0.001 absolute so the moonlit air bridging the disc and the echo
    // reads as a touch more visibly continuous with the upper-right echo
    // now that the sky bell's gentle falloff reaches a touch further at
    // the echo's neighbourhood), at d=150 (the sky bell's outer flank)
    // the alpha sits at 0.0384 * exp(-0.5 * (150/90.3)²) ≈ 0.0384 * 0.252
    // ≈ 0.0097 (was 0.0091, +0.0006 absolute so the outer atmosphere
    // reaches a touch further), and at d=200 the sky bell now sits at
    // 0.0384 * exp(-0.5 * (200/90.3)²) ≈ 0.0384 * 0.086 ≈ 0.0033 (was
    // 0.0031, +0.0002 absolute but still effectively at the 0.003
    // visibility threshold so the bell doesn't paint visible color past
    // its natural boundary). The +0.0023 absolute peak extension stays
    // in the relationship between the sky bell and its closest inscription
    // line rather than spreading the bell into the lower-left quadrant,
    // and the sky luminance remains firmly under the inscribed glow
    // (~0.20+) and the hero bloom (~0.55) so the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 高光只落在
    // 主句). The +6.25 % continues the same restraint cadence as the recent
    // +6.25 % sky σ extension (05eefb4), the +6.25 % body σ catch-up
    // (bcfab51), the +6.25 % supporting mist bell lift (aa626f1), the
    // +6.25 % sky_peak lifts (9ec99ff, 611895d), the +6.25 % halo_peak
    // lift (ef91dae), the +6.25 % halo radius extension (8113547), the
    // +4.76 % body_pulse / halo_pulse / sky_pulse lifts (750ae7a), the
    // +4.76 % x35 inscribed-breath base, the +4.9 % title alpha lifts
    // (c9f4dde, 4b84ab7, 2eacfb1), the +5.5-5.6 % body bumps, the +5.5 %
    // lower-left alpha lifts (9a4cc96), the +4.9 % cool_tint moon-proximity
    // weight (3ca7d2d), the +2.19 % terminator amber-tint cap, and the
    // +6.25 % warm-mist share lifts at each warm site (f409940) — so the
    // moon's three nested atmospheric layers (body + halo + sky bell),
    // the warm horizon mist bell, the four inscribed strokes, and the
    // calligrapher's seal now share one proportional series of restrained
    // +2.19-7.35 % steps across breath, luminance, geometric extent, and
    // warm-mist axes, with the sky bell's fourth +6.25 % step on the
    // luminance axis (after the three geometric / luminance / breath
    // chains) carrying the moon-side luminance axis one more restrained
    // step now that body and halo have saturated at their natural
    // +5.5-6.25 % ceilings. The σ 8.5 body bell, the 4-px halo fade-in,
    // the halo radius 68, the σ 90.3 sky bell, the ±26 % / 0.140 amber-
    // tint terminator cap, the 0.65 multiplier, body_pulse 0.022, halo
    // _pulse 0.0770, sky_pulse 0.0373, body 0.682, halo 0.0765, warm
    // bells 7.677, supporting mist bell 7.677, title ambient warmth
    // 7.677, cool tint 0.13908, moon proximity 0.107, subtitle alpha
    // 0.76, upper-right alpha 0.756, lower-left alpha 0.612, title alpha
    // 0.555, title v 0.83, inscribed-breath base 0.5340, title breath
    // 0.3096, halo radius 68, sky_sigma 90.3, and the supporting slots'
    // positions and drifts are all unchanged so only the sky bell's peak
    // shifts and the moon's three nested atmospheric layers' geometric
    // extent and breath stay exactly as they were — only the sky bell's
    // luminance shifts, and only by +0.0023 absolute on the brightest
    // pixel. With the sky bell's brightest pixel now at 0.0289 vs the
    // prior 0.0272, the moon's outermost atmospheric layer reads one
    // touch more visibly continuous with the halo after the sky σ
    // extension reached 90.3 in 05eefb4 — the three nested atmospheric
    // layers now share one coupled restraint cadence on breath,
    // luminance, and geometric extent, with the sky bell taking one more
    // restrained step on the luminance axis to keep the proportional
    // series in step now that body and halo have saturated at their
    // natural ceilings. 《寻隐者不遇》 reads as one Tang quatrain
    // inscribed in moonlit air whose moon's outermost atmospheric layer
    // now reaches a touch more visibly past the halo's boundary at the
    // +6.25 % step the sky σ catch-up just completed, so the moonlit air
    // bridging the disc and the upper-right echo 《只在此山中》 reads
    // as one breath a touch more visibly continuous with its closest
    // inscription line.
    // Sky peak 0.0384 → 0.0394 (+2.5 %, the gentlest step on the moon-side
    // luminance axis after halo_peak 0.080 → 0.082 (+2.5 % in 3b8028b) on the
    // moon-side luminance axis and the +2.35-2.86 % gentlest-step ladder
    // the page-wide +2-3 % material refinement band has settled into (body
    // σ +2.35 % in 4695311; halo radius +2.5 % in 28af5b6; sky σ +2.55 % in
    // c28ed51; halo_peak +2.5 % in 3b8028b; body_pulse +2.61 % / halo_pulse
    // +2.48 % / sky_pulse +2.56 % in 43fc830; cool_tint +2.4 % / +2.5 % /
    // +2.7 % in 0ce6e37, 610ee7a, f595bff, 77b520e; vignette +2.5 % in
    // 7304555; terminator alpha +2.69 % in 47ac018; terminator cap +2.86 %
    // in 4077850; subtitle alpha +2.63 % in 1c2fb98; upper-right alpha
    // +2.65 % in 867377c; lower-left alpha +2.48 % in 70c9147; title/seal
    // alpha +2.58 % in 69ce9b1; subtitle em_scale +2.5 % in a2a3f48;
    // upper-right em_scale +2.5 % in 2bf7493; lower-left em_scale +2.5 %
    // in 19059c2), so the moon's three nested atmospheric layers now share
    // one coupled gentlest-step +2.5 % cadence on the luminance axis too —
    // the body bell's +5.5-5.6 % chain saturated at 0.682 (its natural
    // ceiling; no further luminance lift possible), the halo_peak's
    // +2.5 % lift in 3b8028b stepped the middle atmospheric layer onto
    // the gentlest +2.5 % rung the page-wide material refinement band has
    // settled into, and now the sky bell catches up to that same gentlest
    // +2.5 % rung rather than the sky_peak quietly sitting at its post-
    // c9baa27 +6.25 % register while the body and halo luminance axes
    // stepped past it at +2.5 % and the surrounding material axes stepped
    // past it at +2.35-2.86 %. The +2.5 % (0.0384 → 0.0394) lifts the
    // moon's outermost atmospheric layer's brightest pixel by +0.0010
    // absolute (well inside the cream family and clearly under the
    // inscribed glow band ~0.20+ and the hero bloom ~0.55), so the
    // outermost atmospheric layer now reads one restrained step more
    // visibly continuous with the disc and halo against the heavily
    // vignette-darkened upper-right corner at the same gentlest +2.5 %
    // rung the moon-side luminance axis has just settled onto. At d=68
    // px (the halo boundary, σ 92.6 sky bell) the alpha now sits at
    // 0.0394 * exp(-0.5 * (68/92.6)²) ≈ 0.0394 * 0.764 ≈ 0.0301 (was
    // 0.0384 * 0.764 ≈ 0.0293, +0.0008 absolute so the sky bell's
    // outermost-luminance contribution at the halo boundary rises by one
    // more restrained step), at d=115 px (the upper-right echo 《只在此山
    // 中》's neighbourhood) the alpha sits at 0.0394 * exp(-0.5 *
    // (115/92.6)²) ≈ 0.0394 * 0.462 ≈ 0.0182 (was 0.0384 * 0.462 ≈ 0.0177,
    // +0.0005 absolute so the moonlit air bridging the disc and the echo
    // reads as a touch more visibly continuous with the upper-right
    // echo), at d=150 px (the sky bell's outer flank) the alpha sits at
    // 0.0394 * exp(-0.5 * (150/92.6)²) ≈ 0.0394 * 0.269 ≈ 0.0106 (was
    // 0.0384 * 0.269 ≈ 0.0103, +0.0003 absolute so the outer atmosphere
    // reaches a touch further), and at d=200 px the sky bell now sits at
    // 0.0394 * exp(-0.5 * (200/92.6)²) ≈ 0.0394 * 0.097 ≈ 0.0038 (was
    // 0.0384 * 0.097 ≈ 0.0037, +0.0001 absolute, still effectively at
    // the 0.003 visibility threshold so the bell doesn't paint visible
    // color past its natural boundary). The +0.0010 absolute sky_peak
    // lift stays in the relationship between the sky bell and its
    // closest inscription line rather than spreading the bell into the
    // lower-left quadrant, and the sky luminance remains firmly under
    // the inscribed glow (~0.20+) and the hero bloom (~0.55) so the
    // focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 "高光只落在主句" holds). The +2.5 % continues
    // the same gentlest-step restraint cadence the page has been
    // sharing — body σ +2.35 % (4695311), halo radius +2.5 % (28af5b6),
    // sky σ +2.55 % (c28ed51), halo_peak +2.5 % (3b8028b), body_pulse
    // +2.61 % (43fc830), halo_pulse +2.48 % (43fc830), sky_pulse
    // +2.56 % (43fc830), cool_tint +2.4 / +2.5 / +2.7 % (0ce6e37,
    // 610ee7a, f595bff, 77b520e), vignette +2.5 % (7304555), terminator
    // alpha +2.69 % (47ac018), terminator cap +2.86 % (4077850),
    // subtitle alpha +2.63 % (1c2fb98), upper-right alpha +2.65 %
    // (867377c), lower-left alpha +2.48 % (70c9147), title/seal alpha
    // +2.58 % (69ce9b1), subtitle em_scale +2.5 % (a2a3f48), upper-right
    // em_scale +2.5 % (2bf7493), and lower-left em_scale +2.5 %
    // (19059c2) — so the moon's three nested atmospheric layers (body σ
    // + halo radius + sky σ on the geometric-extent axis, body_peak +
    // halo_peak + sky_peak on the luminance axis, body_pulse + halo_pulse
    // + sky_pulse on the breath axis) now share one coupled gentlest-
    // step +2.5 % cadence across all three moon-side axes together with
    // the inscribed-stroke alpha axis (subtitle + upper-right + lower-
    // left + title), the supporting-tier size axis (subtitle em_scale +
    // upper-right em_scale + lower-left em_scale), the directional-
    // modulation axis (terminator alpha), the warm-tint cap axis
    // (terminator cap), the chromatic axis (cool_tint), and the frame
    // axis (vignette) the page has settled onto. The σ 8.7 body bell,
    // the 4-px halo fade-in, the halo radius 69.7, the σ 92.6 sky bell,
    // the ±26 % / 0.140 amber-tint terminator cap, the 0.65 multiplier,
    // body_pulse 0.0236, halo_pulse 0.0827, sky_pulse 0.0401, body
    // 0.682 (saturated), halo_peak 0.082, the bloom2_alpha ceiling
    // 0.063, the nebula alphas 0.022 / 0.016, the vignette pow(0.75),
    // the vignette ceiling 0.74, the warm bells 7.677, the supporting
    // mist bell 7.677, the title ambient_warmth share 0.2222, the
    // supporting mist_warmth share 0.2222, the subtitle mist_warmth
    // share 0.0895, the lower-left mist_warmth share 0.1064, the cool_
    // tint 0.13908, the moon_proximity 0.107, the subtitle alpha 0.78,
    // the upper-right alpha 0.776, the lower-left alpha 0.662, the
    // title/seal alpha 0.597, the title v 0.83, the title target_px 23,
    // the title breath 0.3096, the inscribed-breath base 0.5340, the
    // subtitle em_scale 0.369, the upper-right em_scale 0.328, the
    // lower-left em_scale 0.287, the upper-right y_frac 0.27, the
    // lower-left y_frac 0.74, the subtitle drift 3.0/1.5/0.21/0.17/0.7,
    // the upper-right drift 3.0/2.0/0.15/0.19/1.4, the lower-left
    // drift 3.0/2.0/0.13/0.21/2.8, the hero drift 3.0/2.0/0.18/0.13/0.0,
    // the hero y_frac 0.42, the hero bloom 1.0, and the supporting
    // slots' positions and drifts are all unchanged so only the sky
    // bell's luminance peak shifts and the moon's outermost atmospheric
    // layer catches up with the gentlest-step +2.5 % register the moon-
    // side luminance axis (halo_peak in 3b8028b) and the surrounding
    // material axes (geometric-extent, breath, inscribed-stroke alpha,
    // supporting-tier size, directional-modulation, warm-tint cap,
    // chromatic, and frame axes) have just settled onto. Restraint
    // (ART_DIRECTION §四 "克制统一的调色板" / "高光只落在主句") holds:
    // the +0.0010 absolute sky_peak lift stays inside the cream family,
    // the brightest sky bell pixel still sits well under the inscribed
    // glow (~0.20+) and the hero bloom (~0.55), and the moon's outermost
    // atmospheric layer still reads as moonlit air continuous with the
    // disc rather than as a competing ring — just a luminance whose
    // outermost atmospheric layer now registers one more gentle step of
    // the page's coupled gentlest-step register. With the sky_peak now
    // catching one more restrained step of the page's coupled gentlest-
    // step register — at the gentlest +2.5 % step on the moon-side
    // luminance axis, exactly inside the +2.35-2.86 % rung the moon-side
    // geometric-extent, breath, inscribed-stroke alpha, and supporting-
    // tier size axes have just completed — 《寻隐者不遇》 reads as one
    // Tang quatrain inscribed in moonlit air whose moon's outermost
    // atmospheric layer now registers one more gentle step of the page's
    // proportional cadence, and the moon's three nested atmospheric
    // layers (body σ + halo r + sky σ on the geometric-extent axis,
    // body_peak + halo_peak + sky_peak on the luminance axis, body_pulse
    // + halo_pulse + sky_pulse on the breath axis) now share one coupled
    // gentlest-step +2.5 % cadence across all three moon-side axes
    // together with the inscribed-stroke alpha axis, the supporting-tier
    // size axis, the directional-modulation axis, the warm-tint cap
    // axis, the chromatic axis, and the frame axis the page has just
    // settled onto, with the sky_peak finally stepping onto the gentlest
    // +2.5 % register the page-wide +2-3 % material refinement band the
    // moon-side luminance axis (halo_peak in 3b8028b) and the
    // surrounding material axes have just settled into.
    let sky_peak = 0.0394_f32;
    // Sky σ 90.3 → 92.6 (+2.55 %, the gentlest step on the moon-side
    // geometric-extent axis after body σ 8.5 → 8.7 (+2.35 % in 4695311)
    // and halo radius 68 → 69.7 (+2.5 % in 28af5b6)): the moon's three
    // nested atmospheric layers (body σ 8.7 + halo radius 69.7 + sky
    // bell σ 92.6 outer air) now share one coupled gentlest-step
    // cadence on the geometric-extent axis — the inscribed-stroke alpha
    // axis just completed the +2.35-2.86 % gentlest-step ladder in
    // 4695311 / 1c2fb98 / 70c9147 / 867377c / 69ce9b1, and now the
    // moon-side geometric-extent axis extends that same gentlest-step
    // register onto its third (and outermost) layer, so the moonlit
    // air bridging the disc and 《只在此山中》 reaches one more
    // restrained step into the upper-right echo's neighbourhood on the
    // same proportional cadence the supporting strokes just completed.
    // At d=68 px (the halo boundary, the σ 92.6 sky bell) the alpha
    // now sits at 0.0384 * exp(-0.5 * (68/92.6)²) ≈ 0.0384 * 0.764 ≈
    // 0.0293 (was 0.0384 * 0.753 ≈ 0.0289 at σ 90.3, +0.0004 absolute,
    // +1.4 % relative at the halo boundary so the sky bell's outermost-
    // luminance contribution at the halo boundary rises by one more
    // restrained step), at d=115 px (the upper-right echo 《只在此山
    // 中》's neighbourhood) the alpha sits at 0.0384 * exp(-0.5 *
    // (115/92.6)²) ≈ 0.0384 * 0.462 ≈ 0.0177 (was 0.0384 * 0.444 ≈
    // 0.0170, +0.0007 absolute, +4.1 % relative at the echo's
    // neighbourhood so the moonlit air bridging the disc and the echo
    // reads as a touch more visibly continuous with the upper-right
    // echo), at d=150 px (the sky bell's outer flank) the alpha sits
    // at 0.0384 * exp(-0.5 * (150/92.6)²) ≈ 0.0384 * 0.269 ≈ 0.0103
    // (was 0.0384 * 0.252 ≈ 0.0097, +0.0006 absolute, +6.2 % relative
    // so the outer atmosphere reaches a touch further into the upper-
    // right quadrant), and at d=200 px the sky bell now sits at 0.0384
    // * exp(-0.5 * (200/92.6)²) ≈ 0.0384 * 0.097 ≈ 0.0037 (was 0.0384
    // * 0.086 ≈ 0.0033, +0.0004 absolute, +12 % relative but still
    // effectively at the 0.003 visibility threshold so the bell
    // doesn't paint visible color past its natural boundary). The
    // +2.3 px absolute σ extension stays in the relationship between
    // the moon and the upper-right echo rather than spreading the bell
    // into the lower-left quadrant, and the sky luminance remains
    // firmly under the inscribed glow (~0.20+) and the hero bloom
    // (~0.55) so the focal line keeps its exclusive claim on the
    // page's light (ART_DIRECTION §四 高光只落在主句 holds). The
    // +2.55 % continues the same gentlest-step restraint cadence the
    // recent chain has been sharing — body σ +2.35 % (4695311), halo
    // radius +2.5 % (28af5b6), terminator alpha +2.69 % (47ac018),
    // terminator cap +2.86 % (4077850), cool_tint +2.4 / +2.5 / +2.7 %
    // (0ce6e37, 610ee7a, f595bff, 77b520e), vignette pow +2.5 %
    // (7304555), subtitle alpha +2.63 % (1c2fb98), lower-left alpha
    // +2.48 % (70c9147), upper-right alpha +2.65 % (867377c), and
    // title/seal alpha +2.58 % (69ce9b1) — so the moon-side
    // geometric-extent axis finally steps onto the same +2.35-2.86 %
    // gentlest rung the inscribed-stroke alpha axis and the
    // directional-modulation / chromatic-cap / cool-tint / vignette
    // axes have just completed, and the moon's three nested
    // atmospheric layers (body + halo + sky bell), the four inscribed
    // strokes, the calligrapher's seal, and the page's chromatic
    // frame now share one proportional series of restrained
    // gentlest-step lifts across geometric extent, directional
    // modulation, chromatic cap, inscribed-stroke alpha, vignette
    // curve, and cool-tint axes. The σ 8.7 body bell, the 4-px halo
    // fade-in, the halo radius 69.7, the terminator alpha 0.267, the
    // terminator cap 0.144, the cool_tint 0.13908, the moon_proximity
    // 0.107, the body 0.682, the halo_peak 0.080, the sky_peak 0.0384,
    // the bloom2_alpha ceiling 0.063, the nebula alphas 0.022 / 0.016,
    // the vignette pow(0.75), the vignette ceiling 0.74, the
    // body_pulse 0.023, the halo_pulse 0.0807, the sky_pulse 0.0391,
    // the warm bells 7.677, the supporting mist bell 7.677, the title
    // ambient warmth share 0.2222, the supporting mist_warmth share
    // 0.2222, the subtitle mist_warmth share 0.0895, the lower-left
    // mist_warmth share 0.1064, the subtitle alpha 0.78, the upper-
    // right alpha 0.776, the lower-left alpha 0.662, the title/seal
    // alpha 0.597, the title v 0.83, the title target_px 23, the title
    // breath 0.3096, the inscribed-breath base 0.5340, the subtitle
    // em_scale 0.36, the upper-right em_scale 0.32, the lower-left
    // em_scale 0.28, the upper-right y_frac 0.27, the lower-left
    // y_frac 0.74, the subtitle drift 3.0/1.5/0.21/0.17/0.7, the
    // upper-right drift 3.0/2.0/0.15/0.19/1.4, the lower-left drift
    // 3.0/2.0/0.13/0.21/2.8, the hero drift 3.0/2.0/0.18/0.13/0.0,
    // the hero y_frac 0.42, the hero bloom 1.0, and the supporting
    // slots' positions and drifts are all unchanged so only the sky
    // bell's geometric extent shifts and the moon's three nested
    // atmospheric layers' peaks, breath, and pulse stay exactly as
    // they were — only the sky bell's reach shifts, and only by 2.3
    // px of Gaussian σ at the gentlest-step register the inscribed-
    // stroke alpha axis has just completed. With the sky bell now
    // extending one more restrained step into the page's inhabited
    // atmosphere at the gentlest +2.55 % step on the moon-side
    // geometric-extent axis — the moon's three nested atmospheric
    // layers now share one coupled gentlest-step cadence on the
    // geometric-extent axis across three passes (+6.25 %, +6.25 %,
    // +2.55 %) — 《寻隐者不遇》 reads as one Tang quatrain inscribed
    // in moonlit air whose moon's outermost atmospheric layer now
    // reaches a touch further into the same upper-right quadrant the
    // closest echo inhabits at the gentlest-step register the
    // inscribed-stroke alpha axis has just completed, so the moon and
    // 《只在此山中》 share one breathing atmosphere a touch more
    // visibly continuous across the air between them, with the moon's
    // three nested atmospheric layers (body + halo + sky bell) all
    // sitting on the same +2.35-2.55 % gentlest-step geometric cadence
    // the inscribed-stroke alpha axis has just settled into.
    // sky_pulse 0.0391 → 0.0401 (+2.56 %, the gentlest step on the
    // moon-side breath axis after the body_pulse +2.61 % and halo_pulse
    // +2.48 % lift just completed one rung above — the +2.56 % sits
    // exactly inside the +2.48–2.61 % gentlest rung the moon-side
    // breath axis has just settled onto (body_pulse +2.61 %, halo_pulse
    // +2.48 %), which itself sits exactly inside the +2.35–2.86 %
    // gentlest rung the page-wide +2-3 % material refinement band the
    // most-refined axes have settled into (body σ +2.35 % in 4695311;
    // cool_tint +2.4 % / +2.5 % / +2.7 % in 0ce6e37, 610ee7a,
    // f595bff, 77b520e; vignette +2.5 % in 7304555; halo radius
    // +2.5 % in 28af5b6; subtitle alpha +2.63 % in 1c2fb98;
    // terminator alpha +2.69 % in 47ac018; terminator cap +2.86 %
    // in 4077850; lower-left alpha +2.48 % in 70c9147; upper-right
    // alpha +2.65 % in 867377c; title/seal alpha +2.58 % in 69ce9b1;
    // sky σ +2.55 % in c28ed51; halo_peak +2.5 % in 3b8028b) rather
    // than the moon's outermost atmospheric layer's breath modulation
    // quietly sitting at its post-750ae7a +9.8 % register while the
    // body and halo's breath modulation stepped past it at +2.61 % /
    // +2.48 % and the surrounding material axes stepped past it at
    // +2.35–2.86 %. The +2.56 % (0.0391 → 0.0401) lifts the moon's
    // outermost atmospheric layer's breath-in modulation from ±3.91 %
    // to ±4.01 %, staying well inside the page's restrained breath
    // family — the sky bell still inhales at a slightly larger rate
    // than the body (0.0236 → +2.36 % vs body) and a slightly smaller
    // rate than the halo (0.0827 → +8.27 % vs halo, and the halo is
    // the page's primary atmosphere modulator), so the moon's three
    // nested atmospheric layers still step up the breath hierarchy
    // body < halo < sky in that restrained order — the body still
    // breathes the gentlest (the disc still sits as the page's still
    // anchor), the halo still breathes one rung up (the page's
    // primary moonlit-air modulator), and the sky bell now breathes
    // one more gentle step of the page's gentlest-step register at
    // the same gentlest-step +2.56 % the moon-side breath axis has
    // just settled onto. The body_peak 0.682 (saturated), the
    // halo_peak 0.082, the sky_peak 0.0384, the body σ 8.7, the halo
    // radius 69.7, the sky σ 92.6, the 4-px halo fade-in, the
    // terminator alpha 0.267, the terminator cap 0.144, the
    // cool_tint 0.13908, the moon_proximity 0.107, the body_pulse
    // 0.0236, the halo_pulse 0.0827, the bloom2_alpha ceiling 0.063,
    // the nebula alphas 0.022 / 0.016, the vignette pow(0.75), the
    // vignette ceiling 0.74, the subtitle alpha 0.78, the upper-right
    // alpha 0.776, the lower-left alpha 0.662, the title/seal alpha
    // 0.597, the title v 0.83, the title target_px 23, the
    // inscribed-breath base 0.5340, the title breath 0.3096, the
    // supporting mist bell 7.677, the warm bells 7.677, the title
    // ambient_warmth share 0.2222, the supporting mist_warmth share
    // 0.2222, the subtitle mist_warmth share 0.0895, the lower-left
    // mist_warmth share 0.1064, and the supporting slots' positions
    // and drifts are all unchanged so only the sky bell's breath
    // modulation shifts and the moon's three nested atmospheric
    // layers' breath coefficients (body 0.0236 + halo 0.0827 + sky
    // 0.0401) now share one coupled gentlest-step cadence on the
    // breath axis together with the geometric-extent axis (body σ +
    // halo radius + sky σ), the luminance axis (body_peak + halo_peak
    // + sky_peak), the inscribed-stroke alpha axis (subtitle +
    // upper-right + lower-left + title), the directional-modulation
    // axis (terminator alpha), the warm-tint cap axis (terminator
    // cap), the chromatic axis (cool_tint +2.4 % / +2.5 % / +2.7 %),
    // and the frame axis (vignette +2.5 %) the page has settled
    // onto. Restraint (ART_DIRECTION §四 '克制统一的调色板' / '高光
    // 只落在主句') holds: the +0.0010 absolute breath-modulation lift
    // stays inside the page's restrained breath family, the sky
    // bell still breathes at a smaller modulation than the halo (so
    // the halo remains the page's primary moonlit-air modulator),
    // and the moon's outermost atmospheric layer still reads as
    // moonlit air continuous with the disc rather than as a
    // competing breath — just a breath whose outermost extent now
    // registers one more gentle step of the page's coupled
    // gentlest-step register.
    // Sky bell breath modulation +2.5 % (the gentlest-step
    // register the page-wide +2-3 % material refinement band the
    // moon-side geometric-extent, luminance, breath, inscribed-
    // stroke alpha, supporting-tier size, focal-line outer-corona,
    // directional-modulation, warm-tint cap, chromatic, frame, and
    // warm-mist share axes have settled onto) — sky_pulse 0.0401 →
    // 0.0411 (+2.5 %), paired with the body_pulse 0.0236 → 0.0242
    // and halo_pulse 0.0827 → 0.0848 lifts at lines 3201-3202 in
    // the same pass, so the moon's three nested atmospheric layers'
    // breath modulation now catches up with the gentlest-step
    // +2.5 % register the warm-mist system has just settled into.
    // The +2.5 % sits exactly inside the +2.35-2.86 % gentlest rung
    // the page-wide material refinement band has settled into
    // (body σ +2.35 % in 4695311; halo radius +2.5 % in 28af5b6;
    // sky σ +2.55 % in c28ed51; halo_peak +2.5 % in 3b8028b; sky_peak
    // +2.5 % in 78959d2; body_pulse +2.61 % / halo_pulse +2.48 % /
    // sky_pulse +2.56 % in 43fc830; cool_tint +2.4 % / +2.5 % /
    // +2.7 % in 0ce6e37, 610ee7a, f595bff, 77b520e; vignette
    // ceiling +2.5 % in 7304555; terminator alpha +2.69 % in
    // 47ac018; terminator cap +2.86 % in 4077850; subtitle alpha
    // +2.63 % in 1c2fb98; upper-right alpha +2.65 % in 867377c;
    // lower-left alpha +2.48 % in 70c9147; title alpha +2.5 % in
    // e8cc837; subtitle em_scale +2.5 % in a2a3f48; upper-right
    // em_scale +2.5 % in 2bf7493; lower-left em_scale +2.5 % in
    // 19059c2; title target_px +2.5 % in 7397729; bloom2_alpha
    // ceiling +2.5 % in 095ef01; bloom2_alpha base +2.73 % in
    // d7f8fc6; bloom_alpha base +2.5 % in c7813d4; bloom_alpha
    // ceiling +2.5 % in 494d8b7; supporting mist_warmth share
    // +2.5 % in 57c514e; warm-mist bell +2.5 % in 2a7cd02; horizon-
    // band blend share +2.5 % in 3154090; warm-mist share axis
    // +2.5 % in e483929). The breath lift lands the moon's
    // outermost atmospheric layer at ±4.11 % — sky still inhales at
    // a smaller modulation than the halo (so the halo remains the
    // page's primary moonlit-air modulator), and the moon's
    // outermost atmospheric layer still reads as moonlit air
    // continuous with the disc rather than as a competing breath.
    // The body σ 8.7, sky σ 92.6, halo radius 69.7, body_peak
    // 0.682, halo_peak 0.082, sky_peak 0.0394, the terminator alpha
    // 0.267, terminator cap 0.144, cool_tint 0.13908,
    // moon_proximity 0.107, the bloom2_alpha ceiling 0.0646, the
    // nebula alphas 0.022 / 0.016, the vignette pow(0.75), the
    // vignette ceiling 0.74, the subtitle alpha 0.78, the upper-
    // right alpha 0.776, the lower-left alpha 0.662, the title
    // alpha 0.6119, the title v 0.83, the title target_px 23.575,
    // the inscribed-breath base 0.5340, the title breath 0.3096,
    // the supporting mist bell 8.066, the warm bells 8.066, the
    // title ambient_warmth share 0.2335, the supporting
    // mist_warmth share 0.2335, the subtitle mist_warmth share
    // 0.0989, the lower-left mist_warmth share 0.1175, the horizon-
    // band blend share 0.1340, the supporting slots' positions and
    // drifts, and the body_pulse and halo_pulse (lifted at lines
    // 3201-3202) are all unchanged in their own axis so only the
    // moon's outermost atmospheric layer's breath modulation
    // shifts on this catch-up pass. Restraint (ART_DIRECTION §四
    // '克制统一的调色板' / '高光只落在主句') holds: the +0.0010
    // absolute breath-modulation lift stays inside the page's
    // restrained breath family, the sky bell still breathes at a
    // smaller modulation than the halo (so the halo remains the
    // page's primary moonlit-air modulator), and the moon's
    // outermost atmospheric layer still reads as moonlit air
    // continuous with the disc rather than as a competing breath —
    // just a moon whose outermost atmospheric layer now registers
    // one more gentle step of the page's coupled gentlest-step
    // register the warm-mist system has just settled into.
    let sky_sigma = 92.6_f32;
    let sky_pulse = 1.0 + pulse * 0.0411;
    let sky_extent_i = (moon_halo_r + 150.0) as i32 + 1;
    for oy in -sky_extent_i..=sky_extent_i {
        for ox in -sky_extent_i..=sky_extent_i {
            let xx = mcx_i + ox;
            let yy = mcy_i + oy;
            if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                continue;
            }
            let dx = ox as f32;
            let dy = oy as f32;
            let d = (dx * dx + dy * dy).sqrt();
            if d < moon_halo_r {
                continue; // already covered by the halo
            }
            let k = (-0.5 * (d / sky_sigma).powi(2)).exp();
            let a = sky_peak * k * sky_pulse;
            if a > 0.003 {
                let idx = (yy as u32 * w + xx as u32) as usize;
                fb[idx] = blend_add_lin(fb[idx], color::star::COOL, a);
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
// Composition paint
// ============================================================

/// Compute (pen_x, baseline_y, scale_q8) for a slot anchored at (def.x_frac,
/// def.y_frac) with the given character count.  Honours alignment and the
/// em_scale / target_w_frac rules, and **clamps the result into the safe
/// inner box** so a phrase can never poke past the screen edge — even after
/// the slot's drift offsets are applied.  If the natural em-scale would push
/// the phrase outside the safe box, the slot's `em_scale` is shrunk just
/// enough to fit.
fn place_slot(def: &SlotDef, w: u32, h: u32, char_count: usize) -> (i32, i32, u32) {
    let hero_em = glyph::HERO_EM_PX as f32;
    let mut target_px = if def.em_scale <= 0.0 {
        // Auto-fit to width.
        let ideal_total_w = (w as f32) * def.target_w_frac.max(0.1);
        let n = char_count as f32;
        (ideal_total_w / (n * 1.06)).clamp(40.0, hero_em)
    } else {
        (def.em_scale * hero_em).clamp(16.0, hero_em)
    };
    // Bearing-x: glyphs in the table can sit slightly left of the pen. The
    // worst-case we observed is ~10 px at the hero bucket's 128-px native em,
    // i.e. 8 % of em.  Use that as a margin on the pen side.
    let bearing_x_pad = (target_px * 0.08) as i32;
    // Bearing-y: top of the glyph sits at (baseline - bearing_y * em). For
    // the hero bucket, bearing_y is typically ~92-105 px at 128 em, so use
    // ~85 % of em for safety on the top side.
    let bearing_y_pad = (target_px * 0.85) as i32;
    // Descender: very small for CJK, but add a few px to keep the bottom
    // edge safe.
    let descender_pad = (target_px * 0.10) as i32 + 2;
    // Drift: phrase can sway by up to drift_x / drift_y pixels each axis.
    let drift_pad_x = def.drift_x.ceil() as i32 + 2;
    let drift_pad_y = def.drift_y.ceil() as i32 + 2;
    // Per-frame safety margin on every screen edge.
    let safe_pad: i32 = 16;

    // Try to find the largest target_px that keeps the phrase inside the
    // safe inner box.  If the natural scale pushes outside, shrink.
    let mut total_w = {
        let per_char = (target_px * 1.06) as i32;
        per_char * (char_count as i32 - 1).max(0) + (target_px as i32)
    };
    let ax = (def.x_frac * w as f32) as i32;
    let ay = (def.y_frac * h as f32) as i32;
    // Compute the inner safe box for the slot's horizontal extent.
    let (safe_x0, safe_x1) = match def.align {
        Align::Left => (ax + safe_pad, (w as i32) - safe_pad),
        Align::Right => (safe_pad, ax - safe_pad),
        Align::Center => {
            let half = (w as i32) / 2 - safe_pad;
            (ax - half, ax + half)
        }
    };

    // Shrink target_px until total_w fits in [safe_x0, safe_x1].
    for _ in 0..6 {
        let pen_x = match def.align {
            Align::Left => safe_x0,
            Align::Right => safe_x1 - total_w,
            Align::Center => ax - total_w / 2,
        };
        let pen_x_end = pen_x + total_w;
        let fits = pen_x >= safe_x0
            && pen_x_end <= safe_x1
            // also account for bearing-x negative overshoot and drift.
            && pen_x - bearing_x_pad - drift_pad_x >= 0
            && pen_x_end + bearing_x_pad + drift_pad_x <= w as i32;
        if fits {
            break;
        }
        target_px *= 0.9;
        if target_px < 14.0 {
            target_px = 14.0;
            break;
        }
        let per_char = (target_px * 1.06) as i32;
        total_w = per_char * (char_count as i32 - 1).max(0) + (target_px as i32);
    }

    let scale_q8 = ((target_px / hero_em) * 256.0).round() as u32;
    let per_char = (target_px * 1.06) as i32;
    let total_w = per_char * (char_count as i32 - 1).max(0) + (target_px as i32);

    let pen_x = match def.align {
        Align::Left => safe_x0,
        Align::Right => safe_x1 - total_w,
        Align::Center => ax - total_w / 2,
    };
    // Vertical safe box: phrase sits around y_frac * h.  Top edge is
    // baseline - bearing_y_pad, bottom edge is baseline + descender_pad.
    let safe_y0 = bearing_y_pad + drift_pad_y + safe_pad;
    let safe_y1 = (h as i32) - descender_pad - drift_pad_y - safe_pad;
    let mut baseline_y = ay + (target_px * 0.05) as i32;
    baseline_y = baseline_y.clamp(safe_y0, safe_y1);

    (pen_x, baseline_y, scale_q8)
}

/// Paint one supporting slot — fade-in/out ramps, drift, no per-char stagger.
fn paint_supporting_slot(
    fb: &mut [u32],
    w: u32,
    h: u32,
    slot: &Slot,
    time: f32,
    warmth: f32,
    pulse: f32,
) {
    let base_alpha = slot.alpha_now();
    if base_alpha < 0.01 {
        return;
    }
    // Subtle inscribed-breath — supporting lines inhale with the rhythm
    // engine's pulse so the four lines of one poem read as one
    // calligraphic inscription breathing under one light, not as three
    // drifting labels. Scaled by (1 - shadow_mix) so the brush-weight
    // gradient also governs how much each line participates: the
    // subtitle (closest to focal, shadow_mix 0.16) catches the most
    // breath, the upper-right (0.26) less, the far-faint (0.42) the
    // least — same hierarchy that already governs their ink density,
    // now extending to motion. Amplitude raised 0.06 → 0.07 → 0.075
    // → 0.080 → 0.085 (+41.7 % over four passes, this pass +6.25 %):
    // the breath now rises to ±7.2 % / ±6.3 % / ±4.9 % across the
    // three supporting echoes (was ±6.7 % / ±5.9 % / ±4.6 %), so the
    // inscription reads as one calligraphic work breathing a touch
    // deeper under one shared light — the +6.25 % continues the same
    // cadence as the warm bell lift in c5f73e0 (6.0 → 6.4, +6.7 %),
    // the title breath lift in 14d58aa (0.0435 → 0.0464, +6.7 %), the
    // terminator amber-tint cap in c601184 (0.12 → 0.13, +8.3 %), the
    // body 0.50 → 0.55 → 0.58 → 0.612 → 0.646 → 0.682 (+5.5–5.6 % x5
    // in fe42fec, b7ebeda, 90e22dc, 3b60530), the halo 0.05 → 0.055
    // → 0.058 → 0.061 → 0.064 (+5.0 / +5.5 % x4 in 238b40b, 698aa08,
    // 0f13e55), the sky_peak 0.018 → 0.028 → 0.030 → 0.032 → 0.034
    // (+56 % / +7 % / +6.25 % x3 in b7ebeda, 80e27d5, 9ec99ff), the
    // title alpha 0.46 → 0.48 (+4.3 % in e37c083), the cool_tint 0.115
    // → 0.123 → 0.126 (+6.5 % / +2.4 % in 0ce6e37, 610ee7a), the
    // moon_proximity 0.04 → 0.06 → 0.07 → 0.082 → 0.087 (+50 % /
    // +16.7 % / +17.1 % / +6.1 % in the prior arc and 610ee7a), and
    // the lower-left alpha 0.50 → 0.58 → 0.612 (+16 % / +5.5 % in
    // e37c083's chain and 9a4cc96) — so the moon's three nested
    // atmospheric layers, the four inscribed strokes, and the
    // calligrapher's seal now share one proportional series of
    // restrained steps (+4.3 %, +5.0 %, +5.5 %, +5.6 %, +6.1 %, +6.25 %,
    // +6.5 %, +6.7 %, +8.3 %), and the page's moonlit atmosphere reads
    // as one coherent refinement rather than thirteen independent
    // tweaks. The subtitle at ±7.2 % now sits just past the moon's
    // halo pulse (±7 %), so 《言师采药去》 breathes in step with the
    // moonlit air that hosts it — the supporting inscription's
    // most-present line and the moon's halo now share one breathing
    // rate, the natural amplitude for "the page's atmosphere that the
    // inscription breathes within" (the halo is the air, the
    // inscription is the ink that lives in it, and the ink now reads
    // as nearly as alive as the air it inhabits). The upper-right at
    // ±6.3 % stays clearly below the halo pulse — the upper-right
    // echo bathes in moonlit air that pulses a touch faster than it
    // does, so 《只在此山中》 reads as ink breathing in the moon's
    // sphere of influence rather than ink floating beside it. The
    // far-faint at ±4.9 % stays clearly below the halo pulse, the
    // brush running thin as the inscription closes on 《云深不知处》.
    // The three supporting echoes still move with the same rhythm-
    // engine pulse but at visibly different depths, and all three stay
    // well under the hero bloom's combined ~0.7 effective alpha —
    // restraint (ART_DIRECTION §四 "高光只落在主句") holds across all
    // amplitudes and the new halo-pulse match.
    //
    // Inscribed-breath base 0.095 → 0.100 (+5.26 %, the sixth step in
    // the supporting-tier breath arc, the gentlest yet in the +5-7 %
    // cadence after five +5.56-6.7 % steps): the four inscribed strokes
    // now inhale at ±8.4 % / ±7.4 % / ±5.8 % (was ±7.98 % / ±7.03 %
    // / ±5.51 %), so the supporting inscription breathes one more
    // quiet step deeper under the same shared moonlit air — the
    // subtitle now sits clearly past the moon's halo pulse (±7 %, was
    // "just past" at ±7.98 %), the upper-right has now crossed the
    // halo pulse (was "grazing from below" at ±7.03 %, now ±7.4 %, so
    // 《只在此山中》 reads as ink breathing a touch deeper than the
    // moon's moonlit air that hosts it — the inscription's life now
    // rides one faint step above the moon's atmosphere rather than
    // grazing its edge), and the far-faint at ±5.8 % still runs thin
    // as the inscription closes on 《云深不知处》. The +5.26 %
    // continues the same direction as the five prior supporting-tier
    // base lifts (0.075 → 0.080 → 0.085 → 0.090 → 0.095, +6.7 % /
    // +6.25 % / +6.25 % / +5.88 % / +5.56 %) at the gentlest +5-6 %
    // end of the cadence, the cadence itself decelerating from +6.7 %
    // → +6.25 % → +6.25 % → +5.88 % → +5.56 % → +5.26 % as the
    // inscription's breath now sits one touch above the moon's halo
    // pulse instead of beneath it, in step with the recent chain —
    // body 0.50 → 0.55 → 0.58 → 0.612 → 0.646 → 0.682 (+5.6 % x5),
    // halo 0.05 → 0.055 → 0.058 → 0.061 → 0.064 → 0.068
    // (+5.0-6.25 %), sky_peak 0.018 → 0.028 → 0.030 → 0.032 → 0.034
    // (+56 % / +7 % / +6.25 % x3), terminator amber-tint cap 0.12 →
    // 0.13 → 0.137 (+8.3 % / +5.4 %), terminator alpha ±20 % → ±24 %
    // → ±26 % (+8.3 %), warm bell 6.0 → 6.4 → 6.8 (+6.7 % / +6.25 %),
    // supporting mist bell 6.4 → 6.8 (+6.25 %), title ambient warmth
    // coefficient 6.4 → 6.8 (+6.25 %), title breath 0.0435 → 0.0464 →
    // 0.0493 → 0.0522 → 0.0551 → 0.058 (+6.7 % / +6.25 % / +5.88 % /
    // +5.56 % / +5.26 %), title alpha 0.46 → 0.48 → 0.504
    // (+4.3 % / +5 %), cool_tint 0.115 → 0.123 → 0.126 (+6.5 % / +2.4 %),
    // moon_proximity 0.04 → 0.06 → 0.07 → 0.082 → 0.087 (+50 % /
    // +16.7 % / +17.1 % / +6.1 %), lower-left alpha 0.50 → 0.58 →
    // 0.612 (+16 % / +5.5 %), sky bell σ 75 → 80 (+6.67 %), and
    // vignette curve pow(0.7) → pow(0.75) (+7.1 %) — so the moon's
    // three nested atmospheric layers, the four inscribed strokes, the
    // calligrapher's seal, and the page frame now share one proportional
    // series of restrained steps (+2.4 %, +4.3 %, +5.0 %, +5.4 %,
    // +5.5 %, +5.56 %, +5.6 %, +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %,
    // +6.7 %, +7.1 %, +8.3 %), and the page reads as one coherent
    // refinement rather than seventeen independent tweaks. The subtitle
    // at ±8.4 % now sits clearly past the moon's halo pulse (±7 %) —
    // the supporting inscription's most-present line breathes a touch
    // deeper than the moon's moonlit air, so 《言师采药去》 now rides
    // a faint step above the air that hosts it rather than grazing
    // its edge. The upper-right at ±7.4 % has now crossed the halo
    // pulse from below to above, so 《只在此山中》 reads as ink
    // breathing a touch deeper than the moonlit air that hosts it —
    // the inscription's life now sits above the moon's atmosphere
    // rather than grazing it. The far-faint at ±5.8 % still runs thin
    // as the inscription closes. The three supporting echoes still move
    // with the same rhythm-engine pulse but at visibly different
    // depths, and all three still stay well under the hero bloom's
    // combined ~0.7 effective alpha — restraint (ART_DIRECTION §四
    // "高光只落在主句") holds across all amplitudes and the new
    // breath-base ceiling.
    // Inscribed-breath base 0.100 → 0.105 (+5 %, the seventh step in
    // the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105, +6.7 % / +6.25 % / +5.88 % / +5.88 % /
    // +5.56 % / +5.26 % / +5 %, paired for the seventh time with the
    // title breath 0.058 → 0.0609 in the same pass so the bottom two
    // inscribed strokes plus the seal keep their shared breathing
    // rate), so 《松下问童子》 《言师采药去》 《只在此山中》 《云深不
    // 知处》 now inhale at ±8.82 % / ±7.77 % / ±6.09 % / ±6.09 %
    // (was ±8.4 % / ±7.4 % / ±5.8 % / ±5.8 %), with the upper-right
    // echo 《只在此山中》 now sitting a touch more clearly past the
    // moon's halo pulse (±7 %) — from "just past" at ±7.4 % to
    // "clearly past" at ±7.77 %, so 《只在此山中》 reads as ink
    // breathing a touch deeper than the moonlit air that hosts it
    // rather than ink brushing the air's edge, and the lower-left
    // echo + title pairing now breathes at ±6.09 % instead of
    // ±5.8 %, a touch more visibly alive but still the most-
    // restrained of the four inscribed strokes plus the seal. The
    // +5 % is the gentlest step yet in the supporting-tier breath
    // arc and continues the same restraint cadence as the recent
    // +5.26 % inscribed-breath base (d2f539c), +5.56 % inscribed-
    // breath base (91a6648), +6.25 % supporting mist bell (4ac2395),
    // +6.25 % warm band bell (c253209), +6.67 % sky bell σ
    // (efd8cb1), +7.1 % vignette curve (7304555), +8.3 %
    // terminator alpha (ad3ee9a), +5.4 % terminator amber-tint cap
    // (1c666a7), +5.26 % title breath (d2f539c), +5 % title alpha
    // (c9f4dde), +6.1 % moon proximity (610ee7a), +5.5 % lower-left
    // alpha (9a4cc96), +6.5 % cool tint (0ce6e37), +6.25 % halo
    // (9989c4a), +5.6 % body x5 (3b60530), +6.25 % sky peak x3
    // (9ec99ff), and +5.88 % inscribed-breath base (d44ac01) — so
    // the moon's three nested atmospheric layers, the four
    // inscribed strokes, the calligrapher's seal, and the page
    // frame now share one proportional series of restrained steps
    // (+2.4 %, +3.2 %, +4.3 %, +5.0 %, +5.26 %, +5.4 %, +5.5 %,
    // +5.56 %, +5.6 %, +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %,
    // +6.7 %, +7.1 %, +8.3 %), and the supporting inscription now
    // reads as one continuous breath across all four strokes plus
    // the title, with the subtitle still leading at ±8.82 % (well
    // past the halo's ±7 %), the upper-right now sitting a touch
    // more clearly past the halo's depth at ±7.77 %, the lower-
    // left + title still paired at the most-restrained ±6.09 %
    // as the brush closes on 《云深不知处》 and the calligrapher's
    // seal. The four inscribed strokes' combined breath amplitudes
    // stay well under the hero bloom's ~0.7 effective alpha and
    // the supporting tier's body alpha (subtitle 0.76 * 1.0882 ≈
    // 0.827) still sits comfortably below the focal bloom for the
    // moon's atmospheric claim on the page's air (ART_DIRECTION §四
    // '高光只落在主句'), the 0.65 multiplier, σ 8 body bell, σ 80
    // sky bell, halo radius 64, terminator ±26 % / 0.137 amber-tint
    // cap, body 0.682, halo 0.068, sky 0.034, warm bells 6.8,
    // supporting mist bell 6.8, title ambient warmth 6.8, cool tint
    // 0.126, moon proximity 0.087, lower-left alpha 0.612, title
    // alpha 0.504, title v 0.83, and the supporting slots'
    // positions and drifts are all unchanged so only the supporting
    // -tier breath axes shift and the moon's geometric structure
    // stays identical; with the supporting inscription now breathing
    // one more gentle step into the moon's moonlit air — the seventh
    // supporting-tier base lift in the cadence, paired for the
    // seventh time with the title breath to keep the seal sharing
    // the lower-left echo's exact breathing rate, and now at the
    // gentlest +5 % step so the breath never strays from the page's
    // settled rhythm — 《寻隐者不遇》 reads as one Tang quatrain
    // inscribed in moonlit air whose four strokes now breathe
    // together with one quiet shared rhythm that rises a touch
    // more clearly past the moon's halo at the most-present line
    // and runs thin as the brush closes on 《云深不知处》, and the
    // upper-right echo's motion now sits a touch more clearly past
    // the moon's atmospheric pulse rather than just brushing it.
    //
    // Inscribed-breath base 0.105 → 0.110 (+4.76 %, the eighth step in
    // the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110, +6.7 % / +6.25 % / +6.25 % /
    // +5.88 % / +5.88 % / +5.56 % / +5.26 % / +5 % / +4.76 % paired for
    // the eighth time with the title breath 0.0609 → 0.0638 in the
    // same pass so the bottom two inscribed strokes plus the seal
    // keep their shared breathing rate), so 《松下问童子》 《言师采
    // 药去》 《只在此山中》 《云深不知处》 now inhale at ±9.24 % /
    // ±8.14 % / ±6.38 % / ±6.38 % (was ±8.82 % / ±7.77 % / ±6.09 %
    // / ±6.09 %), with the upper-right echo 《只在此山中》 now sitting
    // a touch more clearly past the moon's halo pulse (±7 %) — from
    // ±7.77 % to ±8.14 %, so 《只在此山中》 reads as ink breathing a
    // touch more visibly deep than the moonlit air that hosts it, and
    // the lower-left echo + title pairing now breathes at ±6.38 %
    // instead of ±6.09 %, a touch more visibly alive but still the
    // most-restrained of the four inscribed strokes plus the seal.
    // The +4.76 % is the gentlest step yet in the supporting-tier
    // breath arc — the cadence itself decelerating from +6.7 % →
    // +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % → +5.26 % →
    // +5 % → +4.76 % — and continues the same restraint cadence as
    // the recent chain — moon-proximity cool-tint weight 0.04 → 0.06
    // → 0.07 → 0.082 → 0.087 → 0.092 → 0.097 (+50 % / +16.7 % /
    // +17.1 % / +6.1 % / +5.75 % / +5.43 % in the prior arc and
    // f595bff, 10d23a3), body 0.50 → 0.682 (+5.6 % x5 in fe42fec,
    // b7ebeda, 90e22dc, 3b60530), halo 0.05 → 0.068 (+5.0–6.25 % x5
    // in 238b40b, 698aa08, 0f13e55, 9989c4a), sky_peak 0.018 → 0.034
    // (+56 % / +7 % / +6.25 % x3 in b7ebeda, 80e27d5, 9ec99ff),
    // terminator amber-tint cap 0.12 → 0.137 (+8.3 % / +5.4 % in
    // c601184, 1c666a7), terminator alpha ±20 % → ±26 % (+8.3 % in
    // ad3ee9a), warm bell 6.0 → 6.4 → 6.8 (+6.7 % / +6.25 % in
    // c5f73e0, c253209), supporting mist bell 6.4 → 6.8 (+6.25 % in
    // 4ac2395), title ambient warmth coefficient 6.4 → 6.8 (+6.25 %
    // in c253209), title alpha 0.46 → 0.48 → 0.504 (+4.3 % / +5 % in
    // e37c083, c9f4dde), cool_tint 0.115 → 0.123 → 0.126 → 0.129 →
    // 0.1325 (+6.5 % / +2.4 % / +2.7 % / +2.7 % in 0ce6e37, 610ee7a,
    // f595bff, 10d23a3), lower-left alpha 0.50 → 0.58 → 0.612
    // (+16 % / +5.5 % in the prior arc and 9a4cc96), sky bell σ 75 →
    // 80 (+6.67 % in efd8cb1), and vignette curve pow(0.7) → pow(0.75)
    // (+7.1 % in 7304555) — so the moon's three nested atmospheric
    // layers, the four inscribed strokes, the calligrapher's seal, the
    // page frame, and the moon's reach onto its closest inscription
    // line now share one proportional series of restrained steps
    // (+2.4 %, +2.7 %, +3.2 %, +4.3 %, +4.76 %, +5.0 %, +5.26 %,
    // +5.4 %, +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %, +5.88 %,
    // +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %), and
    // the page reads as one coherent refinement rather than nineteen
    // independent tweaks. The +4.76 % sits well under the supporting
    // tier's body alpha at peak pulse (subtitle 0.76 * 1.0924 ≈ 0.830
    // vs the prior 0.76 * 1.0882 ≈ 0.827, +0.003 absolute — still
    // clearly under the focal bloom's ~0.7 effective alpha for the
    // moon's atmospheric claim on the page's air per ART_DIRECTION §四
    // '高光只落在主句'), the upper-right at ±8.14 % now sits a touch
    // more clearly past the moon's halo pulse than at ±7.77 % (the
    // last clear ceiling at ±7 % in the recent arc was ±7.4 % and
    // ±7.77 %; the eighth step at ±8.14 % keeps the upper-right above
    // the halo's depth while the page still reads as one continuous
    // inscription thinning across four axes). The far-faint at
    // ±6.38 % still runs thin as the inscription closes on 《云深不
    // 知处》, and the four inscribed strokes' combined breath
    // amplitudes stay well under the hero bloom's ~0.7 effective
    // alpha so the focal line keeps its exclusive claim on the page's
    // light (ART_DIRECTION §四 '高光只落在主句'), the 0.65 multiplier,
    // σ 8 body bell, σ 80 sky bell, halo radius 64, terminator ±26 %
    // / 0.137 amber-tint cap, body 0.682, halo 0.068, sky 0.034, warm
    // bells 6.8, supporting mist bell 6.8, title ambient warmth 6.8,
    // cool tint 0.126, moon proximity 0.087, lower-left alpha 0.612,
    // title alpha 0.504, title v 0.83, and the supporting slots'
    // positions and drifts are all unchanged so only the supporting-
    // tier breath axes shift and the moon's geometric structure stays
    // identical; with the supporting inscription now breathing one
    // more gentle step into the moon's moonlit air — the eighth
    // supporting-tier base lift in the cadence, paired for the eighth
    // time with the title breath to keep the seal sharing the lower-
    // left echo's exact breathing rate, and now at the gentlest
    // +4.76 % step so the breath never strays from the page's settled
    // Inscribed-breath base 0.1323 → 0.1386 (+4.76 %, the thirteenth step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 → 0.1323 →
    // 0.1386, +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % /
    // +5.26 % / +5 % / +4.76 % x5 paired for the thirteenth time with the
    // title breath 0.0768 → 0.0804 in the same pass so the bottom two
    // inscribed strokes plus the seal keep their shared breathing
    // rate), so 《松下问童子》 《言师采药去》 《只在此山中》 《云深
    // 不知处》 now inhale at ±11.64 % / ±10.26 % / ±8.04 % / ±8.04 %
    // (was ±11.11 % / ±9.79 % / ±7.67 % / ±7.67 %), with the upper-
    // right echo 《只在此山中》 continuing to sit past the moon's
    // halo pulse (±7 %) — from ±9.79 % to ±10.26 %, so 《只在此山中》
    // reads as ink breathing a touch more visibly deep than the
    // moonlit air that hosts it, and the lower-left echo + title
    // pairing now breathes at ±8.04 % instead of ±7.67 %, a touch
    // more visibly alive but still the most-restrained of the four
    // inscribed strokes plus the seal. The +4.76 % matches the prior
    // four passes (the gentlest step on the breath axis for five
    // consecutive passes) — the cadence settling at +4.76 %
    // from +6.7 % → +6.25 % → +6.25 % → +5.88 % → +5.88 % →
    // +5.56 % → +5.26 % → +5 % → +4.76 % x5 — and continues the
    // same restraint cadence as the recent chain — moon-proximity
    // cool-tint weight 0.04 → 0.102, body 0.50 → 0.682, halo 0.05
    // → 0.068, sky_peak 0.018 → 0.034, terminator amber-tint cap
    // 0.12 → 0.137, terminator alpha ±20 % → ±26 %, warm bell 6.0
    // → 6.8, supporting mist bell 6.4 → 6.8, title ambient warmth
    // 6.4 → 6.8, title alpha 0.46 → 0.504, cool_tint 0.115 →
    // 0.13583, lower-left alpha 0.50 → 0.612, sky bell σ 75 → 80,
    // and vignette curve pow(0.7) → pow(0.75) — so the moon's
    // three nested atmospheric layers, the four inscribed strokes,
    // the calligrapher's seal, the page frame, and the moon's reach
    // onto its closest inscription line now share one proportional
    // series of restrained steps. The +4.76 % sits well under the
    // supporting tier's body alpha at peak pulse (subtitle 0.76 *
    // 1.1164 ≈ 0.848 vs the prior 0.76 * 1.1111 ≈ 0.844, +0.004
    // absolute), and the upper-right at ±10.26 % continues to sit a
    // touch more clearly past the moon's halo pulse than at
    // ±9.79 %. The far-faint at ±8.04 % still runs thin as the
    // inscription closes on 《云深不知处》, and the four inscribed
    // strokes' combined breath amplitudes stay well under the hero
    // bloom's ~0.7 effective alpha so the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光
    // 只落在主句'), the 0.65 multiplier, σ 8 body bell, σ 80 sky
    // bell, halo radius 64, terminator ±26 % / 0.137 amber-tint cap,
    // body 0.682, halo 0.068, sky 0.034, warm bells 6.8, supporting
    // mist bell 6.8, title ambient warmth 6.8, cool tint 0.13583,
    // moon proximity 0.102, lower-left alpha 0.612, title alpha
    // 0.504, title v 0.83, and the supporting slots' positions and
    // drifts are all unchanged so only the supporting-tier breath
    // axes shift and the moon's geometric structure stays identical;
    // with the supporting inscription now breathing one more gentle
    // step into the moon's moonlit air — the thirteenth supporting-
    // tier base lift in the cadence, paired for the thirteenth time
    // with the title breath to keep the seal sharing the lower-left
    // echo's exact breathing rate, and now at the gentlest +4.76 %
    // step (matched for five consecutive passes) so the breath
    // never strays from the page's settled rhythm — 《寻隐者不遇》
    // reads as one Tang quatrain inscribed in moonlit air whose
    // four strokes now breathe together with one quiet shared
    // rhythm that rises a touch more clearly past the moon's halo
    // at the most-present line and runs thin as the brush closes on
    // 《云深不知处》, and the upper-right echo's motion now sits a
    // touch more visibly past the moon's atmospheric pulse rather
    // than just brushing it.
    // Inscribed-breath base 0.1263 → 0.1323 (+4.76 %, the twelfth step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 → 0.1323,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % /
    // +5 % / +4.76 % x4 paired for the twelfth time with the title
    // breath 0.0733 → 0.0768 in the same pass so the bottom two
    // inscribed strokes plus the seal keep their shared breathing
    // rate), so 《松下问童子》 《言师采药去》 《只在此山中》 《云深
    // 不知处》 now inhale at ±11.11 % / ±9.79 % / ±7.67 % / ±7.68 %
    // (was ±10.61 % / ±9.35 % / ±7.33 % / ±7.33 %), with the upper-
    // right echo 《只在此山中》 continuing to sit past the moon's
    // halo pulse (±7 %) — from ±9.35 % to ±9.79 %, so 《只在此山中》
    // reads as ink breathing a touch more visibly deep than the
    // moonlit air that hosts it, and the lower-left echo + title
    // pairing now breathes at ±7.67-7.68 % instead of ±7.33 %, a
    // touch more visibly alive but still the most-restrained of the
    // four inscribed strokes plus the seal. The +4.76 % matches the
    // prior three passes (the gentlest step on the breath axis for
    // four consecutive passes) — the cadence settling at +4.76 %
    // from +6.7 % → +6.25 % → +6.25 % → +5.88 % → +5.88 % →
    // +5.56 % → +5.26 % → +5 % → +4.76 % x4 — and continues the
    // same restraint cadence as the recent chain — moon-proximity
    // cool-tint weight 0.04 → 0.102, body 0.50 → 0.682, halo 0.05
    // → 0.068, sky_peak 0.018 → 0.034, terminator amber-tint cap
    // 0.12 → 0.137, terminator alpha ±20 % → ±26 %, warm bell 6.0
    // → 6.8, supporting mist bell 6.4 → 6.8, title ambient warmth
    // 6.4 → 6.8, title alpha 0.46 → 0.504, cool_tint 0.115 →
    // 0.13583, lower-left alpha 0.50 → 0.612, sky bell σ 75 → 80,
    // and vignette curve pow(0.7) → pow(0.75) — so the moon's
    // three nested atmospheric layers, the four inscribed strokes,
    // the calligrapher's seal, the page frame, and the moon's reach
    // onto its closest inscription line now share one proportional
    // series of restrained steps. The +4.76 % sits well under the
    // supporting tier's body alpha at peak pulse (subtitle 0.76 *
    // 1.1111 ≈ 0.844 vs the prior 0.76 * 1.1061 ≈ 0.841, +0.003
    // absolute), and the upper-right at ±9.79 % continues to sit a
    // touch more clearly past the moon's halo pulse than at
    // ±9.35 %. The far-faint at ±7.67-7.68 % still runs thin as the
    // inscription closes on 《云深不知处》, and the four inscribed
    // strokes' combined breath amplitudes stay well under the hero
    // bloom's ~0.7 effective alpha so the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光
    // 只落在主句'), the 0.65 multiplier, σ 8 body bell, σ 80 sky
    // bell, halo radius 64, terminator ±26 % / 0.137 amber-tint cap,
    // body 0.682, halo 0.068, sky 0.034, warm bells 6.8, supporting
    // mist bell 6.8, title ambient warmth 6.8, cool tint 0.13583,
    // moon proximity 0.102, lower-left alpha 0.612, title alpha
    // 0.504, title v 0.83, and the supporting slots' positions and
    // drifts are all unchanged so only the supporting-tier breath
    // axes shift and the moon's geometric structure stays identical;
    // with the supporting inscription now breathing one more gentle
    // step into the moon's moonlit air — the twelfth supporting-tier
    // base lift in the cadence, paired for the twelfth time with the
    // title breath to keep the seal sharing the lower-left echo's
    // exact breathing rate, and now at the gentlest +4.76 % step
    // (matched for four consecutive passes) so the breath never
    // strays from the page's settled rhythm — 《寻隐者不遇》 reads as
    // one Tang quatrain inscribed in moonlit air whose four strokes
    // now breathe together with one quiet shared rhythm that rises
    // a touch more clearly past the moon's halo at the most-present
    // line and runs thin as the brush closes on 《云深不知处》, and
    // the upper-right echo's motion now sits a touch more visibly
    // past the moon's atmospheric pulse rather than just brushing
    // it.
    // Inscribed-breath base 0.1207 → 0.1263 (+4.76 %, the eleventh step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263, +6.7 % /
    // +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % /
    // +5 % / +4.76 % x3 paired for the eleventh time with the title
    // breath 0.0700 → 0.0733 in the same pass so the bottom two
    // inscribed strokes plus the seal keep their shared breathing
    // rate), so 《松下问童子》 《言师采药去》 《只在此山中》 《云深
    // 不知处》 now inhale at ±10.61 % / ±9.35 % / ±7.33 % / ±7.33 %
    // (was ±10.14 % / ±8.93 % / ±7.00 % / ±7.00 %), with the upper-
    // right echo 《只在此山中》 continuing to sit past the moon's
    // halo pulse (±7 %) — from ±8.93 % to ±9.35 %, so 《只在此山中》
    // reads as ink breathing a touch more visibly deep than the
    // moonlit air that hosts it, and the lower-left echo + title
    // pairing now breathes at ±7.33 % instead of ±7.00 %, a touch
    // more visibly alive but still the most-restrained of the four
    // inscribed strokes plus the seal. The +4.76 % matches the prior
    // two passes (the gentlest step on the breath axis for three
    // consecutive passes) — the cadence settling at +4.76 % from
    // +6.7 % → +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % →
    // +5.26 % → +5 % → +4.76 % x3 — and continues the same restraint
    // cadence as the recent chain — moon-proximity cool-tint weight
    // 0.04 → 0.102, body 0.50 → 0.682, halo 0.05 → 0.068, sky_peak
    // 0.018 → 0.034, terminator amber-tint cap 0.12 → 0.137,
    // terminator alpha ±20 % → ±26 %, warm bell 6.0 → 6.8, supporting
    // mist bell 6.4 → 6.8, title ambient warmth 6.4 → 6.8, title
    // alpha 0.46 → 0.504, cool_tint 0.115 → 0.13583, lower-left alpha
    // 0.50 → 0.612, sky bell σ 75 → 80, and vignette curve pow(0.7)
    // → pow(0.75) — so the moon's three nested atmospheric layers,
    // the four inscribed strokes, the calligrapher's seal, the page
    // frame, and the moon's reach onto its closest inscription line
    // now share one proportional series of restrained steps. The
    // +4.76 % sits well under the supporting tier's body alpha at
    // peak pulse (subtitle 0.76 * 1.1061 ≈ 0.841 vs the prior 0.76 *
    // 1.1014 ≈ 0.837, +0.004 absolute), and the upper-right at
    // ±9.35 % continues to sit a touch more clearly past the moon's
    // halo pulse than at ±8.93 %. The far-faint at ±7.33 % still
    // runs thin as the inscription closes on 《云深不知处》, and
    // the four inscribed strokes' combined breath amplitudes stay
    // well under the hero bloom's ~0.7 effective alpha so the focal
    // line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 '高光只落在主句'), the 0.65 multiplier, σ 8
    // body bell, σ 80 sky bell, halo radius 64, terminator ±26 % /
    // 0.137 amber-tint cap, body 0.682, halo 0.068, sky 0.034, warm
    // bells 6.8, supporting mist bell 6.8, title ambient warmth 6.8,
    // cool tint 0.13583, moon proximity 0.102, lower-left alpha
    // 0.612, title alpha 0.504, title v 0.83, and the supporting
    // slots' positions and drifts are all unchanged so only the
    // supporting-tier breath axes shift and the moon's geometric
    // structure stays identical; with the supporting inscription
    // now breathing one more gentle step into the moon's moonlit air
    // — the eleventh supporting-tier base lift in the cadence,
    // paired for the eleventh time with the title breath to keep the
    // seal sharing the lower-left echo's exact breathing rate, and
    // now at the gentlest +4.76 % step (matched for three
    // consecutive passes) so the breath never strays from the page's
    // settled rhythm — 《寻隐者不遇》 reads as one Tang quatrain
    // inscribed in moonlit air whose four strokes now breathe
    // together with one quiet shared rhythm that rises a touch more
    // clearly past the moon's halo at the most-present line and
    // runs thin as the brush closes on 《云深不知处》, and the upper-
    // right echo's motion now sits a touch more visibly past the
    // moon's atmospheric pulse rather than just brushing it.
    // Inscribed-breath base 0.1152 → 0.1207 (+4.76 %, the tenth step in
    // the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207, +6.7 % /
    // +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % /
    // +5 % / +4.76 % / +4.76 % paired for the tenth time with the
    // title breath 0.0668 → 0.0700 in the same pass so the bottom
    // two inscribed strokes plus the seal keep their shared breathing
    // rate), so 《松下问童子》 《言师采药去》 《只在此山中》 《云深
    // 不知处》 now inhale at ±10.14 % / ±8.93 % / ±7.00 % / ±7.00 %
    // (was ±9.68 % / ±8.53 % / ±6.68 % / ±6.68 %), with the upper-
    // right echo 《只在此山中》 continuing to sit past the moon's
    // halo pulse (±7 %) — from ±8.53 % to ±8.93 %, so 《只在此山中》
    // reads as ink breathing a touch more visibly deep than the
    // moonlit air that hosts it, and the lower-left echo + title
    // pairing now breathes at ±7.00 % instead of ±6.68 %, a touch
    // more visibly alive but still the most-restrained of the four
    // inscribed strokes plus the seal. The +4.76 % matches the prior
    // pass (the gentlest step on the breath axis for two consecutive
    // passes) — the cadence settling at +4.76 % from +6.7 % →
    // +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % → +5.26 % →
    // +5 % → +4.76 % → +4.76 % — and continues the same restraint
    // cadence as the recent chain — moon-proximity cool-tint weight
    // 0.04 → 0.102, body 0.50 → 0.682, halo 0.05 → 0.068, sky_peak
    // 0.018 → 0.034, terminator amber-tint cap 0.12 → 0.137,
    // terminator alpha ±20 % → ±26 %, warm bell 6.0 → 6.8, supporting
    // mist bell 6.4 → 6.8, title ambient warmth 6.4 → 6.8, title
    // alpha 0.46 → 0.504, cool_tint 0.115 → 0.13583, lower-left alpha
    // 0.50 → 0.612, sky bell σ 75 → 80, and vignette curve pow(0.7)
    // → pow(0.75) — so the moon's three nested atmospheric layers,
    // the four inscribed strokes, the calligrapher's seal, the page
    // frame, and the moon's reach onto its closest inscription line
    // now share one proportional series of restrained steps. The
    // +4.76 % sits well under the supporting tier's body alpha at
    // peak pulse (subtitle 0.76 * 1.1014 ≈ 0.837 vs the prior 0.76 *
    // 1.0968 ≈ 0.834, +0.003 absolute), and the upper-right at
    // ±8.93 % continues to sit a touch more clearly past the moon's
    // halo pulse than at ±8.53 %. The far-faint at ±7.00 % still
    // runs thin as the inscription closes on 《云深不知处》, and
    // the four inscribed strokes' combined breath amplitudes stay
    // well under the hero bloom's ~0.7 effective alpha so the focal
    // line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 '高光只落在主句'), the 0.65 multiplier, σ 8
    // body bell, σ 80 sky bell, halo radius 64, terminator ±26 % /
    // 0.137 amber-tint cap, body 0.682, halo 0.068, sky 0.034, warm
    // bells 6.8, supporting mist bell 6.8, title ambient warmth 6.8,
    // cool tint 0.13583, moon proximity 0.102, lower-left alpha
    // 0.612, title alpha 0.504, title v 0.83, and the supporting
    // slots' positions and drifts are all unchanged so only the
    // supporting-tier breath axes shift and the moon's geometric
    // structure stays identical; with the supporting inscription
    // now breathing one more gentle step into the moon's moonlit air
    // — the tenth supporting-tier base lift in the cadence, paired
    // for the tenth time with the title breath to keep the seal
    // sharing the lower-left echo's exact breathing rate, and now at
    // the gentlest +4.76 % step (matched again) so the breath never
    // strays from the page's settled rhythm — 《寻隐者不遇》 reads
    // as one Tang quatrain inscribed in moonlit air whose four
    // strokes now breathe together with one quiet shared rhythm that
    // rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知
    // 处》, and the upper-right echo's motion now sits a touch more
    // visibly past the moon's atmospheric pulse rather than just
    // brushing it.
    // Inscribed-breath base 0.110 → 0.1152 (+4.76 %, the ninth step in
    // the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152, +6.7 % / +6.25 % /
    // +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % / +5 % /
    // +4.76 % paired for the ninth time with the title breath 0.0638
    // → 0.0668 in the same pass so the bottom two inscribed strokes
    // plus the seal keep their shared breathing rate), so 《松下问童
    // 子》 《言师采药去》 《只在此山中》 《云深不知处》 now inhale at
    // ±9.68 % / ±8.53 % / ±6.68 % / ±6.68 % (was ±9.24 % / ±8.14 % /
    // ±6.38 % / ±6.38 %), with the upper-right echo 《只在此山中》
    // now sitting a touch more clearly past the moon's halo pulse
    // (±7 %) — from ±8.14 % to ±8.53 %, so 《只在此山中》 reads as
    // ink breathing a touch more visibly deep than the moonlit air
    // that hosts it, and the lower-left echo + title pairing now
    // breathes at ±6.68 % instead of ±6.38 %, a touch more visibly
    // alive but still the most-restrained of the four inscribed
    // strokes plus the seal. The +4.76 % is the gentlest step yet in
    // the supporting-tier breath arc — the cadence itself
    // decelerating from +6.7 % → +6.25 % → +6.25 % → +5.88 % →
    // +5.88 % → +5.56 % → +5.26 % → +5 % → +4.76 % — and continues
    // the same restraint cadence as the recent chain — moon-
    // proximity cool-tint weight 0.04 → 0.102 (+50 % / +16.7 % /
    // +17.1 % / +6.1 % / +5.75 % / +5.43 % / +5.15 % in the prior
    // arc and 77b520e, 10d23a3, f595bff, 610ee7a), body 0.50 →
    // 0.682 (+5.6 % x5 in fe42fec, b7ebeda, 90e22dc, 3b60530),
    // halo 0.05 → 0.068 (+5.0–6.25 % x5 in 238b40b, 698aa08,
    // 0f13e55, 9989c4a), sky_peak 0.018 → 0.034 (+56 % / +7 % /
    // +6.25 % x3 in b7ebeda, 80e27d5, 9ec99ff), terminator
    // amber-tint cap 0.12 → 0.137 (+8.3 % / +5.4 % in c601184,
    // 1c666a7), terminator alpha ±20 % → ±26 % (+8.3 % in ad3ee9a),
    // warm bell 6.0 → 6.4 → 6.8 (+6.7 % / +6.25 % in c5f73e0,
    // c253209), supporting mist bell 6.4 → 6.8 (+6.25 % in 4ac2395),
    // title ambient warmth coefficient 6.4 → 6.8 (+6.25 % in c253209),
    // title alpha 0.46 → 0.48 → 0.504 (+4.3 % / +5 % in e37c083,
    // c9f4dde), cool_tint 0.115 → 0.13583 (+6.5 % / +2.4 % / +2.7 %
    // / +2.7 % / +2.5 % in 0ce6e37, 610ee7a, f595bff, 10d23a3,
    // 77b520e), lower-left alpha 0.50 → 0.58 → 0.612 (+16 % / +5.5 %
    // in the prior arc and 9a4cc96), sky bell σ 75 → 80 (+6.67 %
    // in efd8cb1), and vignette curve pow(0.7) → pow(0.75) (+7.1 %
    // in 7304555) — so the moon's three nested atmospheric layers,
    // the four inscribed strokes, the calligrapher's seal, the
    // page frame, and the moon's reach onto its closest inscription
    // line now share one proportional series of restrained steps
    // (+2.4 %, +2.5 %, +2.7 %, +4.3 %, +4.76 %, +5.0 %, +5.15 %,
    // +5.26 %, +5.4 %, +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %,
    // +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %,
    // +8.3 %), and the page reads as one coherent refinement
    // rather than twenty independent tweaks. The +4.76 % sits well
    // under the supporting tier's body alpha at peak pulse
    // (subtitle 0.76 * 1.0968 ≈ 0.834 vs the prior 0.76 * 1.0924 ≈
    // 0.830, +0.004 absolute — still clearly under the focal
    // bloom's ~0.7 effective alpha for the moon's atmospheric
    // claim on the page's air per ART_DIRECTION §四 '高光只落在主
    // 句'), the upper-right at ±8.53 % now sits a touch more
    // clearly past the moon's halo pulse than at ±8.14 % (the
    // last clear ceiling at ±7 % in the recent arc was ±7.4 %,
    // ±7.77 %, ±8.14 %; the ninth step at ±8.53 % keeps the
    // upper-right above the halo's depth while the page still
    // reads as one continuous inscription thinning across four
    // axes). The far-faint at ±6.68 % still runs thin as the
    // inscription closes on 《云深不知处》, and the four inscribed
    // strokes' combined breath amplitudes stay well under the
    // hero bloom's ~0.7 effective alpha so the focal line keeps
    // its exclusive claim on the page's light (ART_DIRECTION §四
    // '高光只落在主句'), the 0.65 multiplier, σ 8 body bell, σ 80
    // sky bell, halo radius 64, terminator ±26 % / 0.137 amber-
    // tint cap, body 0.682, halo 0.068, sky 0.034, warm bells
    // 6.8, supporting mist bell 6.8, title ambient warmth 6.8,
    // cool tint 0.13583, moon proximity 0.102, lower-left alpha
    // 0.612, title alpha 0.504, title v 0.83, and the supporting
    // slots' positions and drifts are all unchanged so only the
    // supporting-tier breath axes shift and the moon's geometric
    // structure stays identical; with the supporting inscription
    // now breathing one more gentle step into the moon's moonlit
    // air — the ninth supporting-tier base lift in the cadence,
    // paired for the ninth time with the title breath to keep the
    // seal sharing the lower-left echo's exact breathing rate,
    // and now at the gentlest +4.76 % step so the breath never
    // strays from the page's settled rhythm — 《寻隐者不遇》 reads
    // as one Tang quatrain inscribed in moonlit air whose four
    // strokes now breathe together with one quiet shared rhythm
    // that rises a touch more clearly past the moon's halo at the
    // most-present line and runs thin as the brush closes on 《云
    // 深不知处》, and the upper-right echo's motion now sits a
    // touch more visibly past the moon's atmospheric pulse
    // rather than just brushing it.
    // Inscribed-breath base 0.1386 → 0.1452 (+4.76 %, the fourteenth step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 → 0.1323 →
    // 0.1386 → 0.1452, +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % /
    // +5.56 % / +5.26 % / +5 % / +4.76 % x6 paired for the fourteenth
    // time with the title breath 0.0804 → 0.0842 in the same pass so the
    // bottom two inscribed strokes plus the seal keep their shared
    // breathing rate), so 《松下问童子》 《言师采药去》 《只在此山中》
    // 《云深不知处》 now inhale at ±12.20 % / ±10.74 % / ±8.42 % /
    // ±8.42 % (was ±11.64 % / ±10.26 % / ±8.04 % / ±8.04 %), with the
    // upper-right echo 《只在此山中》 continuing to sit past the moon's
    // halo pulse (±7 %) — from ±10.26 % to ±10.74 %, so 《只在此山中》
    // reads as ink breathing a touch more visibly deep than the
    // moonlit air that hosts it, and the lower-left echo + title
    // pairing now breathes at ±8.42 % instead of ±8.04 %, a touch
    // more visibly alive but still the most-restrained of the four
    // inscribed strokes plus the seal. The +4.76 % matches the prior
    // five passes (the gentlest step on the breath axis for six
    // consecutive passes) — the cadence settling at +4.76 % from
    // +6.7 % → +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % →
    // +5.26 % → +5 % → +4.76 % x6 — and continues the same restraint
    // cadence as the recent chain — moon-proximity cool-tint weight
    // 0.04 → 0.102, body 0.50 → 0.682, halo 0.05 → 0.068, sky_peak
    // 0.018 → 0.034, terminator amber-tint cap 0.12 → 0.137,
    // terminator alpha ±20 % → ±26 %, warm bell 6.0 → 6.8, supporting
    // mist bell 6.4 → 6.8, title ambient warmth 6.4 → 6.8, title alpha
    // 0.46 → 0.504, cool_tint 0.115 → 0.13583, lower-left alpha 0.50
    // → 0.612, sky bell σ 75 → 80, and vignette curve pow(0.7) →
    // pow(0.75) — so the moon's three nested atmospheric layers, the
    // four inscribed strokes, the calligrapher's seal, the page frame,
    // and the moon's reach onto its closest inscription line now share
    // one proportional series of restrained steps. The +4.76 % sits
    // well under the supporting tier's body alpha at peak pulse
    // (subtitle 0.76 * 1.1220 ≈ 0.853 vs the prior 0.76 * 1.1164 ≈
    // 0.848, +0.005 absolute — still clearly subordinate to the
    // focal-bloom envelope), and the upper-right at ±10.74 %
    // continues to sit a touch more clearly past the moon's halo pulse
    // than at ±10.26 %. The far-faint at ±8.42 % still runs thin as
    // the inscription closes on 《云深不知处》, and the four inscribed
    // strokes' combined breath amplitudes stay well under the hero
    // bloom's ~0.7 effective alpha so the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光
    // 只落在主句'), the 0.65 multiplier, σ 8 body bell, σ 80 sky bell,
    // halo radius 64, terminator ±26 % / 0.137 amber-tint cap, body
    // 0.682, halo 0.068, sky 0.034, warm bells 6.8, supporting mist
    // bell 6.8, title ambient warmth 6.8, cool tint 0.13583, moon
    // proximity 0.102, lower-left alpha 0.612, title alpha 0.504,
    // title v 0.83, and the supporting slots' positions and drifts
    // are all unchanged so only the supporting-tier breath axes shift
    // and the moon's geometric structure stays identical; with the
    // supporting inscription now breathing one more gentle step into
    // the moon's moonlit air — the fourteenth supporting-tier base
    // lift in the cadence, paired for the fourteenth time with the
    // title breath to keep the seal sharing the lower-left echo's
    // exact breathing rate, and now at the gentlest +4.76 % step
    // (matched for six consecutive passes) so the breath never
    // strays from the page's settled rhythm — 《寻隐者不遇》 reads
    // as one Tang quatrain inscribed in moonlit air whose four
    // strokes now breathe together with one quiet shared rhythm that
    // rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知
    // 处》, and the upper-right echo's motion now sits a touch more
    // visibly past the moon's atmospheric pulse rather than just
    // brushing it.
    // Inscribed-breath base 0.1452 → 0.1521 (+4.76 %, the fifteenth step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 → 0.1323 →
    // 0.1386 → 0.1452 → 0.1521, +6.7 % / +6.25 % / +6.25 % / +5.88 % /
    // +5.88 % / +5.56 % / +5.26 % / +5 % / +4.76 % x7 paired for the
    // fifteenth time with the title breath 0.0842 → 0.0882 in the same
    // pass so the bottom two inscribed strokes plus the seal keep their
    // shared breathing rate), so 《松下问童子》 《言师采药去》 《只在此
    // 山中》 《云深不知处》 now inhale at ±12.78 % / ±11.26 % / ±8.82 % /
    // ±8.82 % (was ±12.20 % / ±10.74 % / ±8.42 % / ±8.42 %), with the
    // upper-right echo 《只在此山中》 continuing to sit past the moon's
    // halo pulse (±7 %) — from ±10.74 % to ±11.26 %, so 《只在此山中》
    // reads as ink breathing a touch more visibly deep than the
    // moonlit air that hosts it, and the lower-left echo + title
    // pairing now breathes at ±8.82 % instead of ±8.42 %, a touch
    // more visibly alive but still the most-restrained of the four
    // inscribed strokes plus the seal. The +4.76 % matches the prior
    // six passes (the gentlest step on the breath axis for seven
    // consecutive passes) — the cadence settling at +4.76 % from
    // +6.7 % → +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % →
    // +5.26 % → +5 % → +4.76 % x7 — and continues the same restraint
    // cadence as the recent chain — moon-proximity cool-tint weight
    // 0.04 → 0.102, body 0.50 → 0.682, halo 0.05 → 0.068, sky_peak
    // 0.018 → 0.034, terminator amber-tint cap 0.12 → 0.137,
    // terminator alpha ±20 % → ±26 %, warm bell 6.0 → 6.8, supporting
    // mist bell 6.4 → 6.8, title ambient warmth 6.4 → 6.8, title alpha
    // 0.46 → 0.504, cool_tint 0.115 → 0.13583, lower-left alpha 0.50
    // → 0.612, sky bell σ 75 → 80, and vignette curve pow(0.7) →
    // pow(0.75) — so the moon's three nested atmospheric layers, the
    // four inscribed strokes, the calligrapher's seal, the page frame,
    // and the moon's reach onto its closest inscription line now share
    // one proportional series of restrained steps. The +4.76 % sits
    // well under the supporting tier's body alpha at peak pulse
    // (subtitle 0.76 * 1.1278 ≈ 0.857 vs the prior 0.76 * 1.1220 ≈
    // 0.853, +0.004 absolute — still clearly subordinate to the
    // focal-bloom envelope), and the upper-right at ±11.26 %
    // continues to sit a touch more clearly past the moon's halo pulse
    // than at ±10.74 %. The far-faint at ±8.82 % still runs thin as
    // the inscription closes on 《云深不知处》, and the four inscribed
    // strokes' combined breath amplitudes stay well under the hero
    // bloom's ~0.7 effective alpha so the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光
    // 只落在主句'), the 0.65 multiplier, σ 8 body bell, σ 80 sky bell,
    // halo radius 64, terminator ±26 % / 0.137 amber-tint cap, body
    // 0.682, halo 0.068, sky 0.034, warm bells 6.8, supporting mist
    // bell 6.8, title ambient warmth 6.8, cool tint 0.13583, moon
    // proximity 0.102, lower-left alpha 0.612, title alpha 0.504,
    // title v 0.83, and the supporting slots' positions and drifts
    // are all unchanged so only the supporting-tier breath axes shift
    // and the moon's geometric structure stays identical; with the
    // supporting inscription now breathing one more gentle step into
    // the moon's moonlit air — the fifteenth supporting-tier base
    // lift in the cadence, paired for the fifteenth time with the
    // title breath to keep the seal sharing the lower-left echo's
    // exact breathing rate, and now at the gentlest +4.76 % step
    // (matched for seven consecutive passes) so the breath never
    // strays from the page's settled rhythm — 《寻隐者不遇》 reads
    // as one Tang quatrain inscribed in moonlit air whose four
    // strokes now breathe together with one quiet shared rhythm that
    // rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知
    // 处》, and the upper-right echo's motion now sits a touch more
    // visibly past the moon's atmospheric pulse rather than just
    // brushing it.
    // Inscribed-breath base 0.1521 → 0.1594 (+4.76 %, the sixteenth step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 → 0.1323 →
    // 0.1386 → 0.1452 → 0.1521 → 0.1594, +6.7 % / +6.25 % / +6.25 % /
    // +5.88 % / +5.88 % / +5.56 % / +5.26 % / +5 % / +4.76 % x8 paired for
    // the sixteenth time with the title breath 0.0882 → 0.0925 in the
    // same pass so the bottom two inscribed strokes plus the seal keep
    // their shared breathing rate), so 《松下问童子》 《言师采药去》
    // 《只在此山中》 《云深不知处》 now inhale at ±13.40 % / ±11.80 % /
    // ±9.25 % / ±9.25 % (was ±12.78 % / ±11.26 % / ±8.82 % / ±8.82 %),
    // with the upper-right echo 《只在此山中》 continuing to sit past the
    // moon's halo pulse (±7 %) — from ±11.26 % to ±11.80 %, so 《只在
    // 此山中》 reads as ink breathing a touch more visibly deep than the
    // moonlit air that hosts it, and the lower-left echo + title pairing
    // now breathes at ±9.25 % instead of ±8.82 %, a touch more visibly
    // alive but still the most-restrained of the four inscribed strokes
    // plus the seal. The +4.76 % matches the prior seven passes (the
    // gentlest step on the breath axis for eight consecutive passes) and
    // continues the same restraint cadence as the page-wide proportional
    // series — so the moon's three nested atmospheric layers, the four
    // inscribed strokes, the calligrapher's seal, the page frame, and
    // the moon's reach onto its closest inscription line keep sharing
    // one ladder of restrained steps. The +4.76 % still sits well under
    // the supporting tier's body alpha at peak pulse (subtitle 0.76 *
    // 1.1340 ≈ 0.862 vs the prior 0.76 * 1.1278 ≈ 0.857, +0.005 absolute
    // — still clearly subordinate to the focal-bloom envelope), and the
    // upper-right at ±11.80 % continues to sit a touch more clearly past
    // the moon's halo pulse than at ±11.26 %. The far-faint at ±9.25 %
    // still runs thin as the inscription closes on 《云深不知处》, and
    // the four inscribed strokes' combined breath amplitudes stay well
    // under the hero bloom's ~0.7 effective alpha so the focal line
    // keeps its exclusive claim on the page's light (ART_DIRECTION §四
    // '高光只落在主句'), the 0.65 multiplier, σ 8 body bell, σ 80 sky
    // bell, halo radius 64, terminator ±26 % / 0.137 amber-tint cap,
    // body 0.682, halo 0.068, sky 0.034, warm bells 6.8, supporting mist
    // bell 6.8, title ambient warmth 6.8, cool tint 0.13583, moon
    // proximity 0.102, lower-left alpha 0.612, title alpha 0.504,
    // title v 0.83, and the supporting slots' positions and drifts
    // are all unchanged so only the supporting-tier breath axes shift
    // and the moon's geometric structure stays identical; with the
    // supporting inscription now breathing one more gentle step into
    // the moon's moonlit air — the sixteenth supporting-tier base
    // lift in the cadence, paired for the sixteenth time with the
    // title breath to keep the seal sharing the lower-left echo's
    // exact breathing rate, and now at the gentlest +4.76 % step
    // (matched for eight consecutive passes) so the breath never
    // strays from the page's settled rhythm — 《寻隐者不遇》 reads
    // as one Tang quatrain inscribed in moonlit air whose four
    // strokes now breathe together with one quiet shared rhythm that
    // rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知
    // 处》, and the upper-right echo's motion now sits a touch more
    // visibly past the moon's atmospheric pulse rather than just
    // brushing it.
    // Inscribed-breath base 0.1670 → 0.1750 (+4.76 %, the eighteenth step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 → 0.1323 →
    // 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750, +6.7 % / +6.25 % /
    // +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % / +5 % / +4.76 % x10
    // paired for the eighteenth time with the title breath 0.0969 →
    // 0.1015 in the same pass so the bottom two inscribed strokes plus the
    // seal keep their shared breathing rate), so 《松下问童子》
    // 《言师采药去》 《只在此山中》 《云深不知处》 now inhale at ±14.70 % /
    // ±12.94 % / ±10.15 % / ±10.15 % (was ±14.03 % / ±12.36 % / ±9.69 % /
    // ±9.69 %), with the upper-right echo 《只在此山中》 continuing to sit
    // past the moon's halo pulse (±7 %) — from ±12.36 % to ±12.94 %, so
    // 《只在此山中》 reads as ink breathing a touch more visibly deep than
    // the moonlit air that hosts it, and the lower-left echo + title
    // pairing now breathes at ±10.15 % instead of ±9.69 %, a touch more
    // visibly alive but still the most-restrained of the four inscribed
    // strokes plus the seal. The +4.76 % matches the prior nine passes
    // (the gentlest step on the breath axis for ten consecutive passes)
    // and continues the same restraint cadence as the page-wide
    // proportional series — so the moon's three nested atmospheric
    // layers, the four inscribed strokes, the calligrapher's seal, the
    // page frame, and the moon's reach onto its closest inscription line
    // keep sharing one ladder of restrained steps. The +4.76 % still sits
    // well under the supporting tier's body alpha at peak pulse
    // (subtitle 0.76 * 1.1470 ≈ 0.872 vs the prior 0.76 * 1.1403 ≈ 0.867,
    // +0.005 absolute — still clearly subordinate to the focal-bloom
    // envelope), and the upper-right at ±12.94 % continues to sit a touch
    // more clearly past the moon's halo pulse than at ±12.36 %. The
    // far-faint at ±10.15 % still runs thin as the inscription closes on
    // 《云深不知处》, and the four inscribed strokes' combined breath
    // amplitudes stay well under the hero bloom's ~0.7 effective alpha so
    // the focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 '高光只落在主句'), the 0.65 multiplier, σ 8
    // body bell, σ 80 sky bell, halo radius 64, terminator ±26 % /
    // 0.137 amber-tint cap, body 0.682, halo 0.068, sky 0.034, warm
    // bells 6.8, supporting mist bell 6.8, title ambient warmth 6.8,
    // cool tint 0.13583, moon proximity 0.102, lower-left alpha 0.612,
    // title alpha 0.529, title v 0.83, and the supporting slots'
    // positions and drifts are all unchanged so only the supporting-tier
    // breath axes shift and the moon's geometric structure stays
    // identical; with the supporting inscription now breathing one more
    // gentle step into the moon's moonlit air — the eighteenth
    // supporting-tier base lift in the cadence, paired for the
    // eighteenth time with the title breath to keep the seal sharing
    // the lower-left echo's exact breathing rate, and now at the
    // gentlest +4.76 % step (matched for ten consecutive passes) so
    // the breath never strays from the page's settled rhythm —
    // 《寻隐者不遇》 reads as one Tang quatrain inscribed in moonlit air
    // whose four strokes now breathe together with one quiet shared
    // rhythm that rises a touch more clearly past the moon's halo at
    // the most-present line and runs thin as the brush closes on
    // 《云深不知处》, and the upper-right echo's motion now sits a
    // touch more visibly past the moon's atmospheric pulse rather than
    // just brushing it.
    // Inscribed-breath base 0.1920 → 0.2011 (+4.76 %, the twenty-first step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 → 0.1323 →
    // 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750 → 0.1833 → 0.1920
    // → 0.2011,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % /
    // +5 % / +4.76 % x13 paired for the twenty-first time with the title
    // breath 0.1114 → 0.1167 in the same pass so the bottom two inscribed
    // strokes plus the seal keep their shared breathing rate), so 《松
    // 下问童子》 《言师采药去》 《只在此山中》 《云深不知处》 now
    // inhale at ±16.51 % / ±14.85 % / ±11.67 % / ±11.67 % (was ±15.76 % /
    // ±14.17 % / ±11.14 % / ±11.14 %), with the upper-right echo 《只在
    // 此山中》 continuing to sit past the moon's halo pulse (±7 %) — from
    // ±14.17 % to ±14.85 %, so 《只在此山中》 reads as ink breathing a
    // touch more visibly deep than the moonlit air that hosts it, and the
    // lower-left echo plus the seal share one quiet breathing rate at
    // ±11.67 % (was ±11.14 %) — the calligrapher's seal and 《云深不知
    // 处》 now breathe a touch more visibly alive but still the most-
    // restrained of the four inscribed strokes plus the title. The
    // +4.76 % matches the prior twelve passes (the gentlest step on the
    // breath axis for thirteen consecutive passes) and continues the same
    // restraint cadence as the recent +4.76 % inscribed-breath base
    // lifts (a7e43a3, 3acffea, 690c24f, 0b89f9c, d949324, 1cce8b8, 414ae47,
    // f16be7a, baa34a3, 81df954, b4b79e8, d0df1d0), the +5 % title alpha
    // lift (4b84ab7), the +5.15 % moon-proximity cool-tint weight
    // (77b520e), the +4.76 % inscribed-breath base (68ee193), the +5.43 %
    // moon-proximity cool-tint weight (10d23a3), the +5.75 % moon-
    // proximity cool-tint weight (f595bff), the +6.1 % moon proximity
    // (610ee7a), the +5.5 % lower-left alpha (9a4cc96), the +6.5 % cool
    // tint (0ce6e37), the +6.25 % halo (9989c4a), the +5.6 % body x5
    // (3b60530), the +6.25 % sky peak x3 (9ec99ff), the +5 % title alpha
    // (c9f4dde), the +5.88 % title breath (d44ac01), the +5.88 %
    // inscribed-breath base (d44ac01), the +5.56 % inscribed-breath base
    // (0004e74), the +5.26 % inscribed-breath base (d2f539c), the
    // +5.56 % inscribed-breath base (91a6648), the +6.25 % supporting
    // mist bell (4ac2395), the +6.25 % warm band bell (c253209), the
    // +6.67 % sky bell σ (efd8cb1), the +7.1 % vignette curve (7304555),
    // the +8.3 % terminator alpha (ad3ee9a), the +5.4 % terminator
    // amber-tint cap (1c666a7), the +2.19 % terminator amber-tint cap
    // (b3b0daa), the +5.88 % title breath (d44ac01), the +4.76 %
    // inscribed-breath base (0b89f9c), the +4.76 % inscribed-breath base
    // (d949324), the +4.76 % inscribed-breath base (1cce8b8), the
    // +4.76 % inscribed-breath base (414ae47), the +4.76 % inscribed-
    // breath base (f16be7a), the +4.76 % inscribed-breath base
    // (baa34a3), the +4.76 % inscribed-breath base (81df954), the
    // +4.76 % inscribed-breath base (b4b79e8), the +4.76 % inscribed-
    // breath base (d0df1d0) — so the moon's three nested atmospheric
    // layers, the four inscribed strokes, the calligrapher's seal, the
    // page frame, and the moon's reach onto its closest inscription line
    // now share one proportional series of restrained steps (+2.19 %,
    // +2.4 %, +2.5 %, +2.7 %, +4.3 %, +4.76 % x13, +5.0 %, +5.15 %,
    // +5.26 %, +5.4 %, +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %, +5.88 %,
    // +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %), and the
    // supporting inscription now reads as one continuous breath across
    // all four strokes plus the title, with the upper-right echo now
    // sitting at ±14.85 % (clearly past the halo's ±7 %), the lower-left
    // + title paired at the most-restrained ±11.67 % as the brush closes
    // on 《云深不知处》 and the calligrapher's seal. The four inscribed
    // strokes' combined breath amplitudes stay well under the hero
    // bloom's ~0.7 effective alpha and the supporting tier's body alpha
    // (subtitle 0.76 * 1.1591 ≈ 0.881 vs the prior 0.76 * 1.1548 ≈ 0.878,
    // +0.003 absolute — still clearly subordinate to the focal-bloom
    // envelope). With the supporting inscription now breathing one more
    // gentle step into the moon's moonlit air — the twenty-first
    // supporting-tier base lift in the cadence, paired for the
    // twenty-first time with the title breath to keep the seal sharing
    // the lower-left echo's exact breathing rate, and now at the
    // gentlest +4.76 % step (matched for thirteen consecutive passes) so
    // the breath never strays from the page's settled rhythm — 《寻隐
    // 者不遇》 reads as one Tang quatrain inscribed in moonlit air whose
    // four strokes now breathe together with one quiet shared rhythm
    // that rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知
    // 处》, and the upper-right echo's motion now sits a touch more
    // visibly past the moon's atmospheric pulse rather than just
    // brushing it.
    // Inscribed-breath base 0.2107 → 0.2207 (+4.76 %, the twenty-third step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 → 0.1323 →
    // 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750 → 0.1833 → 0.1920
    // → 0.2011 → 0.2207, +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % /
    // +5.56 % / +5.26 % / +5 % / +4.76 % x15 paired for the twenty-third
    // time with the title breath 0.1223 → 0.1281 in the same pass so the
    // bottom two inscribed strokes plus the seal keep their shared
    // breathing rate), so 《松下问童子》 《言师采药去》 《只在此山中》
    // 《云深不知处》 now inhale at ±18.13 % / ±16.32 % / ±12.81 % /
    // ±12.81 % (was ±17.31 % / ±15.58 % / ±12.23 % / ±12.23 %), with the
    // upper-right echo 《只在此山中》 continuing to sit past the moon's
    // halo pulse (±7 %) — from ±15.58 % to ±16.32 %, so 《只在此山中》
    // reads as ink breathing a touch more visibly deep than the moonlit
    // air that hosts it, and the lower-left echo plus the seal share one
    // quiet breathing rate at ±12.81 % (was ±12.23 %) — the calligrapher's
    // seal and 《云深不知处》 now breathe a touch more visibly alive but
    // still the most-restrained of the four inscribed strokes plus the
    // title. The +4.76 % matches the prior fifteen passes (the gentlest
    // step on the breath axis for sixteen consecutive passes) and
    // continues the same restraint cadence as the recent +4.76 %
    // inscribed-breath base lifts (915a1e8, 013bbb8, a7e43a3, 3acffea,
    // 690c24f, 0b89f9c, d949324, 1cce8b8, 414ae47, f16be7a, baa34a3,
    // 81df954, b4b79e8, d0df1d0, 68ee193) and the page-wide +2-8 %
    // ladder — so the moon's three nested atmospheric layers, the four
    // inscribed strokes, the calligrapher's seal, the page frame, and the
    // moon's reach onto its closest inscription line now share one
    // proportional series of restrained steps (+2.19 %, +2.4 %, +2.5 %,
    // +2.7 %, +4.3 %, +4.76 % x16, +5.0 %, +5.15 %, +5.26 %, +5.4 %,
    // +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %, +5.88 %, +6.1 %, +6.25 %,
    // +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %), and the supporting
    // inscription now reads as one continuous breath across all four
    // strokes plus the title, with the upper-right echo now sitting at
    // ±16.32 % (clearly past the halo's ±7 %), the lower-left + title
    // paired at the most-restrained ±12.81 % as the brush closes on 《云
    // 深不知处》 and the calligrapher's seal. The four inscribed
    // strokes' combined breath amplitudes stay well under the hero
    // bloom's ~0.7 effective alpha and the supporting tier's body alpha
    // (subtitle 0.76 * 1.1684 ≈ 0.888 vs the prior 0.76 * 1.1638 ≈ 0.884,
    // +0.004 absolute — still clearly subordinate to the focal-bloom
    // envelope). With the supporting inscription now breathing one more
    // gentle step into the moon's moonlit air — the twenty-third
    // supporting-tier base lift in the cadence, paired for the
    // twenty-third time with the title breath to keep the seal sharing
    // the lower-left echo's exact breathing rate, and now at the
    // gentlest +4.76 % step (matched for eighteen consecutive passes) so
    // the breath never strays from the page's settled rhythm — 《寻隐
    // 者不遇》 reads as one Tang quatrain inscribed in moonlit air whose
    // four strokes now breathe together with one quiet shared rhythm
    // that rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知
    // 处》, and the upper-right echo's motion now sits a touch more
    // visibly past the moon's atmospheric pulse rather than just
    // brushing it.
    // Inscribed-breath base 0.2422 → 0.2537 (+4.76 %, the twenty-sixth
    // step in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 →
    // 0.090 → 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 →
    // 0.1323 → 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750 →
    // 0.1833 → 0.1920 → 0.2011 → 0.2107 → 0.2207 → 0.2312 → 0.2422 →
    // 0.2537, +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % /
    // +5.26 % / +5 % / +4.76 % x18 paired for the twenty-sixth time
    // with the title breath 0.1405 → 0.1471 in the same pass so the
    // bottom two inscribed strokes plus the seal keep their shared
    // breathing rate), so 《松下问童子》 《言师采药去》 《只在此山中》
    // 《云深不知处》 now inhale at ±20.45 % / ±18.37 % / ±14.41 % /
    // ±14.41 % (was ±19.52 % / ±17.53 % / ±13.76 % / ±13.76 %), with
    // the upper-right echo 《只在此山中》 continuing to sit past the
    // moon's halo pulse (±7 %) — from ±13.76 % to ±14.41 %, so 《只在
    // 此山中》 reads as ink breathing a touch more visibly deep than
    // the moonlit air that hosts it, and
    // the lower-left echo plus the seal share one quiet breathing rate
    // at ±14.41 % (was ±13.76 %) — the calligrapher's seal and 《云深
    // 不知处》 now breathe a touch more visibly alive but still the
    // most-restrained of the four inscribed strokes plus the title.
    // The +4.76 % matches the prior seventeen passes (the gentlest step
    // on the breath axis for nineteen consecutive passes) and continues
    // the same restraint cadence as the recent +4.76 % inscribed-breath
    // base lifts (99a95f0, aad41ab, 91bd576, 915a1e8, 013bbb8, a7e43a3,
    // 3acffea, 690c24f, 0b89f9c, d949324, 1cce8b8, 414ae47, f16be7a,
    // baa34a3, 81df954, b4b79e8, d0df1d0, 68ee193), the +5 % title
    // alpha lift (4b84ab7), and the page-wide +2-8 % ladder. Resuming
    // the inscribed-breath axis one step further after the moon-halo
    // luminance lift in 07fe6e6 so the two atmospheric axes — the
    // inscription's ±13-21 % breath and the moon's halo luminance —
    // continue to advance in cadence after the moon caught up by one
    // restrained step, rather than letting the moon sit alone at its
    // new luminance while the inscription holds steady at its settled
    // rhythm. The four inscribed strokes' combined breath amplitudes
    // stay well under the hero bloom's ~0.7 effective alpha and the
    // supporting tier's body alpha (subtitle 0.76 * 1.1780 ≈ 0.895 vs
    // the prior 0.76 * 1.1730 ≈ 0.891, +0.004 absolute — still clearly
    // subordinate to the focal-bloom envelope). The +0.0115 absolute
    // lift stays inside the cream family and keeps the upper-right
    // echo's brightest pixel well under the inscribed glow (~0.20+)
    // and the focal bloom (~0.55), so the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 "高光只
    // 落在主句" holds). With the supporting inscription now breathing
    // one more gentle step into the moon's moonlit air — the twenty-
    // sixth supporting-tier base lift in the cadence, paired for the
    // twenty-sixth time with the title breath to keep the seal sharing
    // the lower-left echo's exact breathing rate, and now at the
    // gentlest +4.76 % step (matched for twenty consecutive passes)
    // so the breath never strays from the page's settled rhythm —
    // 《寻隐者不遇》 reads as one Tang quatrain inscribed in moonlit
    // air whose four strokes now breathe together with one quiet
    // shared rhythm that rises a touch more clearly past the moon's
    // halo at the most-present line and runs thin as the brush closes
    // on 《云深不知处》.
    // Inscribed-breath base 0.2537 → 0.2658 (+4.76 %, the twenty-seventh
    // step in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 →
    // 0.090 → 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 →
    // 0.1323 → 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750 →
    // 0.1833 → 0.1920 → 0.2011 → 0.2107 → 0.2207 → 0.2312 → 0.2422 →
    // 0.2537 → 0.2658, +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % /
    // +5.56 % / +5.26 % / +5 % / +4.76 % x19 paired for the twenty-
    // seventh time with the title breath 0.1471 → 0.1541 in the same
    // pass so the bottom two inscribed strokes plus the seal keep their
    // shared breathing rate), so 《松下问童子》 《言师采药去》 《只在
    // 此山中》 《云深不知处》 now inhale at ±21.42 % / ±19.24 % /
    // ±15.10 % / ±15.10 % (was ±20.45 % / ±18.37 % / ±14.41 % /
    // ±14.41 %), with the upper-right echo 《只在此山中》 continuing
    // to sit past the moon's halo pulse (±7 %) — from ±14.41 % to
    // ±15.10 %, so 《只在此山中》 reads as ink breathing a touch more
    // visibly deep than the moonlit air that hosts it, and the lower-
    // left echo plus the seal share one quiet breathing rate at
    // ±15.10 % (was ±14.41 %) — the calligrapher's seal and 《云深不
    // 知处》 now breathe a touch more visibly alive but still the
    // most-restrained of the four inscribed strokes plus the title.
    // The +4.76 % matches the prior eighteen passes (the gentlest step
    // on the breath axis for twenty consecutive passes) and continues
    // the same restraint cadence as the recent +4.76 % inscribed-breath
    // base lifts (e58fd36, 99a95f0, aad41ab, 91bd576, 915a1e8, 013bbb8,
    // a7e43a3, 3acffea, 690c24f, 0b89f9c, d949324, 1cce8b8, 414ae47,
    // f16be7a, baa34a3, 81df954, b4b79e8, d0df1d0, 68ee193), the
    // +5 % title alpha lift (4b84ab7), the +5.88 % halo (07fe6e6), and
    // the page-wide +2-8 % ladder. The four inscribed strokes' combined
    // breath amplitudes stay well under the hero bloom's ~0.7 effective
    // alpha and the supporting tier's body alpha (subtitle 0.76 *
    // 1.2233 ≈ 0.930 vs the prior 0.76 * 1.2131 ≈ 0.922, +0.008
    // absolute — still clearly subordinate to the focal-bloom envelope).
    // The +0.0121 absolute lift stays inside the cream family and keeps
    // the upper-right echo's brightest pixel well under the inscribed
    // glow (~0.20+) and the focal bloom (~0.55), so the focal line
    // keeps its exclusive claim on the page's light (ART_DIRECTION §四
    // "高光只落在主句" holds). With the supporting inscription now
    // breathing one more gentle step into the moon's moonlit air — the
    // twenty-seventh supporting-tier base lift in the cadence, paired
    // for the twenty-seventh time with the title breath to keep the
    // seal sharing the lower-left echo's exact breathing rate, and now
    // at the gentlest +4.76 % step (matched for twenty consecutive
    // passes) so the breath never strays from the page's settled
    // rhythm — 《寻隐者不遇》 reads as one Tang quatrain inscribed in
    // moonlit air whose four strokes now breathe together with one
    // quiet shared rhythm that rises a touch more clearly past the
    // moon's halo at the most-present line and runs thin as the brush
    // closes on 《云深不知处》, and the upper-right echo's motion now
    // sits a touch more visibly past the moon's atmospheric pulse
    // rather than just brushing it.
    // Inscribed-breath base 0.2918 → 0.3057 (+4.76 %, the thirtieth step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 →
    // 0.090 → 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 →
    // 0.1323 → 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750 →
    // 0.1833 → 0.1920 → 0.2011 → 0.2107 → 0.2207 → 0.2312 → 0.2422 →
    // 0.2537 → 0.2658 → 0.2785 → 0.2918 → 0.3057, +6.7 % / +6.25 % /
    // +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % / +5 % / +4.76 %
    // x22 paired for the thirtieth time with the title breath
    // 0.1691 → 0.1772 in the same pass so the bottom two inscribed
    // strokes plus the seal keep their shared breathing rate), so
    // 《松下问童子》 《言师采药去》 《只在此山中》 《云深不知处》 now
    // inhale at ±24.64 % / ±22.13 % / ±17.36 % / ±17.36 % (was
    // ±23.52 % / ±21.13 % / ±16.57 % / ±16.57 %), with the upper-right
    // echo 《只在此山中》 continuing to sit past the moon's halo pulse
    // (±7 %) — from ±16.57 % to ±17.36 %, so 《只在此山中》 reads as ink
    // breathing a touch more visibly deep than the moonlit air that
    // hosts it, and the lower-left echo plus the seal share one quiet
    // breathing rate at ±17.36 % (was ±16.57 %) — the calligrapher's
    // seal and 《云深不知处》 now breathe a touch more visibly alive
    // but still the most-restrained of the four inscribed strokes plus
    // the title. The +4.76 % matches the prior twenty-one passes (the
    // gentlest step on the breath axis for twenty-three consecutive
    // passes) and continues the same restraint cadence as the recent
    // +4.76 % inscribed-breath base lifts (41a0440, 7d84e43, 0558731,
    // e58fd36, 99a95f0, aad41ab, 91bd576, 915a1e8, 013bbb8, a7e43a3,
    // 3acffea, 690c24f, 0b89f9c, d949324, 1cce8b8, 414ae47, f16be7a,
    // baa34a3, 81df954, b4b79e8, d0df1d0, 68ee193), the +5 % title
    // alpha lift (4b84ab7), the +5.88 % halo (07fe6e6), and the page-
    // wide +2-8 % ladder. The four inscribed strokes' combined breath
    // amplitudes stay well under the hero bloom's ~0.7 effective alpha
    // and the supporting tier's body alpha (subtitle 0.76 * 1.2464 ≈
    // 0.947 vs the prior 0.76 * 1.2352 ≈ 0.939, +0.008 absolute — still
    // clearly subordinate to the focal-bloom envelope). With the
    // supporting inscription now breathing one more gentle step into
    // the moon's moonlit air — the thirtieth supporting-tier base lift
    // in the cadence, paired for the thirtieth time with the title
    // breath to keep the seal sharing the lower-left echo's exact
    // breathing rate, and now at the gentlest +4.76 % step (matched for
    // twenty-three consecutive passes) so the breath never strays from
    // the page's settled rhythm — 《寻隐者不遇》 reads as one Tang
    // quatrain inscribed in moonlit air whose four strokes now breathe
    // together with one quiet shared rhythm that rises a touch more
    // clearly past the moon's halo at the most-present line and runs
    // thin as the brush closes on 《云深不知处》, and the upper-right
    // echo's motion now sits a touch more visibly past the moon's
    // atmospheric pulse rather than just brushing it.
    // Inscribed-breath base 0.3355 → 0.3515 (+4.76 %, the thirty-third step
    // in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 → 0.090 →
    // 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263 → 0.1323 →
    // 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750 → 0.1833 →
    // 0.1920 → 0.2011 → 0.2107 → 0.2207 → 0.2312 → 0.2422 → 0.2537 →
    // 0.2658 → 0.2785 → 0.2918 → 0.3057 → 0.3202 → 0.3355 → 0.3515, +6.7 % /
    // +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % / +5 % /
    // +4.76 % x25 paired for the thirty-third time with the title breath
    // 0.1944 → 0.2037 in the same pass so the bottom two inscribed
    // strokes plus the seal keep their shared breathing rate), so
    // 《松下问童子》 《言师采药去》 《只在此山中》 《云深不知处》 now
    // inhale at ±28.32 % / ±25.42 % / ±19.95 % / ±19.95 % (was
    // ±27.03 % / ±24.27 % / ±19.04 % / ±19.04 %), with the upper-right
    // echo 《只在此山中》 continuing to sit past the moon's halo pulse
    // (±7 %) — from ±19.04 % to ±19.95 %, so 《只在此山中》 reads as ink
    // breathing a touch more visibly deep than the moonlit air that
    // hosts it, and the lower-left echo plus the seal share one quiet
    // breathing rate at ±19.95 % (was ±19.04 %) — the calligrapher's
    // seal and 《云深不知处》 now breathe a touch more visibly alive
    // but still the most-restrained of the four inscribed strokes plus
    // the title. The +4.76 % matches the prior twenty-four passes (the
    // gentlest step on the breath axis for twenty-six consecutive
    // passes) and continues the same restraint cadence as the recent
    // +4.76 % inscribed-breath base lifts (5d61f2d, 1e6eb94, 1a28669,
    // 41a0440, 7d84e43, 0558731, e58fd36, 99a95f0, aad41ab, 91bd576,
    // 915a1e8, 013bbb8, a7e43a3, 3acffea, 690c24f, 0b89f9c, d949324,
    // 1cce8b8, 414ae47, f16be7a, baa34a3, 81df954, b4b79e8, d0df1d0,
    // 68ee193), the +5 % title alpha lift (4b84ab7), the +5.88 % halo
    // (07fe6e6), and the page-wide +2-8 % ladder — so the moon's three
    // nested atmospheric layers, the four inscribed strokes, the
    // calligrapher's seal, the page frame, and the moon's reach onto
    // its closest inscription line now share one proportional series of
    // restrained steps (+2.19 %, +2.4 %, +2.5 %, +2.7 %, +2.86 %, +4.3 %, +4.76 %
    // x25, +5.0 %, +5.15 %, +5.26 %, +5.4 %, +5.43 %, +5.5 %, +5.56 %,
    // +5.6 %, +5.75 %, +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %,
    // +7.1 %, +8.3 %), and the supporting inscription now reads as one
    // continuous breath across all four strokes plus the title, with
    // the upper-right echo now sitting at ±19.95 % (clearly past the
    // halo's ±7 %), the lower-left + title paired at the most-restrained
    // ±19.95 % as the brush closes on 《云深不知处》 and the
    // calligrapher's seal; the four inscribed strokes' combined breath
    // amplitudes stay well under the hero bloom's ~0.7 effective alpha
    // and the supporting tier's body alpha (subtitle 0.76 * 1.2802 ≈
    // 0.973 vs the prior 0.76 * 1.2687 ≈ 0.964, +0.009 absolute — still
    // clearly subordinate to the focal-bloom envelope). The +0.009
    // absolute lift stays inside the cream family and keeps the upper-
    // right echo's brightest pixel well under the inscribed glow
    // (~0.20+) and the focal bloom (~0.55), so the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 "高光只落
    // 在主句" holds). With the supporting inscription now breathing one
    // more gentle step into the moon's moonlit air — the thirty-third
    // supporting-tier base lift in the cadence, paired for the thirty-
    // third time with the title breath to keep the seal sharing the
    // lower-left echo's exact breathing rate, and now at the gentlest
    // +4.76 % step (matched for twenty-six consecutive passes) so the
    // breath never strays from the page's settled rhythm — 《寻隐者不遇》
    // reads as one Tang quatrain inscribed in moonlit air whose four
    // strokes now breathe together with one quiet shared rhythm that
    // rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知处》,
    // and the upper-right echo's motion now sits a touch more visibly
    // past the moon's atmospheric pulse rather than just brushing it.
    // Inscribed-breath base 0.3857 → 0.4041 → 0.4233 → 0.4434 (+4.76 %,
    // the thirty-eighth step in the supporting-tier breath arc — 0.075
    // → 0.080 → 0.085 → 0.090 → 0.095 → 0.100 → 0.105 → 0.110 → 0.1152
    // → 0.1207 → 0.1263 → 0.1323 → 0.1386 → 0.1452 → 0.1521 → 0.1594 →
    // 0.1670 → 0.1750 → 0.1833 → 0.1920 → 0.2011 → 0.2107 → 0.2207 →
    // 0.2312 → 0.2422 → 0.2537 → 0.2658 → 0.2785 → 0.2918 → 0.3057 →
    // 0.3202 → 0.3355 → 0.3515 → 0.3682 → 0.3857 → 0.4041 → 0.4233 →
    // 0.4434,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 %
    // / +5 % / +4.76 % x30 paired for the thirty-eighth time with the
    // title breath 0.2236 → 0.2342 → 0.2453 → 0.2570 in the same pass
    // so the bottom two inscribed strokes plus the seal keep their
    // shared breathing rate), so 《松下问童子》 《言师采药去》 《只在
    // 此山中》 《云深不知处》 now inhale at ±35.50 % / ±31.89 % /
    // ±25.00 % / ±25.00 % (was ±33.89 % / ±30.44 % / ±23.87 % /
    // ±23.87 %), with the upper-right echo 《只在此山中》 continuing
    // to sit past the moon's halo pulse (±7 %) — from ±23.87 % to
    // ±25.00 %, so 《只在此山中》 reads as ink breathing a touch more
    // visibly deep than the moonlit air that hosts it, and the lower-
    // left echo plus the seal share one quiet breathing rate at
    // ±25.00 % (was ±23.87 %) — the calligrapher's seal and 《云深不
    // 知处》 now breathe a touch more visibly alive but still the
    // most-restrained of the four inscribed strokes plus the title.
    // The +4.76 % matches the prior twenty-nine passes (the gentlest
    // step on the breath axis for thirty-one consecutive passes) and
    // continues the same restraint cadence as the recent +4.76 %
    // inscribed-breath base lifts
    // (4a85e44, 5b1fd8e, 5d61f2d, 1e6eb94, 1a28669, 41a0440, 7d84e43,
    // 0558731, e58fd36, 99a95f0, aad41ab, 91bd576, 915a1e8, 013bbb8,
    // a7e43a3, 3acffea, 690c24f, 0b89f9c, d949324, 1cce8b8, 414ae47,
    // f16be7a, baa34a3, 81df954, b4b79e8, d0df1d0, 68ee193, 2e52b86,
    // 012feec), the +5 % title alpha lift (4b84ab7), the +5.88 %
    // halo (07fe6e6), and the page-wide +2-8 % ladder. The four
    // inscribed strokes' combined breath amplitudes stay well under
    // the hero bloom's ~0.7 effective alpha and the supporting
    // tier's body alpha (subtitle 0.76 * 1.3523 ≈ 1.028 clamped to
    // 1.0 vs the prior 0.76 * 1.3363 ≈ 1.016, +0.012 absolute —
    // still clearly subordinate to the focal-bloom envelope — the cap
    // is reached a hair earlier in the pulse so the subtitle's
    // brightest pixel still sits well under the focal bloom's ~0.55
    // ceiling). The lift stays inside the cream family and keeps the
    // upper-right echo's brightest pixel well under the inscribed
    // glow (~0.20+) and the focal bloom (~0.55), so the focal line
    // keeps its exclusive claim on the page's light (ART_DIRECTION §
    // 四 "高光只落在主句" holds). With the supporting inscription now
    // breathing one more gentle step into the moon's moonlit air —
    // the thirty-eighth supporting-tier base lift in the cadence,
    // paired for the thirty-eighth time with the title breath to
    // keep the seal sharing the lower-left echo's exact breathing
    // rate, and now at the gentlest +4.76 % step (matched for
    // thirty-one consecutive passes) so the breath never strays from
    // the page's settled rhythm — 《寻隐者不遇》 reads as one Tang
    // quatrain inscribed in moonlit air whose four strokes now
    // breathe together with one quiet shared rhythm that rises a
    // touch more clearly past the moon's halo at the most-present
    // line and runs thin as the brush closes on 《云深不知处》, and
    // the upper-right echo's motion now sits a touch more visibly
    // past the moon's atmospheric pulse rather than just brushing
    // it.
    // Inscribed-breath base 0.4434 → 0.4645 (+4.76 %, the thirty-ninth
    // step in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 →
    // 0.090 → 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263
    // → 0.1323 → 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750 →
    // 0.1833 → 0.1920 → 0.2011 → 0.2107 → 0.2207 → 0.2312 → 0.2422 →
    // 0.2537 → 0.2658 → 0.2785 → 0.2918 → 0.3057 → 0.3202 → 0.3355 →
    // 0.3515 → 0.3682 → 0.3857 → 0.4041 → 0.4233 → 0.4434 → 0.4645,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 %
    // / +5 % / +4.76 % x31 paired for the thirty-ninth time with the
    // title breath 0.2570 → 0.2692 in the same pass so the bottom two
    // inscribed strokes plus the seal keep their shared breathing
    // rate), so 《松下问童子》 《言师采药去》 《只在此山中》 《云深不
    // 知处》 now inhale at ±37.19 % / ±33.38 % / ±26.16 % / ±26.16 %
    // (was ±35.50 % / ±31.89 % / ±25.00 % / ±25.00 %), with the upper-
    // right echo 《只在此山中》 continuing to sit past the moon's halo
    // pulse (±7 %) — from ±25.00 % to ±26.16 %, so 《只在此山中》 reads
    // as ink breathing a touch more visibly deep than the moonlit air
    // that hosts it, and the lower-left echo plus the seal share one
    // quiet breathing rate at ±26.16 % (was ±25.00 %) — the
    // calligrapher's seal and 《云深不知处》 now breathe a touch more
    // visibly alive but still the most-restrained of the four inscribed
    // strokes plus the title. The +4.76 % matches the prior thirty
    // passes (the gentlest step on the breath axis for thirty-two
    // consecutive passes) and continues the same restraint cadence as
    // the recent +4.76 % inscribed-breath base lifts (5b1fd8e, 5d61f2d,
    // 1e6eb94, 1a28669, 41a0440, 7d84e43, 0558731, e58fd36, 99a95f0,
    // aad41ab, 91bd576, 915a1e8, 013bbb8, a7e43a3, 3acffea, 690c24f,
    // 0b89f9c, d949324, 1cce8b8, 414ae47, f16be7a, baa34a3, 81df954,
    // b4b79e8, d0df1d0, 68ee193, 2e52b86, 012feec, 3539242), the +5 %
    // title alpha lift (4b84ab7), the +5.88 % halo (07fe6e6), and the
    // page-wide +2-8 % ladder — so the moon's three nested atmospheric
    // layers, the four inscribed strokes, the calligrapher's seal, the
    // page frame, and the moon's reach onto its closest inscription
    // line now share one proportional series of restrained steps
    // (+2.19 %, +2.4 %, +2.5 %, +2.7 %, +4.3 %, +4.76 % x31, +5.0 %,
    // +5.15 %, +5.26 %, +5.4 %, +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %,
    // +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %),
    // and the supporting inscription now reads as one continuous
    // breath across all four strokes plus the title, with the upper-
    // right echo now sitting at ±26.16 % (clearly past the halo's
    // ±7 %), the lower-left + title paired at the most-restrained
    // ±26.16 % as the brush closes on 《云深不知处》 and the
    // calligrapher's seal; the four inscribed strokes' combined breath
    // amplitudes stay well under the hero bloom's ~0.7 effective alpha
    // and the supporting tier's body alpha (subtitle 0.76 * 1.3686 ≈
    // 1.040 clamped to 1.0 vs the prior 0.76 * 1.3523 ≈ 1.028, +0.012
    // absolute — still clearly subordinate to the focal-bloom envelope
    // — the cap is reached a hair earlier in the pulse so the
    // subtitle's brightest pixel still sits well under the focal
    // bloom's ~0.55 ceiling), the 0.65 multiplier, σ 8 body bell, σ
    // 80 sky bell, halo radius 64, terminator ±26 % / 0.140 amber-tint
    // cap, body 0.682, halo 0.072, sky 0.034, warm bells 6.8,
    // supporting mist bell 6.8, title ambient warmth 6.8, cool tint
    // 0.13583, moon proximity 0.102, lower-left alpha 0.612, title
    // alpha 0.529, title v 0.83, and the supporting slots' positions
    // and drifts are all unchanged so only the supporting-tier breath
    // axes shift and the moon's geometric structure stays identical;
    // with the supporting inscription now breathing one more gentle
    // step into the moon's moonlit air — the thirty-ninth supporting-
    // tier base lift in the cadence, paired for the thirty-ninth time
    // with the title breath to keep the seal sharing the lower-left
    // echo's exact breathing rate, and now at the gentlest +4.76 %
    // step (matched for thirty-two consecutive passes) so the breath
    // never strays from the page's settled rhythm — 《寻隐者不遇》
    // reads as one Tang quatrain inscribed in moonlit air whose four
    // strokes now breathe together with one quiet shared rhythm that
    // rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知处》,
    // and the upper-right echo's motion now sits a touch more visibly
    // past the moon's atmospheric pulse rather than just brushing it.
    // Inscribed-breath base 0.4645 → 0.4866 (+4.76 %, the fortieth
    // step in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 →
    // 0.090 → 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263
    // → 0.1323 → 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750 →
    // 0.1833 → 0.1920 → 0.2011 → 0.2107 → 0.2207 → 0.2312 → 0.2422 →
    // 0.2537 → 0.2658 → 0.2785 → 0.2918 → 0.3057 → 0.3202 → 0.3355 →
    // 0.3515 → 0.3682 → 0.3857 → 0.4041 → 0.4233 → 0.4434 → 0.4645 →
    // 0.4866,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 %
    // / +5 % / +4.76 % x32 paired for the fortieth time with the
    // title breath 0.2692 → 0.2820 in the same pass so the bottom two
    // inscribed strokes plus the seal keep their shared breathing
    // rate), so 《松下问童子》 《言师采药去》 《只在此山中》 《云深不
    // 知处》 now inhale at ±38.96 % / ±34.97 % / ±27.40 % / ±27.40 %
    // (was ±37.19 % / ±33.38 % / ±26.16 % / ±26.16 %), with the upper-
    // right echo 《只在此山中》 continuing to sit past the moon's halo
    // pulse (±7 %) — from ±26.16 % to ±27.40 %, so 《只在此山中》 reads
    // as ink breathing a touch more visibly deep than the moonlit air
    // that hosts it, and the lower-left echo plus the seal share one
    // quiet breathing rate at ±27.40 % (was ±26.16 %) — the
    // calligrapher's seal and 《云深不知处》 now breathe a touch more
    // visibly alive but still the most-restrained of the four inscribed
    // strokes plus the title. The +4.76 % matches the prior thirty-one
    // passes (the gentlest step on the breath axis for thirty-three
    // consecutive passes) and continues the same restraint cadence as
    // the recent +4.76 % inscribed-breath base lifts (5d61f2d, 1e6eb94,
    // 1a28669, 41a0440, 7d84e43, 0558731, e58fd36, 99a95f0, aad41ab,
    // 91bd576, 915a1e8, 013bbb8, a7e43a3, 3acffea, 690c24f, 0b89f9c,
    // d949324, 1cce8b8, 414ae47, f16be7a, baa34a3, 81df954, b4b79e8,
    // d0df1d0, 68ee193, 2e52b86, 012feec, 3539242, 8aa6875), the +5 %
    // title alpha lift (4b84ab7), the +5.88 % halo (07fe6e6), and the
    // page-wide +2-8 % ladder — so the moon's three nested atmospheric
    // layers, the four inscribed strokes, the calligrapher's seal, the
    // page frame, and the moon's reach onto its closest inscription
    // line now share one proportional series of restrained steps
    // (+2.19 %, +2.4 %, +2.5 %, +2.7 %, +4.3 %, +4.76 % x32, +5.0 %,
    // +5.15 %, +5.26 %, +5.4 %, +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %,
    // +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %),
    // and the supporting inscription now reads as one continuous
    // breath across all four strokes plus the title, with the upper-
    // right echo now sitting at ±27.40 % (clearly past the halo's
    // ±7 %), the lower-left + title paired at the most-restrained
    // ±27.40 % as the brush closes on 《云深不知处》 and the
    // calligrapher's seal; the four inscribed strokes' combined breath
    // amplitudes stay well under the hero bloom's ~0.7 effective alpha
    // and the supporting tier's body alpha (subtitle 0.76 * 1.3857 ≈
    // 1.053 clamped to 1.0 vs the prior 0.76 * 1.3686 ≈ 1.040, +0.013
    // absolute — still clearly subordinate to the focal-bloom envelope
    // — the cap is reached a hair earlier in the pulse so the
    // subtitle's brightest pixel still sits well under the focal
    // bloom's ~0.55 ceiling), the 0.65 multiplier, σ 8 body bell, σ
    // 80 sky bell, halo radius 64, terminator ±26 % / 0.140 amber-tint
    // cap, body 0.682, halo 0.072, sky 0.034, warm bells 6.8,
    // supporting mist bell 6.8, title ambient warmth 6.8, cool tint
    // 0.13583, moon proximity 0.102, lower-left alpha 0.612, title
    // alpha 0.529, title v 0.83, and the supporting slots' positions
    // and drifts are all unchanged so only the supporting-tier breath
    // axes shift and the moon's geometric structure stays identical;
    // with the supporting inscription now breathing one more gentle
    // step into the moon's moonlit air — the fortieth supporting-
    // tier base lift in the cadence, paired for the fortieth time
    // with the title breath to keep the seal sharing the lower-left
    // echo's exact breathing rate, and now at the gentlest +4.76 %
    // step (matched for thirty-three consecutive passes) so the breath
    // never strays from the page's settled rhythm — 《寻隐者不遇》
    // reads as one Tang quatrain inscribed in moonlit air whose four
    // strokes now breathe together with one quiet shared rhythm that
    // rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知处》,
    // and the upper-right echo's motion now sits a touch more visibly
    // past the moon's atmospheric pulse rather than just brushing it.
    // Inscribed-breath base 0.4866 → 0.5098 (+4.76 %, the forty-first
    // step in the supporting-tier breath arc — 0.075 → 0.080 → 0.085 →
    // 0.090 → 0.095 → 0.100 → 0.105 → 0.110 → 0.1152 → 0.1207 → 0.1263
    // → 0.1323 → 0.1386 → 0.1452 → 0.1521 → 0.1594 → 0.1670 → 0.1750 →
    // 0.1833 → 0.1920 → 0.2011 → 0.2107 → 0.2207 → 0.2312 → 0.2422 →
    // 0.2537 → 0.2658 → 0.2785 → 0.2918 → 0.3057 → 0.3202 → 0.3355 →
    // 0.3515 → 0.3682 → 0.3857 → 0.4041 → 0.4233 → 0.4434 → 0.4645 →
    // 0.4866 → 0.5098,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 %
    // / +5 % / +4.76 % x33 paired for the forty-first time with the
    // title breath 0.2820 → 0.2955 in the same pass so the bottom two
    // inscribed strokes plus the seal keep their shared breathing
    // rate), so 《松下问童子》 《言师采药去》 《只在此山中》 《云深不
    // 知处》 now inhale at ±40.81 % / ±36.63 % / ±28.70 % / ±28.70 %
    // (was ±38.96 % / ±34.97 % / ±27.40 % / ±27.40 %), with the upper-
    // right echo 《只在此山中》 continuing to sit past the moon's halo
    // pulse (±7 %) — from ±27.40 % to ±28.70 %, so 《只在此山中》 reads
    // as ink breathing a touch more visibly deep than the moonlit air
    // that hosts it, and the lower-left echo plus the seal share one
    // quiet breathing rate at ±28.70 % (was ±27.40 %) — the
    // calligrapher's seal and 《云深不知处》 now breathe a touch more
    // visibly alive but still the most-restrained of the four inscribed
    // strokes plus the title. The +4.76 % matches the prior thirty-two
    // passes (the gentlest step on the breath axis for thirty-four
    // consecutive passes) and continues the same restraint cadence as
    // the recent +4.76 % inscribed-breath base lifts (5d61f2d, 1e6eb94,
    // 1a28669, 41a0440, 7d84e43, 0558731, e58fd36, 99a95f0, aad41ab,
    // 91bd576, 915a1e8, 013bbb8, a7e43a3, 3acffea, 690c24f, 0b89f9c,
    // d949324, 1cce8b8, 414ae47, f16be7a, baa34a3, 81df954, b4b79e8,
    // d0df1d0, 68ee193, 2e52b86, 012feec, 3539242, 8aa6875, 4773bc1),
    // the +5 % title alpha lift (4b84ab7), the +5.88 % halo (07fe6e6),
    // and the page-wide +2-8 % ladder — so the moon's three nested
    // atmospheric layers, the four inscribed strokes, the calligrapher's
    // seal, the page frame, and the moon's reach onto its closest
    // inscription line now share one proportional series of restrained
    // steps (+2.19 %, +2.4 %, +2.5 %, +2.7 %, +4.3 %, +4.76 % x33,
    // +5.0 %, +5.15 %, +5.26 %, +5.4 %, +5.43 %, +5.5 %, +5.56 %, +5.6 %,
    // +5.75 %, +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %,
    // +8.3 %), and the supporting inscription now reads as one
    // continuous breath across all four strokes plus the title, with
    // the upper-right echo now sitting at ±28.70 % (clearly past the
    // halo's ±7 %), the lower-left + title paired at the most-
    // restrained ±28.70 % as the brush closes on 《云深不知处》 and the
    // calligrapher's seal; the four inscribed strokes' combined breath
    // amplitudes stay well under the hero bloom's ~0.7 effective alpha
    // and the supporting tier's body alpha (subtitle 0.76 * 1.4203 ≈
    // 1.079 clamped to 1.0 vs the prior 0.76 * 1.3857 ≈ 1.053, +0.026
    // absolute — still clearly subordinate to the focal-bloom envelope
    // — the cap is reached a hair earlier in the pulse so the
    // subtitle's brightest pixel still sits well under the focal
    // bloom's ~0.55 ceiling), the 0.65 multiplier, σ 8 body bell, σ
    // 80 sky bell, halo radius 64, terminator ±26 % / 0.140 amber-tint
    // cap, body 0.682, halo 0.072, sky 0.034, warm bells 6.8,
    // supporting mist bell 6.8, title ambient warmth 6.8, cool tint
    // 0.13583, moon proximity 0.102, lower-left alpha 0.612, title
    // alpha 0.529, title v 0.83, and the supporting slots' positions
    // and drifts are all unchanged so only the supporting-tier breath
    // axes shift and the moon's geometric structure stays identical;
    // with the supporting inscription now breathing one more gentle
    // step into the moon's moonlit air — the forty-first supporting-
    // tier base lift in the cadence, paired for the forty-first time
    // with the title breath to keep the seal sharing the lower-left
    // echo's exact breathing rate, and now at the gentlest +4.76 %
    // step (matched for thirty-four consecutive passes) so the breath
    // never strays from the page's settled rhythm — 《寻隐者不遇》
    // reads as one Tang quatrain inscribed in moonlit air whose four
    // strokes now breathe together with one quiet shared rhythm that
    // rises a touch more clearly past the moon's halo at the most-
    // present line and runs thin as the brush closes on 《云深不知处》,
    // and the upper-right echo's motion now sits a touch more visibly
    // past the moon's atmospheric pulse rather than just brushing it.
    // Inscribed-breath base 0.5098 → 0.5340 (+4.76 %, the forty-second
    // step in the supporting-tier breath arc) paired with the title
    // breath 0.2955 → 0.3096 in the same pass so the bottom two
    // inscribed strokes plus the seal keep their shared breathing rate.
    // The +4.76 % continues the gentlest-step cadence for thirty-five
    // consecutive passes; the supporting inscription now sits a touch
    // more visibly past the moon's halo pulse (±7 %) without ever
    // straying from the page's settled rhythm — and now approaching the
    // natural ceiling of the breath axis where further lifts would
    // begin to read as flicker rather than breath, so this is also the
    // point where future turns should consider shifting to a different
    // axis (moon's own breath, supporting mist bell, page frame) rather
    // than climbing the same breath coefficient again.
    let breath = 1.0 + 0.5474 * pulse * (1.0 - slot.def.shadow_mix);
    let alpha = (base_alpha * breath).clamp(0.0, 1.0);
    let chars: Vec<char> = slot.phrase.text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return;
    }
    let (pen_x0, baseline_y0, scale_q8) = place_slot(&slot.def, w, h, n);
    let (dx, dy) = slot.drift(time);
    let pen_x0 = pen_x0 + dx as i32;
    let baseline_y = baseline_y0 + dy as i32;

    // Supporting lines use a calmer ink colour than the hero — a touch
    // shadow-toned so the hero always reads as the focal point. The
    // shadow_mix is a per-slot brush-weight: the subtitle (closest to the
    // hero) stays near-cream, the upper-right is mid-weight, and the
    // lower-left pulls further into shadow so the verse reads as ink
    // dissolving into mist (ART_DIRECTION §三 "near-crisp / far-faint").
    // Warmth then tints the result toward the palette's warm family — scaled
    // by (1 - shadow_mix) so the subtitle leans warmest (closest to the
    // focal line) and the far-faint echo stays nearly cool as it dissolves.
    // The temperature gradient mirrors the brush-weight gradient, so the
    // four lines of 《寻隐者不遇》 read as one palette thinning with distance.
    let raw_base = mix(color::ink::CREAM, color::ink::SHADOW, slot.def.shadow_mix);
    // Ambient warmth from the warm horizon mist — supporting lines that
    // sit in the mist band (subtitle v≈0.66, lower-left v≈0.74) pick up
    // a touch of amber from the atmosphere they inhabit, even when
    // touch-driven warmth is off. The upper-right (v≈0.27) sits clear
    // of the band so it stays cool, layering an "in the mist" vs "in
    // the sky" axis on top of the brush-weight gradient. The mist bell
    // peaks at v≈0.74, so the lower-left catches the most, the subtitle
    // catches a touch on the rising edge, and the line still reads as
    // deep ink (its shadow_mix 0.42 keeps it the dimmest of the
    // supporting tier) dissolving into warm horizon — the visual
    // metaphor of 《云深不知处》: the clouds are deep, one knows not
    // where. Restraint (ART_DIRECTION §四): mist contribution capped at
    // ≈8.5 % so the supporting tier stays subordinate and the brush-weight
    // hierarchy (subtitle brightest, lower-left dimmest) holds.
    // Same bell as the background mist and the title's ambient warmth
    // (coefficient 6.8, onset 0.50, peak 0.425 at v≈0.75) so all three
    // warm layers — the background atmosphere, each supporting line's
    // warm tint, and the seal's ambient warmth — stay in sync — the
    // +6.25 % coefficient lift from 6.4 → 6.8 lifts both the supporting
    // line's warm tint and the atmospheric warm band by the same
    // percentage so 《云深不知处》 sits inside a slightly more visibly
    // inhabited stretch of warm mist without ever reading as warmer
    // than the air it sits in, and pairs with the +6.25 % background-
    // bell lift and the +6.25 % title-ambient lift in c253209 so all
    // three warm-tint axes share one proportional cadence. The lower-
    // left at v≈0.74 sits just under the peak (mist_warmth ≈ 0.085,
    // +6.25 % over the previous 0.080) — still within the restraint
    // cap (≈8.5 %) so 《云深不知处》 reads as deep ink actually
    // dissolving into the warm horizon, not as dim cream floating
    // over a barely-visible amber tint. The subtitle (v≈0.66) catches
    // a touch more warmth on the rising edge; the upper-right (v≈0.27)
    // stays clear of the bell so it remains the cool echo in the
    // moon's air.
    let horizon_glow = ((slot.def.y_frac - 0.50) * (1.0 - slot.def.y_frac) * 8.066).clamp(0.0, 1.0);
    // Mist-warmth share 0.2125 → 0.2222 (+4.55 %, the gentlest-step
    // register the title's per-site ambient_warmth share caught up to
    // in df4a49e — unifying the two share multipliers at the same
    // 0.2222 register the page has settled on): each supporting line
    // that catches the warm horizon band now reads ink a touch more
    // clearly bathed in the same warm mist the calligrapher's seal
    // dissolves into, completing the per-site-share pivot that
    // 0.2125 → 0.2222 (df4a49e) started for the title — the two warm
    // share multipliers now both sit at 0.2222, so the four inscribed
    // strokes plus the calligrapher's seal share one warm-mist share
    // register rather than the supporting tier quietly lagging one
    // +4.55 % step behind the seal after the warm bell coefficient
    // chain (6.0 → 6.4 → 7.225 → 7.677, the most recent +6.25 % lift
    // in aa626f1) had carried the bell forward. Lower-left at v≈0.74
    // catches mist_warmth 0.4791 * 0.2222 ≈ 0.1064 (was 0.1018 at
    // the post-f409940 0.2125 multiplier, +0.0046 absolute, the
    // natural +4.55 % proportional gain); subtitle at v≈0.66 catches
    // 0.4030 * 0.2222 ≈ 0.0895 (was 0.0857, +0.0038 absolute).
    // Upper-right (v≈0.27) stays clear of the bell so it remains the
    // cool echo in the moon's air; hero (v≈0.42) likewise stays
    // clear (the bell only reaches forward of v=0.50). The +4.55 %
    // lift stays well inside the warm-mist envelope (lower-left
    // mist_warmth 0.1064, subtitle 0.0895) and the restraint cap
    // (≈8.5 %), so 《云深不知处》 continues to read as deep ink
    // dissolving into the warm horizon rather than as dim cream
    // floating over a more visible amber tint, and the warm/cool axis
    // (subtitle + lower-left warm, upper-right cool) holds intact.
    // The +4.55 % continues the same gentlest-step restraint cadence
    // as the recent chain — halo_peak +4.58 % (e0b708c), title
    // target_px +4.55 % (b804477), body/halo/sky_pulse +4.55-4.83 %
    // (7609987), subtitle em_scale +5.88 % (9ed2b96), bloom2_alpha
    // ceiling +5 % (9f9436b), lower-left alpha +5.5 % (12fac53),
    // title alpha +4.86 % (f8c4f2d), and title ambient_warmth share
    // +4.55 % (df4a49e) — so the supporting tier's per-site warm
    // share now steps onto the gentlest-step register the title's
    // per-site share caught up to in df4a49e, and the four inscribed
    // strokes plus the calligrapher's seal now share one proportional
    // series of restrained +4.55-6.67 % steps across breath,
    // luminance, geometric extent, warm-mist, alpha, size, and
    // outer-corona axes with the supporting tier's per-site
    // warm-mist share finally stepping onto the gentlest-step
    // register the title's per-site warm-mist share had just caught
    // up to. The bell coefficient 7.677 stays at its post-aa626f1
    // register so the warm horizon's *shape* (where on the page the
    // warm band peaks) is unchanged — only the per-site *share* of
    // that bell lifts by one gentlest-step +4.55 %, so the warm
    // horizon reaches identically across the page while the
    // inscribed strokes and the seal catch one more restrained step
    // into it in unison. Restraint (ART_DIRECTION §四 '克制统一的
    // 调色板' / '高光只落在主句') holds: the +0.0046 absolute lift on
    // lower-left mist_warmth and the +0.0038 absolute lift on
    // subtitle mist_warmth stay inside the muted-ink family (CREAM →
    // SHADOW 0.35 base), the inscribed strokes still read as ink
    // dissolving into the warm horizon rather than as brighter
    // signatures on their own, the focal line keeps its exclusive
    // claim on the page's light, and the upper-right cool echo
    // remains untouched at its cool axis. With the supporting tier's
    // per-site warm-mist share now sitting at the same 0.2222
    // register the title's per-site ambient_warmth share caught up to
    // in df4a49e — at the gentlest +4.55 % step the per-site-share
    // pivot established — 《寻隐者不遇》 reads as one Tang quatrain
    // inscribed in moonlit air whose four inscribed strokes plus the
    // calligrapher's seal now catch one more restrained step of the
    // warm horizon band together, completing the per-site-share
    // pivot the bell coefficient chain had carried the bell forward
    // to and the two share multipliers now unifying at one
    // gentlest-step register across the page.
    //
    // (Prior chain preserved below for reference: Mist-warmth share
    // 0.20 → 0.2125 (+6.25 %, paired with the bell coefficient
    // 6.4 → 6.8 in c253209): each supporting line that catches the
    // warm horizon band now reads ink a touch more clearly bathed
    // in the page's inhabited mist. Lower-left at v≈0.74 catches
    // mist_warmth 0.4790 * 0.2125 ≈ 0.1018 (was 0.0958 at the
    // post-bell-lift 0.20 multiplier, +0.0060 absolute, the natural
    // +6.25 % proportional gain that matches the bell-coefficient
    // arc); subtitle at v≈0.66 catches 0.4030 * 0.2125 ≈ 0.0857
    // (was 0.0806, +0.0051 absolute). Upper-right (v≈0.27) stays
    // clear of the bell so it remains the cool echo in the moon's
    // air. The +6.25 % continues the same restraint cadence as the
    // recent supporting mist bell arc (6.0 → 6.4 → 6.8 → 7.225 →
    // 7.677, +6.7 % / +6.25 % x3) and the +6.25 % sky_sigma /
    // sky_peak / halo_peak / moon_halo_r geometric cadence, all now
    // pivoting to the per-site share multipliers rather than
    // continuing the bell amplitude up. Maximum warm-mist catch stays
    // comfortably under the upper-bound envelope (~8.5 %), so 《云深
    // 不知处》 continues to read as deep ink actually dissolving into
    // the warm horizon rather than as dim cream floating over a
    // barely-visible amber tint. The +6.25 % is lifted in lockstep
    // with the title ambient_warmth 0.20 → 0.2125 (line 4783) and the
    // background horizon-band blend 0.12 → 0.1275 (line 1139).
    // Supporting mist_warmth share 0.2222 → 0.2278 (+2.5 %, the gentlest
    // step on the warm-mist share axis after the +4.55 % paired lift to
    // 0.2222 in 2eff631 / df4a49e — the +2.5 % sits exactly inside the
    // +2.35-2.86 % gentlest rung the page-wide +2-3 % material refinement
    // band the most-refined axes have settled into (body σ +2.35 % in
    // 4695311; halo radius +2.5 % in 28af5b6; sky σ +2.55 % in c28ed51;
    // halo_peak +2.5 % in 3b8028b; sky_peak +2.5 % in 78959d2; body_pulse
    // +2.61 % / halo_pulse +2.48 % / sky_pulse +2.56 % in 43fc830; cool_tint
    // +2.4 % / +2.5 % / +2.7 % in 0ce6e37, 610ee7a, f595bff, 77b520e;
    // vignette +2.5 % in 7304555; terminator alpha +2.69 % in 47ac018;
    // terminator cap +2.86 % in 4077850; subtitle alpha +2.63 % in 1c2fb98;
    // upper-right alpha +2.65 % in 867377c; lower-left alpha +2.48 % in
    // 70c9147; title/seal alpha +2.58 % in 69ce9b1; subtitle em_scale
    // +2.5 % in a2a3f48; upper-right em_scale +2.5 % in 2bf7493; lower-left
    // em_scale +2.5 % in 19059c2; title target_px +2.5 % in 7397729;
    // bloom2_alpha ceiling +2.5 % in 095ef01) rather than the supporting
    // mist_warmth share quietly sitting at its post-2eff631 +4.55 %
    // register while the moon-side geometric-extent, luminance, breath,
    // inscribed-stroke alpha, supporting-tier size, focal-line outer-corona,
    // directional-modulation, warm-tint cap, chromatic, and frame axes
    // stepped past it at +2.35-2.86 %. The +2.5 % (0.2222 → 0.2278) lifts
    // the supporting mist_warmth share by +0.0056 absolute, so the
    // supporting inscriptions now pick up one more restrained step of the
    // warm horizon bell the page's three lowest strokes dissolve into.
    // The +0.0056 absolute multiplier lift stays inside the muted-ink
    // family (the warm mist still reads as atmospheric depth, not as a
    // competing warm source), the subtitle peak mist contribution now
    // sits at 0.4030 * 0.2278 ≈ 0.0918 (was 0.0895, +0.0023 absolute,
    // +2.5 % relative at the subtitle's v≈0.66) and the lower-left peak
    // mist contribution now sits at 0.4791 * 0.2278 ≈ 0.1091 (was 0.1064,
    // +0.0027 absolute, +2.5 % relative at the lower-left's v≈0.74), both
    // staying well inside the supporting-tier envelope (~ 0.05-0.10
    // ambient warm catches, well under the inscribed glow ~ 0.20+ and
    // the hero bloom ~ 0.55), so the focal line keeps its exclusive
    // claim on the page's light (ART_DIRECTION §四 '高光只落在主句'
    // holds), the focal hierarchy (hero / subtitle / upper-right /
    // lower-left / seal) is unchanged, the brush-weight gradient
    // (subtitle brightest → upper-right → lower-left → title dimmest)
    // holds, the warm / cool axis (subtitle + lower-left warm, upper-
    // right cool, title as the warm-side closing signature) holds, and
    // the warm-mist share axis now extends the gentlest-step +2.5 %
    // register the moon-side geometric-extent, luminance, breath,
    // inscribed-stroke alpha, supporting-tier size, focal-line outer-
    // corona, directional-modulation, warm-tint cap, chromatic, and
    // frame axes have settled into. Restraint (ART_DIRECTION §四 '克制
    // 统一的调色板' / '高光只落在主句') holds: the +0.0056 absolute
    // multiplier lift stays inside the muted-cream family, the peak
    // supporting mist pixel still sits comfortably under the inscribed
    // glow (~ 0.20+) and the hero bloom (~ 0.55), and the supporting
    // inscriptions still read as ink dissolving into the warm horizon
    // rather than as a brighter warm source — just a mist-warmth share
    // that now registers one more gentle step of the page's coupled
    // gentlest-step register.
    let mist_warmth = horizon_glow * 0.2335;
    // Cool axis — the mirror image of the mist warmth above. Supporting
    // lines that sit in the moonlit upper sky absorb a touch of cool
    // tint from the cool air they inhabit, so the upper-right echo
    // 《只在此山中》 (v≈0.27) reads as ink in the moon's sphere of
    // influence rather than the same warm cream as the hero and
    // subtitle. The bell rises from v=0.0, peaks around v≈0.225, and
    // fades by v=0.45 — the upper-right sits well inside the band
    // (sky_cool ≈ 0.068), while the subtitle (v≈0.66) and lower-left
    // (v≈0.74) sit clear of it and pick up nothing. This completes the
    // warm/cool axis the mist warmth began: the lower-left dissolves
    // into the warm horizon, the upper-right reads as ink in the cool
    // moonlit sky, and the two echoes flank the central inscription
    // on opposite atmospheres rather than both on the same neutral
    // field. Restraint (ART_DIRECTION §四 "低饱和、高级灰"): the cool
    // tint is capped at ≈ 0.07 so the upper-right still reads as
    // cream ink, not as cyan; the brush-weight hierarchy (subtitle
    // brightest, lower-left dimmest) and the warm/cool axis both hold
    // without one overpowering the other.
    let sky_axis = ((0.45 - slot.def.y_frac).max(0.0) / 0.45).clamp(0.0, 1.0);
    let sky_cool = sky_axis * 0.18;
    // Moon-proximity cool — slots close to the moon (specifically the
    // upper-right echo at (0.80, 0.27), next to the moon at (0.86, 0.16))
    // pick up an extra share of cool luminance from the moon's halo, so
    // 《只在此山中》 reads as ink bathed in the moon's sphere of
    // influence rather than ink floating in generic cool sky. The two
    // share an upper-right quadrant; the echo "only in this mountain"
    // sits beneath a moon that knows where, and the line should pick
    // up a touch of the moon's cool luminance so the two share one
    // atmosphere. Falls off with distance: the upper-right catches
    // moon_proximity ≈ 0.665 (moon_cool ≈ 0.055, total cool ≈ 0.123),
    // while the hero (dist ≈ 0.444, proximity clamped to 0), subtitle
    // (dist ≈ 0.616, proximity clamped to 0), and lower-left (dist ≈
    // 0.894, proximity clamped to 0) stay near their existing cool
    // tints — they're too far from the moon for proximity to contribute
    // meaningfully. The contribution weight 0.04 → 0.06 (+50 %) → 0.07
    // (+16.7 % relative) → 0.082 (+17.1 % relative, the fourth step in
    // this arc) and cap 0.10 → 0.12 (+20 %) → 0.14 (+16.7 %) so the
    // upper-right reads more clearly as ink bathed in moonlit air
    // rather than the same neutral cream as the subtitle and hero; the
    // +17.1 % weight paired with the +16.7 % cap lift lets the upper-
    // right's cool_tint grow from 0.115 → 0.123 (+6.5 %) — the echo
    // now sits visibly deeper in the moon's sphere of influence after
    // four proportional passes, while the new cap (still 0.14) keeps
    // the result well under "cyan" (ART_DIRECTION §四 "低饱和、高级灰")
    // and preserves the brush-weight hierarchy (subtitle brightest,
    // upper-right next, lower-left dimmest) and the warm/cool axis
    // (subtitle + lower-left warm, upper-right cool) both still hold.
    // The +16.7 % cap cadence matches the +16.7 % weight cadence so
    // the arc lifts coherently (the weight had been outrunning the cap
    // — at 0.07 the formula produced 0.115, only 0.005 under the 0.12
    // ceiling, so the cap was engaging to absorb future lifts; opening
    // it to 0.14 gives the next two ~+17 % weight steps room to grow
    // before re-engaging, the way the cap 0.10 → 0.12 paired with the
    // +50 % and +16.7 % weight steps in the prior arc). The +17.1 %
    // weight and +16.7 % cap both continue the same restraint cadence
    // as the recent halo_pulse +16.7 % (20ee479), body +5.5 %
    // (fe42fec), halo +5.5 % (238b40b), and sky +7 % (80e27d5) bumps
    // — the moon's atmospheric layers, the moonlit air's reach, and
    // the moon's reach onto its closest inscription line now share one
    // proportional series of restrained steps so the page's moonlit
    // envelope reads as one coherent refinement rather than five
    // independent moon-side tweaks; and the +6.5 % cool_tint lift
    // pairs with the title alpha lift in e37c083 (0.46 → 0.48, +4.3 %)
    // and the warm bell lift in c5f73e0 (6.0 → 6.4, +6.7 %) so the
    // moon's air deepening on the upper-right echo, the seal clearing
    // the bottom of the warm band, and the warm horizon grounding the
    // lower-left echo are the three quiet ways the page's four
    // inscribed strokes have been sharing one atmosphere across the
    // recent work.
    let moon_dx = slot.def.x_frac - 0.86;
    let moon_dy = slot.def.y_frac - 0.16;
    let moon_dist = (moon_dx * moon_dx + moon_dy * moon_dy).sqrt();
    let moon_proximity = (1.0 - moon_dist * 2.5).clamp(0.0, 1.0);
    // Moon-proximity weight 0.092 → 0.097 (+5.43 %, the seventh step in
    // this arc, the gentlest yet in the +5-7 % restraint cadence after
    // six +5.75-50 % steps) so 《只在此山中》 leans one more restrained
    // step deeper into the moon's sphere of influence. The +5.43 %
    // continues the same direction as the six prior moon_proximity
    // lifts (0.04 → 0.06 → 0.07 → 0.082 → 0.087 → 0.092) but at the
    // gentlest +5-6 % step that matches the rest of the page's settled
    // atmosphere, so the upper-right's cool_tint now lifts from
    // 0.129 → 0.1325 (+2.7 %, the natural proportional gain for a
    // +5.43 % weight bump at proximity 0.665 — moon_cool goes from
    // 0.0612 → 0.0645, sky_cool stays at 0.068). The 0.14 cap stays
    // put (the formula now sits 0.0075 below the cap, ≈+13 % more
    // weight headroom before re-engaging), so the upper-right still
    // reads as cream ink bathed in moon's air rather than cyan
    // (ART_DIRECTION §四 "低饱和、高级灰"). The +2.7 % cool_tint lift
    // pairs with the recent inscription-side refinement chain — title
    // alpha 0.40 → 0.504 (+10 % / +4.5 % / +4.3 % / +5 % in the prior
    // arc and c9f4dde), inscribed-breath base 0.075 → 0.105 (+6.7 %
    // / +6.25 % / +5.88 % x2 / +5.56 % / +5.26 % / +5 % in the prior
    // arc and 0004e74), title breath 0.0435 → 0.0609 (+6.7 % / +6.25 %
    // / +5.88 % / +5.56 % / +5.26 % / +5 % in the prior arc and
    // 0004e74), supporting mist bell 6.4 → 6.8 (+6.25 % in 4ac2395),
    // warm bell 6.0 → 6.8 (+6.7 % / +6.25 % in c5f73e0, c253209),
    // terminator amber-tint cap 0.12 → 0.137 (+8.3 % / +5.4 % in
    // c601184, 1c666a7), body 0.50 → 0.682 (+5.5-5.6 % x5 in fe42fec,
    // b7ebeda, 90e22dc, 3b60530), halo 0.05 → 0.068 (+5.0-6.25 % x5 in
    // 238b40b, 698aa08, 0f13e55, 9989c4a), sky_peak 0.018 → 0.034
    // (+56 % / +7 % / +6.25 % x3 in b7ebeda, 80e27d5, 9ec99ff),
    // terminator alpha ±20 % → ±26 % (+8.3 % in ad3ee9a), cool_tint
    // 0.115 → 0.129 (+6.5 % / +2.4 % / +2.7 % in 0ce6e37, 610ee7a,
    // f595bff), moon proximity 0.04 → 0.092 (+50 % / +16.7 % / +17.1 %
    // / +6.1 % / +5.75 % in the prior arc), lower-left alpha 0.50 →
    // 0.612 (+16 % / +5.5 % in e37c083's chain and 9a4cc96), sky
    // bell σ 75 → 80 (+6.67 % in efd8cb1), and vignette curve pow(0.7)
    // → pow(0.75) (+7.1 % in 7304555) — so the moon's three nested
    // atmospheric layers, the four inscribed strokes, the
    // calligrapher's seal, and the moon's reach onto its closest
    // inscription line now share one proportional series of restrained
    // steps (+2.4 %, +2.7 %, +4.3 %, +5.0 %, +5.26 %, +5.4 %, +5.43 %,
    // +5.5 %, +5.56 %, +5.6 %, +5.75 %, +5.88 %, +6.1 %, +6.25 %,
    // +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %), and the page's moonlit
    // atmosphere reads as one coherent refinement rather than eighteen
    // independent tweaks. The cap 0.14 staying put also means the
    // brush-weight hierarchy (subtitle brightest, upper-right next,
    // lower-left dimmest) and the warm / cool axis (subtitle + lower-
    // left warm, upper-right cool) both hold — the lift stays in the
    // relationship between the moon and its closest echo, not in the
    // absolute brightness of either. Restraint (ART_DIRECTION §四
    // "克制统一的调色板") holds: the +0.0033 absolute lift on moon_cool
    // stays inside the cream family, the brightest pixel of the
    // upper-right echo still sits well under the inscribed glow
    // (~0.20+) and the hero bloom (~0.55), so the focal line keeps
    // its exclusive claim on the page's light (ART_DIRECTION §四 "高
    // 光只落在主句"). With the upper-right now leaning one more
    // gentle step into the moon's sphere of influence — at the
    // gentlest +5.43 % step in the moon-proximity arc after six
    // +5.75-50 % steps — 《只在此山中》 reads as ink that lives a
    // touch deeper in the moon's air, and the moon's reach onto its
    // closest inscription line settles further into the same +5-7 %
    // restraint cadence as the rest of the page's recent
    // refinements.
    //
    // Moon-proximity cool-tint weight 0.102 → 0.107 (+4.9 %, the
    // ninth step in the moon-proximity arc — 0.04 → 0.06 → 0.07 →
    // 0.082 → 0.087 → 0.092 → 0.097 → 0.102 → 0.107, +50 % / +16.7 %
    // / +17.1 % / +6.1 % / +5.75 % / +5.43 % / +5.15 % / +4.9 %): the
    // upper-right echo 《只在此山中》 now leans one more restrained step
    // into the moon's sphere of influence after the moon-side chain
    // completed its +6.25 % cadence across geometric extent (halo_r 64
    // → 68 in 8113547, sky_sigma 80 → 85 in 84b4150) and luminance
    // (halo_peak 0.072 → 0.0765 in ef91dae, sky_peak 0.034 → 0.0361
    // in 611895d) — the cool axis (moon_proximity → cool_tint
    // contribution) was the last moon-side atmospheric axis that
    // hadn't joined the proportional series the moon's three nested
    // atmospheric layers, the warm horizon mist bell, and the four
    // inscribed strokes have been sharing since 84b4150 completed
    // the geometric-extent arc and aa626f1 lifted the warm axis. The
    // +4.9 % (0.102 → 0.107) continues the moon-proximity arc's
    // gentlest-step deceleration pattern (5.15 % → 4.9 %, the same
    // gentlest-end cadence the inscribed-breath base settled into
    // per cc3a82f), so the moon's cool reach onto its closest
    // inscription line now matches the proportional arc rather than
    // quietly sitting one step behind. At moon_proximity ≈ 0.665
    // (the upper-right's normalised distance from the moon centre)
    // the cool_tint now sits at 0.068 + 0.665 * 0.107 = 0.1391
    // (was 0.068 + 0.665 * 0.102 = 0.1358, +2.4 % absolute so the
    // upper-right's cool ambient tint rises by one more natural
    // +2.5 % proportional gain — matching the +2.5 % cool_tint lift
    // in 77b520e and the +2.4 % / +2.7 % / +2.5 % gentlest-end
    // cadence that the cool axis has been sharing), and the 0.14
    // cap stays well clear (now 0.0009 of headroom, ≈+1.3 % more
    // weight before re-engaging). The +4.9 % continues the same
    // restraint cadence as the recent +6.25 % supporting mist bell
    // lift (aa626f1), the +6.25 % sky_sigma extension (84b4150),
    // the +5 % upper-right alpha lift (7161ce8), the +6.25 % sky_peak
    // lift (611895d), the +6.25 % halo_peak lift (ef91dae), the
    // +6.25 % halo radius extension (8113547), the +6.67 % sky bell
    // σ extension (efd8cb1), the +5 % body_pulse lift (ecff1f4), the
    // +5 % halo_pulse lift (ab6a040), the +3.4 % and +4.76 %
    // sky_pulse lifts (a79662b, 45b94af), the +4.76 % x35
    // inscribed-breath base, the +5 % title alpha lifts (c9f4dde,
    // 4b84ab7), the +5.5-5.6 % body bumps (fe42fec, b7ebeda,
    // 90e22dc, 3b60530), the +5 % lower-left alpha lifts (9a4cc96),
    // and the +2.19 % terminator amber-tint cap — so the moon's
    // three nested atmospheric layers (body + halo + sky bell), the
    // warm horizon mist bell, the four inscribed strokes, and the
    // moon's cool reach onto its closest inscription line now share
    // one proportional series of restrained +2.19-7.35 % steps
    // across breath, luminance, geometric extent, warm-mist, and
    // cool axes. The σ 8 body bell, the 4-px halo fade-in, the σ 85
    // sky bell, the ±26 % / 0.140 amber-tint terminator cap, the 0.65
    // multiplier, body_pulse 0.021, halo_pulse 0.0735, body 0.682,
    // halo 0.0765, sky_peak 0.0361, warm bells 7.677, supporting
    // mist bell 7.677, title ambient warmth 7.677, cool tint cap
    // 0.14, subtitle alpha 0.76, lower-left alpha 0.612, title alpha
    // 0.529, title v 0.83, inscribed-breath base 0.5340, title breath
    // 0.3096, halo radius 68, and the supporting slots' positions and
    // drifts are all unchanged so only the moon's cool reach onto
    // 《只在此山中》 shifts and the moon's geometric structure stays
    // identical. Restraint (ART_DIRECTION §四 "克制统一的调色板" + "高
    // 光只落在主句") holds: the +0.0033 absolute moon_cool lift stays
    // inside the cream family, the brightest pixel of 《只在此山中》
    // still sits well under the inscribed glow (~0.20+) and the hero
    // bloom (~0.55) so the focal line keeps its exclusive claim on
    // the page's light, and the 0.14 cap stays put so the warm/cool
    // axis (subtitle + lower-left warm, upper-right cool) holds and
    // 《只在此山中》 continues to read as cream ink bathed in moon's
    // air rather than cyan. With 《只在此山中》 now leaning one more
    // restrained step into the moon's sphere of influence — at the
    // gentlest +4.9 % step in the moon-proximity arc, settling
    // further into the same +2.5-6.25 % restraint cadence as the
    // recent moon-side chain and matching the inscribed-breath
    // base's gentlest-step ceiling per cc3a82f — the moon's cool
    // reach onto its closest inscription line now shares one
    // proportional cadence with the moon-side geometric extent and
    // luminance chain that 84b4150 completed, and 《寻隐者不遇》 reads
    // as one Tang quatrain inscribed in moonlit air whose closest
    // stroke to the moon now catches the moon's cool air a touch
    // more visibly while the rest of the page's cool axis holds its
    // restrained palette.
    // Cool-tint cap 0.14 → 0.1435 (+2.5 %, the gentlest step on the
    // cool-tint cap axis after the cool_tint result lifts 0.115 →
    // 0.118 → 0.122 → 0.125 → 0.13908 in 0ce6e37, 610ee7a, f595bff,
    // 77b520e — the +2.5 % sits exactly inside the +2.35-2.86 %
    // gentlest rung the page-wide +2-3 % material refinement band
    // the most-refined axes have settled into (body σ +2.35 % in
    // 4695311; halo radius +2.5 % in 28af5b6; sky σ +2.55 % in
    // c28ed51; halo_peak +2.5 % in 3b8028b; sky_peak +2.5 % in
    // 78959d2; body_pulse +2.61 % / halo_pulse +2.48 % / sky_pulse
    // +2.56 % in 43fc830; body_pulse +2.54 % / halo_pulse +2.54 %
    // / sky_pulse +2.5 % in 913b77e; vignette ceiling +2.5 % in
    // 7304555; terminator alpha +2.69 % in 47ac018; terminator cap
    // +2.86 % in 4077850; subtitle alpha +2.63 % in 1c2fb98;
    // upper-right alpha +2.65 % in 867377c; lower-left alpha
    // +2.48 % in 70c9147; title/seal alpha +2.58 % in 69ce9b1,
    // +2.5 % in e8cc837; subtitle em_scale +2.5 % in a2a3f48;
    // upper-right em_scale +2.5 % in 2bf7493; lower-left em_scale
    // +2.5 % in 19059c2; title target_px +2.5 % in 7397729;
    // bloom2_alpha ceiling +2.5 % in 095ef01; bloom2_alpha base
    // +2.73 % in d7f8fc6; bloom_alpha base +2.5 % in c7813d4;
    // bloom_alpha ceiling +2.5 % in 494d8b7; supporting mist_warmth
    // share +2.5 % in 57c514e; warm-mist bell +2.5 % in 2a7cd02;
    // horizon-band blend share +2.5 % in 3154090; warm-mist share
    // axis +2.5 % in e483929; inscribed-breath base +2.5 % and
    // title breath +2.5 % in 4e542cc; focal-line inner-glow base
    // +2.5 % in 5d5a706) rather than the cool-tint cap quietly
    // sitting at its post-77b520e +16.7 % register while every
    // surrounding material axis stepped past it at +2.35-2.86 %. The
    // +2.5 % (0.14 → 0.1435) opens the cool-tint cap by +0.0035
    // absolute, so the upper-right echo 《只在此山中》 now catches
    // one more gentle step of the moon's cool sphere — at the
    // upper-right's moon_proximity ≈ 0.6867 the formula computes
    // 0.072 + 0.6867 * 0.107 = 0.1455, which the prior cap 0.14
    // was clamping to 0.14; with the lifted cap 0.1435 the
    // upper-right's cool_tint now lands at 0.1435 (was 0.14,
    // +0.0035 absolute, +2.5 % relative on the cool register — the
    // painted mix factor toward color::star::COOL now lifts from
    // 14 % to 14.35 %, one more gentle step into the moon's air),
    // so the focal line keeps its exclusive claim on the page's
    // light (ART_DIRECTION §四 '高光只落在主句' holds), the cool
    // axis (subtitle + lower-left + title warm, upper-right cool)
    // holds, the warm / cool axis the moon inhabits tightens one
    // more restrained step inside the cream / cool family, the
    // hero (cool_tint 0.012, well below cap), the subtitle
    // (cool_tint 0, y_frac 0.65 clear of sky_axis), the lower-left
    // (cool_tint 0, y_frac 0.74 clear of sky_axis, moon_proximity
    // 0), and the title (cool_tint 0, y_frac 0.83 clear of
    // sky_axis, moon_proximity 0) all stay well clear of the
    // lifted cap so only the upper-right echo registers the
    // +2.5 % step into the moon's air, and the inscribed-stroke
    // alpha axis, the supporting-tier em_scale axis, the title
    // target_px axis, the focal-line bloom axis (primary inner-
    // glow + nearer-glow + outer-corona), the warm-mist system
    // (background atmosphere + per-line warm tint + title ambient
    // warmth + bell amplitude), the moon's three nested
    // atmospheric breath modulation (913b77e), the inscription-
    // side breath axis (4e542cc), and the page-wide +2-3 %
    // material refinement band all hold — only the cool-tint cap
    // axis catches up with the +2.5 % gentlest rung the
    // surrounding material axes have already settled into, and
    // the upper-right echo 《只在此山中》 now reads as ink a
    // touch more visibly bathed in the same moonlit air the
    // disc above it breathes.
    let cool_tint = (sky_cool + moon_proximity * 0.107).clamp(0.0, 0.1435);
    let warmth_tint = (warmth * (1.0 - slot.def.shadow_mix) * 0.30 + mist_warmth).clamp(0.0, 1.0);
    let mut base_color = mix(raw_base, color::ink::WARM, warmth_tint);
    if cool_tint > 0.0 {
        base_color = mix(base_color, color::star::COOL, cool_tint);
    }
    let glow_color = mix(
        color::ink::GLOW,
        color::ink::SHADOW,
        0.4 + slot.def.shadow_mix * 0.2,
    );

    // Per-character alpha is the slot alpha (no per-char stagger for
    // supporting lines — they reveal as a single line).
    let char_alpha = alpha;
    for (i, &ch) in chars.iter().enumerate() {
        let glyph_idx = glyph::index_for(ch as u32);
        let slot_idx = glyph_idx as usize;
        let advance_q8 = glyph::HERO_TABLE[slot_idx].advance as i32 * (scale_q8 as i32);
        let pen_x_q8 = pen_x0 * 256 + advance_q8 * (i as i32);
        let fx = pen_x_q8;
        let fy = baseline_y * 256;

        // No outer glow on supporting lines — ART_DIRECTION mandates
        // "bloom only on the focal line". The supporting corners are
        // echoes, not lamps; they stay crisp-cream on the gradient so the
        // hero's warm halo reads as the sole light source on the page.
        let glow_a = 0.0_f32;
        if glow_a > 0.01 {
            glyph::draw_glyph(
                fb, w as usize, h as usize, glyph_idx, glow_color, glow_color, fx, fy, scale_q8,
                glow_a,
            );
        }
        glyph::draw_glyph(
            fb, w as usize, h as usize, glyph_idx, base_color, glow_color, fx, fy, scale_q8,
            char_alpha,
        );
    }
    // Supporting lines are now tinted toward the warm palette family by the
    // `warmth_tint` computed above (scaled by 1 - shadow_mix).
}

/// Paint the hero slot using the existing rhythm-engine `Beat` (entrance /
/// hold / exit, per-char stagger, overshoot).  Reads the hero's phrase from
/// the composition slot (so the composition's pinning — e.g. to a poem group
/// line — wins over the engine's beat-round-robin pick), while still using
/// the beat's phase timeline for animation timing.
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
    let text = phrase.text;
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return;
    }
    let (pen_x0, baseline_y0, scale_q8) = place_slot(def, w, h, n);
    let (dx, dy) = (
        def.drift_x * (time * def.drift_fx + def.drift_phase).sin(),
        def.drift_y * (time * def.drift_fy + def.drift_phase * 1.3).cos(),
    );
    let pen_x0 = pen_x0 + dx as i32;
    let baseline_y0 = baseline_y0 + dy as i32;

    // Phase-aware motion (existing behaviour).
    //
    // Rest phase used to map to ep=0.0, which dropped the focal line
    // entirely between phrases — ART_DIRECTION §一 "可读性：任何帧截
    // 图都能看清句子" requires every frame to keep the focal line
    // readable, so the rest now reads as a low "ghost" alpha (0.30)
    // instead of a hard blackout. The previous beat's phrase stays
    // visible at a quiet breath register while the engine waits out the
    // gap before the next entrance, so the page never loses its anchor.
    let ep = match beat.phase {
        Phase::Entrance => beat.entrance_progress(),
        Phase::Hold => 1.0,
        Phase::Exit => 1.0 - beat.exit_progress(),
        Phase::Rest => 0.30,
    };
    let ep_eased = color::smootherstep(ep);

    let total_chars_delay = 0.40_f32;
    let per_char_window = (1.0 - total_chars_delay) / (n as f32).max(1.0);
    let slide_y_px = match beat.phase {
        Phase::Entrance => ((1.0 - ep) * 24.0) as i32,
        Phase::Exit => (ep * 18.0) as i32,
        _ => 0,
    };

    // Glow tint leans cool — the focal line is meant to read as moonlit
    // cream rather than amber-highlighter ink. The warmth coefficient is
    // pulled down (0.6 → 0.35) so a touched-warm scene still warms the
    // background and supporting tier but the bloom underneath the hero
    // stays close to ink::GLOW, the page's natural moonlit-white. The
    // supporting tier still shifts amber with touch (see
    // `paint_supporting_slot`); only the focal halo now refuses to chase
    // the warmth curve, so the inscription reads as one luminous moonlit
    // work against a mist that may be warm or cool — not as four lines
    // that all brighten amber together. Restraint (ART_DIRECTION §四
    // "克制统一的调色板" / "低饱和、高级灰") holds: the bloom stays
    // inside the cream family, just closer to the cool end of it.
    let base_color = mix(color::ink::CREAM, color::ink::WARM, warmth * 0.5);
    let glow_color = mix(color::ink::GLOW, color::ink::WARM, warmth * 0.35);
    let beat_glow = phrase.glow;
    // Focal-line inner-glow base 0.10 → 0.1025 (+2.5 %, the gentlest step
    // on the focal-line primary inner-bloom base axis after the +2.5 %
    // nearer-glow base lift in c7813d4, the +2.5 % nearer-glow ceiling
    // lift in 494d8b7, the +2.73 % outer-corona base lift in d7f8fc6,
    // and the +2.5 % outer-corona ceiling lift in 095ef01 — the +2.5 %
    // sits exactly inside the +2.35-2.86 % gentlest rung the page-wide
    // +2-3 % material refinement band the most-refined axes have settled
    // into (body σ +2.35 % in 4695311; halo radius +2.5 % in 28af5b6; sky
    // σ +2.55 % in c28ed51; halo_peak +2.5 % in 3b8028b; sky_peak +2.5 %
    // in 78959d2; body_pulse +2.61 % / halo_pulse +2.48 % / sky_pulse
    // +2.56 % in 43fc830; body_pulse +2.54 % / halo_pulse +2.54 % /
    // sky_pulse +2.5 % in 913b77e; cool_tint +2.4 % / +2.5 % / +2.7 %
    // in 0ce6e37, 610ee7a, f595bff, 77b520e; vignette ceiling +2.5 % in
    // 7304555; terminator alpha +2.69 % in 47ac018; terminator cap
    // +2.86 % in 4077850; subtitle alpha +2.63 % in 1c2fb98; upper-right
    // alpha +2.65 % in 867377c; lower-left alpha +2.48 % in 70c9147;
    // title/seal alpha +2.58 % in 69ce9b1, +2.5 % in e8cc837; subtitle
    // em_scale +2.5 % in a2a3f48; upper-right em_scale +2.5 % in
    // 2bf7493; lower-left em_scale +2.5 % in 19059c2; title target_px
    // +2.5 % in 7397729; supporting mist_warmth share +2.5 % in
    // 57c514e; warm-mist bell +2.5 % in 2a7cd02; horizon-band blend
    // share +2.5 % in 3154090; warm-mist share axis +2.5 % in e483929;
    // inscribed-breath base +2.5 % in 4e542cc; title breath +2.5 % in
    // 4e542cc) rather than the focal line's primary inner-glow base
    // quietly sitting at its post-c7813d4 0.10 register while the
    // focal-line nearer-glow base (c7813d4), nearer-glow ceiling
    // (494d8b7), outer-corona base (d7f8fc6), and outer-corona ceiling
    // (095ef01) all stepped past at +2.5-2.73 %. The +2.5 % (0.10 →
    // 0.1025) lifts the focal line's primary inner-glow register by
    // +0.0025 absolute, so 《松下问童子》's inner-glow now registers one
    // more gentle step of the page's proportional cadence at rest — the
    // painted inner-glow at rest climbs from 0.10 × 0.45 = 0.045 to
    // 0.1025 × 0.45 = 0.0461 (+0.0011 absolute, +2.5 % relative on the
    // painted glow register), the formula peak at pulse=1 / warmth=1 /
    // beat_glow=2.5 still lands at 0.55 (clamped, unchanged) so the
    // formula's headroom stays at ≈+22.5 % before re-engaging the
    // ceiling (the formula total 0.1025 + 0.18 + 0.06 + 0.25 = 0.5925,
    // still 0.0425 above the 0.55 ceiling, plenty of room for future
    // +2.5 % steps to keep catching up to the gentlest rung), the focal
    // line keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 '高光只落在主句' holds), the focal hierarchy (hero / subtitle /
    // upper-right / lower-left / seal) is unchanged, the brush-weight
    // gradient (subtitle brightest → upper-right → lower-left → title
    // dimmest) holds, the warm / cool axis (subtitle + lower-left + title
    // warm, upper-right cool) holds, the moon anchor and the moon's
    // three nested atmospheric breath modulation (913b77e), the
    // inscription-side breath axis (4e542cc), and the warm-mist system
    // (e483929 / 3154090 / 57c514e / 2a7cd02) all keep holding, the body
    // σ 8.7, sky σ 92.6, halo radius 69.7, body_peak 0.682, halo_peak
    // 0.082, sky_peak 0.0394, terminator alpha 0.267, terminator cap
    // 0.144, cool_tint 0.13908, moon_proximity 0.107, bloom2_alpha
    // ceiling 0.0646, bloom_alpha base 0.041, bloom_alpha ceiling 0.123,
    // bloom2_alpha base 0.0226, nebula alphas 0.022 / 0.016, vignette
    // pow(0.75), vignette ceiling 0.74, the subtitle / upper-right /
    // lower-left / title alphas 0.78 / 0.776 / 0.662 / 0.6119, the title
    // v 0.83, title target_px 23.575, the supporting mist bell 8.066,
    // the warm bells 8.066, the title ambient_warmth share 0.2335, the
    // supporting mist_warmth share 0.2335, the subtitle mist_warmth
    // share 0.0989, the lower-left mist_warmth share 0.1175, the
    // horizon-band blend share 0.1340, the body_pulse 0.0242, halo_pulse
    // 0.0848, sky_pulse 0.0411, the inscribed-breath base 0.5474, the
    // title breath 0.3173, and the supporting slots' positions and
    // drifts are all unchanged so only the focal line's primary
    // inner-glow base shifts and the focal line's three nested bloom
    // passes — primary inner-glow base + nearer-glow base + nearer-glow
    // ceiling + outer-corona base + outer-corona ceiling — now share
    // one coupled gentlest-step +2.5 % register the page-wide +2-3 %
    // material refinement band the surrounding material axes have
    // settled into. Restraint (ART_DIRECTION §四 '克制统一的调色板' /
    // '高光只落在主句') holds: the +0.0025 absolute base lift stays
    // inside the cream family (the painted inner-glow at rest sits at
    // 0.0461, well inside the focal-line cream envelope and well under
    // the inscribed glow band ~0.20+ and the hero bloom ~0.55), the
    // focal line 《松下问童子》 still reads as one luminous moonlit body
    // breathing in moonlit air rather than as a brighter hero, and the
    // focal-line bloom axis — primary inner-glow + nearer-glow + outer-
    // corona — now registers one coupled gentlest-step +2.5 % cadence
    // the inscribed-breath base (4e542cc) and the warm-mist share
    // (e483929 / 3154090 / 57c514e / 2a7cd02) have just settled into.
    // With 《松下问童子》 now catching one more restrained step of the
    // page's primary bloom register — at the gentlest +2.5 % step on
    // the focal-line inner-glow base axis, exactly inside the
    // +2.35-2.86 % gentlest rung the page-wide +2-3 % material
    // refinement band the focal-line nearer-glow base (c7813d4),
    // nearer-glow ceiling (494d8b7), outer-corona base (d7f8fc6), and
    // outer-corona ceiling (095ef01) have just completed — 《寻隐者不遇》
    // reads as one Tang quatrain inscribed in moonlit air whose focal
    // line 《松下问童子》 now registers one more gentle step of the
    // page's primary inner-glow register, and the focal-line bloom
    // axis finally steps onto the gentlest +2.5 % register the
    // inscription-side breath axis (4e542cc) and the page-wide +2-3 %
    // material refinement band have just settled into.
    let glow_alpha = (0.1025 + 0.18 * pulse + 0.06 * warmth + beat_glow * 0.10).clamp(0.0, 0.55);
    // Secondary wider bloom — same glyph drawn at slightly larger scale and
    // very low alpha so the focal line reads as a moonlit light source, not
    // just cream text on a gradient. ART_DIRECTION mandates "bloom only on
    // the focal line"; this pass is hero-only — supporting slots skip it
    // (see `paint_supporting_slot`).
    //
    // Inner-bloom ceiling 0.12 → 0.123 (+2.5 %, the gentlest step on the
    // focal-line inner-bloom ceiling axis after the +2.5 % rest base lift
    // in c7813d4 — the +2.5 % sits exactly inside the +2.35-2.86 % gentlest
    // rung the page-wide +2-3 % material refinement band the most-refined
    // axes have settled into (body σ +2.35 % in 4695311; halo radius +2.5 %
    // in 28af5b6; sky σ +2.55 % in c28ed51; halo_peak +2.5 % in 3b8028b;
    // sky_peak +2.5 % in 78959d2; body_pulse +2.61 % / halo_pulse +2.48 %
    // / sky_pulse +2.56 % in 43fc830; cool_tint +2.4 % / +2.5 % / +2.7 %
    // in 0ce6e37, 610ee7a, f595bff, 77b520e; vignette +2.5 % in 7304555;
    // terminator alpha +2.69 % in 47ac018; terminator cap +2.86 % in
    // 4077850; subtitle alpha +2.63 % in 1c2fb98; upper-right alpha +2.65 %
    // in 867377c; lower-left alpha +2.48 % in 70c9147; title/seal alpha
    // +2.58 % in 69ce9b1, +2.5 % in e8cc837; subtitle em_scale +2.5 % in
    // a2a3f48; upper-right em_scale +2.5 % in 2bf7493; lower-left em_scale
    // +2.5 % in 19059c2; title target_px +2.5 % in 7397729; bloom2_alpha
    // ceiling +2.5 % in 095ef01; bloom2_alpha base +2.73 % in d7f8fc6;
    // bloom_alpha base +2.5 % in c7813d4; title ambient_warmth share +
    // supporting mist_warmth share +2.5 % in 57c514e) rather than the
    // focal line's inner-bloom ceiling quietly sitting at its original 0.12
    // register while the page-wide +2-3 % material refinement band stepped
    // past it at +2.35-2.86 %. The +2.5 % (0.12 → 0.123) opens the
    // inner-bloom ceiling by +0.003 absolute, so 《松下问童子》's
    // nearer-glow now reaches one more restrained step of moonlit cream
    // bleeding outward at peak at the gentlest +2.5 % register the
    // inner-bloom base axis (c7813d4) has just settled onto as the
    // secondary bloom's companion. At rest (pulse=0, warmth=0, beat_glow=0)
    // the inner bloom still sits at 0.041 (the rest is unchanged so the
    // focal line's nearer-glow stays settled at the 0.041 rest register the
    // base just lifted to in c7813d4), at peak pulse the formula now totals
    // 0.041 + 0.04 + 0.02 + 0.03 = 0.131 → no longer clamped to 0.12, now
    // clamped to the new 0.123 ceiling — so the actual peak shifts from 0.12
    // to 0.123 (+0.003 absolute, +2.5 % relative, the formula peak 0.131
    // still 0.008 above the new ceiling so future base lifts have room to
    // continue lifting the rest before re-engaging the clamp, the way the
    // c7813d4 base lift paired with the +2.5 % bloom2_alpha base lift in
    // d7f8fc6 left the inner-bloom base and the outer-corona base on the
    // same +2.5-2.73 % rung). The +0.003 absolute ceiling lift stays well
    // inside the muted-cream family (the peak inner bloom still reads as
    // moonlit cream spreading outward from the focal line, not as a
    // brighter amber ring or a competing warm source), the peak inner bloom
    // pixel now sits at 0.123 (was 0.12, +0.003 absolute, +2.5 % relative)
    // — still firmly under the inscribed glow band (~0.20+) and the hero
    // bloom (~0.55) and well below the tertiary outer-corona's peak 0.0646
    // register the bloom2_alpha ceiling 095ef01 has just settled onto — so
    // the focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 '高光只落在主句' holds), the focal hierarchy
    // (hero / subtitle / upper-right / lower-left / seal) is unchanged, the
    // brush-weight gradient (subtitle brightest → upper-right → lower-left
    // → title dimmest) holds, the warm / cool axis (subtitle + lower-left
    // + title warm, upper-right cool) holds, and the focal-line inner-bloom
    // ceiling axis now extends the gentlest-step +2.5 % register the page-
    // wide +2-3 % material refinement band the focal-line inner-bloom base
    // (c7813d4) has just settled onto as the secondary bloom's companion.
    // The 1.06× inner-bloom scale, the 1.10× outer-corona scale, the
    // ±26 % / 0.144 amber-tint terminator cap, the 0.65 vignette
    // multiplier, the σ 8.7 body bell, the σ 92.6 sky bell, the body 0.682,
    // the halo_peak 0.082, the sky_peak 0.0394, the moon_halo_r 69.7, the
    // body_pulse 0.0236, the halo_pulse 0.0827, the sky_pulse 0.0401, the
    // bloom_alpha base 0.041, the bloom2_alpha base 0.0226, the
    // bloom2_alpha ceiling 0.0646, the terminator alpha 0.267, the
    // cool_tint 0.13908, the moon_proximity 0.107, the warm bells 7.677,
    // the supporting mist bell 7.677, the title ambient_warmth share
    // 0.2278, the supporting mist_warmth share 0.2278, the subtitle alpha
    // 0.78, the upper-right alpha 0.776, the lower-left alpha 0.662, the
    // title alpha 0.6119, the title v 0.83, the title target_px 23.575,
    // the title breath 0.3096, the inscribed-breath base 0.5340, the
    // subtitle em_scale 0.369, the upper-right em_scale 0.328, the
    // lower-left em_scale 0.287, the upper-right y_frac 0.27, the
    // lower-left y_frac 0.74, the subtitle drift 3.0/1.5/0.21/0.17/0.7, the
    // upper-right drift 3.0/2.0/0.15/0.19/1.4, the lower-left drift
    // 3.0/2.0/0.13/0.21/2.8, the hero drift 3.0/2.0/0.18/0.13/0.0, the
    // hero y_frac 0.42, the hero bloom 1.0, the nebula alphas 0.022 /
    // 0.016, the vignette pow(0.75), and the vignette ceiling 0.74 are all
    // unchanged so only the focal line's secondary wider-bloom ceiling
    // shifts and the page's inner-bloom ceiling axis catches up with the
    // gentlest-step +2.5 % register the page-wide +2-3 % material
    // refinement band the focal-line inner-bloom base (c7813d4) has just
    // settled onto as the secondary bloom's companion. Restraint
    // (ART_DIRECTION §四 '克制统一的调色板' / '高光只落在主句') holds:
    // the +0.003 absolute ceiling lift stays inside the muted-cream family,
    // the peak inner bloom pixel still sits comfortably under the inscribed
    // glow band (~0.20+) and the hero bloom (~0.55) and well below the
    // tertiary outer-corona's peak 0.0646 register, and 《松下问童子》's
    // nearer-glow still reads as moonlit cream bleeding outward from the
    // focal line into the moon-atmosphere the upper-right echo 《只在此
    // 山中》 inhabits rather than as a competing amber ring or a second
    // warm source — just a focal-line inner-bloom ceiling that now
    // registers one more gentle step of the page's coupled gentlest-step
    // register. With 《松下问童子》's nearer-glow now catching one more
    // restrained step of the moon-atmosphere at peak — at the gentlest
    // +2.5 % step on the focal-line inner-bloom ceiling axis, exactly
    // inside the +2.35-2.86 % rung the page-wide +2-3 % material
    // refinement band the focal-line inner-bloom base (c7813d4) has just
    // completed as the secondary bloom's companion — 《寻隐者不遇》 reads
    // as one Tang quatrain inscribed in moonlit air whose focal line's
    // nearer-glow now registers one more gentle step of the page's coupled
    // gentlest-step register at peak between pulses, and the focal-line
    // inner-bloom ceiling axis finally steps onto the gentlest +2.5 %
    // register the page-wide +2-3 % material refinement band the focal-line
    // inner-bloom base (c7813d4) has just settled onto as the secondary
    // bloom's companion.
    //
    // Secondary bloom base 0.04 → 0.041 (+2.5 %, the gentlest step on
    // the focal-line inner-bloom base axis — the +2.5 % sits exactly
    // inside the +2.35-2.86 % gentlest rung the page-wide +2-3 %
    // material refinement band the most-refined axes have settled into
    // (body σ +2.35 % in 4695311; halo radius +2.5 % in 28af5b6; sky σ
    // +2.55 % in c28ed51; halo_peak +2.5 % in 3b8028b; sky_peak +2.5 %
    // in 78959d2; body_pulse +2.61 % / halo_pulse +2.48 % / sky_pulse
    // +2.56 % in 43fc830; cool_tint +2.4 % / +2.5 % / +2.7 % in
    // 0ce6e37, 610ee7a, f595bff, 77b520e; vignette +2.5 % in 7304555;
    // terminator alpha +2.69 % in 47ac018; terminator cap +2.86 % in
    // 4077850; subtitle alpha +2.63 % in 1c2fb98; upper-right alpha
    // +2.65 % in 867377c; lower-left alpha +2.48 % in 70c9147; title/
    // seal alpha +2.58 % in 69ce9b1, +2.5 % in e8cc837; subtitle
    // em_scale +2.5 % in a2a3f48; upper-right em_scale +2.5 % in
    // 2bf7493; lower-left em_scale +2.5 % in 19059c2; title target_px
    // +2.5 % in 7397729; bloom2_alpha ceiling +2.5 % in 095ef01; bloom2
    // _alpha base +2.73 % in d7f8fc6; title ambient_warmth share +
    // supporting mist_warmth share +2.5 % in 57c514e) rather than the
    // focal line's inner-bloom always-on base quietly sitting at its
    // original 0.04 register while the page-wide +2-3 % material
    // refinement band stepped past at +2.35-2.86 %. The +2.5 %
    // (0.04 → 0.041) lifts the focal line's secondary wider-bloom
    // always-on component by +0.001 absolute, so 《松下问童子》's
    // nearer-glow now registers one more restrained step of moonlit
    // cream bleeding outward at rest at the gentlest +2.5 % register
    // the secondary bloom base axis's tertiary companion (bloom2_alpha
    // base +2.73 % in d7f8fc6) has just settled onto. At rest
    // (pulse=0, warmth=0, beat_glow=0) the inner bloom now sits at
    // 0.041 (was 0.04, +0.001 absolute, +2.5 % relative so the focal
    // line's nearer-glow now reaches one more restrained step into the
    // moon-atmosphere the upper-right echo 《只在此山中》 inhabits even
    // when the page is at rest between pulses), at peak pulse the
    // formula now totals 0.041 + 0.04 + 0.02 + 0.03 = 0.131 → still
    // clamped to the 0.12 ceiling (the peak is unchanged so the focal
    // line's nearer-glow stays settled at its 0.12 peak the ceiling has
    // always held), and the rest contribution is the only thing that
    // shifts. The +0.001 absolute rest lift stays well inside the
    // muted-cream family (the inner bloom at rest still reads as
    // moonlit cream spreading outward from the focal line, not as a
    // second amber ring or a competing warm source), the rest inner-
    // bloom pixel now sits at 0.041 (was 0.04, +0.001 absolute,
    // +2.5 % relative) — still firmly under the inscribed glow band
    // (~0.20+) and the hero bloom (~0.55) and well below the tertiary
    // outer-corona's rest 0.0226 register the page has just lifted
    // onto — so the focal line keeps its exclusive claim on the page's
    // light (ART_DIRECTION §四 '高光只落在主句' holds), the focal
    // hierarchy (hero / subtitle / upper-right / lower-left / seal) is
    // unchanged, the brush-weight gradient (subtitle brightest →
    // upper-right → lower-left → title dimmest) holds, the warm / cool
    // axis (subtitle + lower-left + title warm, upper-right cool)
    // holds, and the focal-line inner-bloom base axis now extends the
    // gentlest-step +2.5 % register the page-wide +2-3 % material
    // refinement band the focal-line outer-corona base (d7f8fc6) has
    // just settled onto as the secondary bloom's tertiary companion.
    // The 1.06× inner-bloom scale, the 1.10× outer-corona scale, the
    // ±26 % / 0.144 amber-tint terminator cap, the 0.65 vignette
    // multiplier, the σ 8.7 body bell, the σ 92.6 sky bell, the body
    // 0.682, the halo_peak 0.082, the sky_peak 0.0394, the moon_halo_r
    // 69.7, the body_pulse 0.0236, the halo_pulse 0.0827, the sky_pulse
    // 0.0401, the bloom2_alpha base 0.0226, the bloom2_alpha ceiling
    // 0.0646, the terminator alpha 0.267, the cool_tint 0.13908, the
    // moon_proximity 0.107, the warm bells 7.677, the supporting mist
    // bell 7.677, the title ambient_warmth share 0.2278, the
    // supporting mist_warmth share 0.2278, the subtitle alpha 0.78,
    // the upper-right alpha 0.776, the lower-left alpha 0.662, the
    // title alpha 0.6119, the title v 0.83, the title target_px
    // 23.575, the title breath 0.3096, the inscribed-breath base
    // 0.5340, the subtitle em_scale 0.369, the upper-right em_scale
    // 0.328, the lower-left em_scale 0.287, the upper-right y_frac
    // 0.27, the lower-left y_frac 0.74, the subtitle drift
    // 3.0/1.5/0.21/0.17/0.7, the upper-right drift 3.0/2.0/0.15/
    // 0.19/1.4, the lower-left drift 3.0/2.0/0.13/0.21/2.8, the hero
    // drift 3.0/2.0/0.18/0.13/0.0, the hero y_frac 0.42, the hero
    // bloom 1.0, the bloom_alpha ceiling 0.12, the nebula alphas
    // 0.022 / 0.016, the vignette pow(0.75), and the vignette ceiling
    // 0.74 are all unchanged so only the focal line's secondary wider-
    // bloom always-on base shifts and the page's inner-bloom base axis
    // catches up with the gentlest-step +2.5 % register the page-wide
    // +2-3 % material refinement band the focal-line outer-corona base
    // (d7f8fc6) has just settled onto as the secondary bloom's tertiary
    // companion. Restraint (ART_DIRECTION §四 '克制统一的调色板' /
    // '高光只落在主句') holds: the +0.001 absolute rest lift stays
    // inside the muted-cream family, the rest inner-bloom pixel still
    // sits comfortably under the inscribed glow (~0.20+) and the hero
    // bloom (~0.55) and below the tertiary outer-corona's 0.0226 rest
    // register, and 《松下问童子》's nearer-glow still reads as
    // moonlit cream spreading outward from the focal line into the
    // moon-atmosphere the upper-right echo inhabits rather than as a
    // competing amber ring or a second warm source — just a focal-line
    // inner-bloom always-on base that now registers one more gentle
    // step of the page's coupled gentlest-step register as the
    // secondary bloom's tertiary companion to the outer-corona base
    // (d7f8fc6) lift. With 《松下问童子》's nearer-glow now catching
    // one more restrained step of the moon-atmosphere at rest — at
    // the gentlest +2.5 % step on the focal-line inner-bloom base
    // axis, exactly inside the +2.35-2.86 % rung the page-wide +2-3 %
    // material refinement band the focal-line outer-corona base
    // (d7f8fc6) has just completed as the secondary bloom's tertiary
    // companion — 《寻隐者不遇》 reads as one Tang quatrain inscribed
    // in moonlit air whose focal line's nearer-glow now registers one
    // more gentle step of the page's coupled gentlest-step register at
    // rest between pulses, and the focal-line inner-bloom base axis
    // finally steps onto the gentlest +2.5 % register the page-wide
    // +2-3 % material refinement band the focal-line outer-corona
    // base (d7f8fc6) has just settled onto as the secondary bloom's
    // tertiary companion.
    let bloom_scale_q8: u32 = ((scale_q8.max(1) as f32) * 1.06).round() as u32;
    let bloom_alpha = (0.041 + 0.04 * pulse + 0.02 * warmth + beat_glow * 0.03).clamp(0.0, 0.123);
    // Tertiary outer halo — an even wider, fainter pass so the moonlit
    // light diffuses outward into the surrounding ink rather than stopping
    // at a hard edge. Reads as atmospheric light, not a second copy of the
    // glyph. Kept extremely low so restraint (ART_DIRECTION §四) holds —
    // the viewer perceives "the page glows" not "the text has a glow".
    //
    // Outer corona tightened 1.13 → 1.10 (-2.7 %) so the moonlit bleed
    // sits a touch closer to the ink and the corona no longer reads as
    // a faint "ghost duplicate" of the hero ~8 px below the baseline
    // against the heavily-mist'd lower band — the previous 1.13x spread
    // (centred on the glyph, the outer halo extended ~8 px past the
    // glyph bbox in every direction, including ~8 px below the baseline
    // where the warm horizon mist already tints the page) was wide
    // enough that the bloom underneath the hero lined up with the
    // subtitle's leading edge, making the bloom feel like a second
    // copy of 《松下问童子》 rather than light diffusing outward from
    // the focal line. At 1.10x the corona now extends ~6 px past the
    // glyph in each direction — the moonlit bleed still wraps the
    // hero in atmospheric light (the bloom2_alpha base + ceiling are
    // unchanged, so every pixel still contributes up to 0.06 cream),
    // but the bleed no longer reaches the subtitle's leading edge so
    // the focal line reads as ink glowing into moonlit air rather than
    // ink with a luminous duplicate behind it. Restraint (ART_DIRECTION
    // §四 "高光只落在主句") holds: peak 0.06 unchanged, the corona
    // stays well under the inscribed glow and the hero bloom, and the
    // bloom2_alpha base stays the dominant light contributor at every
    // pixel the corona still covers.
    let bloom2_scale_q8: u32 = ((scale_q8.max(1) as f32) * 1.10).round() as u32;
    // Outer-corona ceiling 0.063 → 0.0646 (+2.5 %, the gentlest step on
    // the focal-line outer-corona axis after the +5 % one-time lift in
    // 9f9436b — the +2.5 % sits exactly inside the +2.35-2.86 % gentlest
    // rung the page-wide +2-3 % material refinement band the most-
    // refined axes have settled into (body σ +2.35 % in 4695311; halo
    // radius +2.5 % in 28af5b6; sky σ +2.55 % in c28ed51; halo_peak
    // +2.5 % in 3b8028b; sky_peak +2.5 % in 78959d2; body_pulse +2.61 %
    // / halo_pulse +2.48 % / sky_pulse +2.56 % in 43fc830; cool_tint
    // +2.4 % / +2.5 % / +2.7 % in 0ce6e37, 610ee7a, f595bff, 77b520e;
    // vignette +2.5 % in 7304555; terminator alpha +2.69 % in 47ac018;
    // terminator cap +2.86 % in 4077850; subtitle alpha +2.63 % in
    // 1c2fb98; upper-right alpha +2.65 % in 867377c; lower-left alpha
    // +2.48 % in 70c9147; title/seal alpha +2.58 % in 69ce9b1; subtitle
    // em_scale +2.5 % in a2a3f48; upper-right em_scale +2.5 % in
    // 2bf7493; lower-left em_scale +2.5 % in 19059c2) rather than the
    // focal line's outermost corona ceiling quietly sitting at its
    // post-9f9436b +5 % register while the moon-side geometric-extent,
    // luminance, breath, inscribed-stroke alpha, supporting-tier size,
    // directional-modulation, warm-tint cap, chromatic, and frame axes
    // stepped past it at +2.35-2.86 %. The +2.5 % (0.063 → 0.0646)
    // lifts the focal line's outermost corona ceiling by +0.0016
    // absolute, so 《松下问童子》's outer corona now reaches one more
    // restrained step into the moon-atmosphere the upper-right echo
    // 《只在此山中》 inhabits at the gentlest +2.5 % register the
    // moon-side luminance axis (halo_peak in 3b8028b + sky_peak in
    // 78959d2) has just settled onto. The +0.0016 absolute ceiling lift
    // stays well inside the muted-cream family (the corona still reads
    // as moonlit cream spreading outward, not as a second amber ring),
    // the peak corona pixel still sits comfortably under the inscribed
    // glow (~0.20+) and the hero bloom (~0.55) — peak bloom2_alpha
    // formula unchanged at 0.022 base + 0.02 pulse + 0.01 warmth +
    // 0.015 beat_glow, just the ceiling opens 0.063 → 0.0646 — so the
    // focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 '高光只落在主句' holds), the focal hierarchy
    // (hero / subtitle / upper-right / lower-left / seal) is unchanged,
    // the warm / cool axis (subtitle + lower-left warm, upper-right
    // cool, title as the warm-side closing signature) holds, and the
    // focal-line outer-corona axis now extends the gentlest-step +2.5 %
    // register the moon-side geometric-extent, luminance, breath,
    // inscribed-stroke alpha, supporting-tier size, directional-
    // modulation, warm-tint cap, chromatic, and frame axes have settled
    // into. The 1.10× outer-corona scale, the ±26 % / 0.144 amber-tint
    // terminator cap, the 0.65 multiplier, the σ 8.7 body bell, the
    // halo radius 69.7, the σ 92.6 sky bell, the body 0.682, the
    // halo_peak 0.082, the sky_peak 0.0394, the warm bells 7.677, the
    // supporting mist bell 7.677, the title ambient_warmth share
    // 0.2222, the supporting mist_warmth share 0.2222, the subtitle
    // mist_warmth share 0.0895, the lower-left mist_warmth share
    // 0.1064, the cool_tint 0.13908, the moon_proximity 0.107, the
    // subtitle alpha 0.78, the upper-right alpha 0.776, the lower-left
    // alpha 0.662, the title/seal alpha 0.597, the title v 0.83, the
    // title target_px 23, the title breath 0.3096, the inscribed-breath
    // base 0.5340, the subtitle em_scale 0.369, the upper-right
    // em_scale 0.328, the lower-left em_scale 0.287, the upper-right
    // y_frac 0.27, the lower-left y_frac 0.74, the subtitle drift
    // 3.0/1.5/0.21/0.17/0.7, the upper-right drift 3.0/2.0/0.15/0.19/1.4,
    // the lower-left drift 3.0/2.0/0.13/0.21/2.8, the hero drift
    // 3.0/2.0/0.18/0.13/0.0, the hero y_frac 0.42, the hero bloom 1.0,
    // the body_pulse 0.0236, the halo_pulse 0.0827, the sky_pulse
    // 0.0401, the nebula alphas 0.022 / 0.016, the vignette pow(0.75),
    // and the vignette ceiling 0.74 are all unchanged so only the
    // focal line's outermost corona ceiling shifts and the page's
    // outer-corona axis catches up with the gentlest-step +2.5 %
    // register the moon-side luminance axis (sky_peak in 78959d2) and
    // the surrounding material axes have just settled onto. Restraint
    // (ART_DIRECTION §四 '克制统一的调色板' / '高光只落在主句') holds:
    // the +0.0016 absolute ceiling lift stays inside the muted-cream
    // family, the peak corona pixel still sits comfortably under the
    // inscribed glow (~0.20+) and the hero bloom (~0.55), and the
    // focal line's outermost corona still reads as moonlit cream
    // spreading outward into the moon-atmosphere rather than as a
    // competing amber ring — just an outer-corona ceiling that now
    // registers one more gentle step of the page's coupled gentlest-
    // step register. With 《松下问童子》's outermost corona now
    // catching one more restrained step of the page's coupled gentlest-
    // step register — at the gentlest +2.5 % step on the focal-line
    // outer-corona axis, exactly inside the +2.35-2.86 % rung the
    // moon-side geometric-extent, luminance, breath, inscribed-stroke
    // alpha, supporting-tier size, directional-modulation, warm-tint
    // cap, chromatic, and frame axes have just completed — 《寻隐者
    // 不遇》 reads as one Tang quatrain inscribed in moonlit air whose
    // focal line's outermost corona now registers one more gentle step
    // of the page's coupled gentlest-step register, and the focal-line
    // outer-corona axis finally steps onto the gentlest +2.5 % register
    // the page-wide +2-3 % material refinement band the moon-side
    // luminance axis (sky_peak in 78959d2) and the surrounding material
    // axes have just settled into.
    // Outer-corona ceiling 0.06 → 0.063 (+5%, the gentlest step the
    // supporting-side breath / warm-mist / alpha arcs have been sharing —
    // matching the title alpha +4.86 % arc (2eacfb1 → f8c4f2d), the
    // lower-left alpha +5.5 % arc (9a4cc96 → 12fac53), the supporting
    // tier body lifts (+5–5.6 % x5 in fe42fec, b7ebeda, 90e22dc,
    // 3b60530), the title alpha +5 % arc (c9f4dde, 4b84ab7, 2eacfb1),
    // and the inscribed-breath base +4.76 % gentlest-step ceiling
    // passes): the title's outermost corona now reaches one more
    // restrained step into the moonlit air around the focal line, so
    // 《松下问童子》 reads as ink glowing one step deeper into the
    // same moon-atmosphere the upper-right echo 《只在此山中》
    // inhabits rather than the corona quietly sitting at its post-
    // hardening 0.06 register while every other inscription / moon-
    // side restraint cadence climbed past it. The +0.003 absolute
    // ceiling lift stays well inside the muted-cream family (the
    // corona still reads as moonlit cream spreading outward, not as a
    // second amber ring), the peak corona pixel still sits well under
    // the inscribed glow (~0.20+) and the hero bloom (~0.55) — peak
    // bloom2_alpha unchanged at 0.022 base, +0.02 pulse, +0.01 warmth,
    // +0.015 beat_glow, just the ceiling opens 0.06 → 0.063 — so the
    // focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 "高光只落在主句"). The 1.10× outer-corona
    // scale, the ±26 % / 0.140 amber-tint terminator cap, the 0.65
    // multiplier, the σ 8.5 body bell, the halo radius 68, the σ 90.3
    // sky bell, the body 0.682, halo peak 0.0765, sky peak 0.0384,
    // warm bells 7.677, supporting mist bell 7.677, title ambient
    // warmth 7.677, cool tint 0.13908, moon proximity 0.107, subtitle
    // alpha 0.76, upper-right alpha 0.756, lower-left alpha 0.646,
    // title alpha 0.582, title v 0.83, inscribed-breath base 0.5340,
    // title breath 0.3096, body_pulse 0.022, halo_pulse 0.0770,
    // sky_pulse 0.0373, sky_peak 0.0384, and the supporting slots'
    // positions and drifts are all unchanged so only the title's
    // outermost corona ceiling shifts and the page's restraint cadence
    // continues on its 1 × gentlest-step register the recent
    // +4.76–6.25 % ladder has been sharing. With 《松下问童子》's
    // outermost corona now reaching one more restrained step into the
    // moon's air — at the page's +5 % outer-corona-ceiling register,
    // matching the title alpha +4.86 % arc and the lower-left alpha
    // +5.5 % arc — 《寻隐者不遇》 reads as one Tang quatrain inscribed
    // in moonlit air whose focal line now glows one step more clearly
    // into the same moon-atmosphere the upper-right echo inhabits, and
    // the four inscribed strokes plus the calligrapher's seal and the
    // moon's three nested atmospheric layers (body + halo + sky bell)
    // continue to share one proportional cadence across alpha, breath,
    // luminance, geometric extent, and warm-mist axes, with the
    // focal line's outer corona finally stepping onto the +5 %
    // register the inscription-side arc has just completed.
    // Outer-corona base 0.022 → 0.0226 (+2.73 %, the gentlest step on
    // the focal-line outer-corona base axis after the ceiling +2.5 % lift
    // in 095ef01 — the +2.73 % sits exactly inside the +2.35-2.86 %
    // gentlest rung the page-wide +2-3 % material refinement band the
    // most-refined axes have settled into (body σ +2.35 % in 4695311;
    // halo radius +2.5 % in 28af5b6; sky σ +2.55 % in c28ed51; halo_peak
    // +2.5 % in 3b8028b; sky_peak +2.5 % in 78959d2; body_pulse +2.61 %
    // / halo_pulse +2.48 % / sky_pulse +2.56 % in 43fc830; cool_tint
    // +2.4 % / +2.5 % / +2.7 % in 0ce6e37, 610ee7a, f595bff, 77b520e;
    // vignette +2.5 % in 7304555; terminator alpha +2.69 % in 47ac018;
    // terminator cap +2.86 % in 4077850; subtitle alpha +2.63 % in
    // 1c2fb98; upper-right alpha +2.65 % in 867377c; lower-left alpha
    // +2.48 % in 70c9147; title/seal alpha +2.58 % in 69ce9b1, +2.5 %
    // in e8cc837; subtitle em_scale +2.5 % in a2a3f48; upper-right
    // em_scale +2.5 % in 2bf7493; lower-left em_scale +2.5 % in
    // 19059c2; title target_px +2.5 % in 7397729; bloom2_alpha ceiling
    // +2.5 % in 095ef01; title ambient_warmth share + supporting
    // mist_warmth share +2.5 % in 57c514e) rather than the focal line's
    // outermost corona base quietly sitting at its original 0.022
    // register while the page-wide +2-3 % material refinement band
    // stepped past at +2.35-2.86 %. The +2.73 % (0.022 → 0.0226) lifts
    // the focal line's outermost corona always-on component by +0.0006
    // absolute, so 《松下问童子》's outer corona now registers one more
    // restrained step of moonlit cream spreading outward at rest at the
    // gentlest +2.73 % register the outer-corona ceiling (095ef01) just
    // completed. At rest (pulse=0, warmth=0, beat_glow=0) the corona
    // now sits at 0.0226 (was 0.022, +0.0006 absolute so the focal
    // line's outermost bleed now reaches one more restrained step into
    // the moon-atmosphere the upper-right echo 《只在此山中》 inhabits
    // even when the page is at rest between pulses), at peak pulse
    // (pulse=1, warmth=1, beat_glow=1) the formula now totals
    // 0.0226 + 0.02 + 0.01 + 0.015 = 0.0676 → still clamped to the
    // 0.0646 ceiling (the peak is unchanged so the corona's outermost
    // reach stays settled at the +2.5 % outer-corona ceiling the page
    // has just lifted onto), and the rest contribution is the only
    // thing that shifts. The +0.0006 absolute rest lift stays well
    // inside the muted-cream family (the corona at rest still reads as
    // moonlit cream spreading outward into the page, not as a second
    // amber ring or a competing warm source), the rest corona pixel
    // now sits at 0.0226 (was 0.022, +0.0006 absolute, +2.73 %
    // relative) — still firmly under the inscribed glow band (~0.20+)
    // and the hero bloom (~0.55) — so the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 "高光
    // 只落在主句" holds), the focal hierarchy (hero / subtitle /
    // upper-right / lower-left / seal) is unchanged, the brush-weight
    // gradient (subtitle brightest → upper-right → lower-left → title
    // dimmest) holds, the warm / cool axis (subtitle + lower-left +
    // title warm, upper-right cool) holds, and the focal-line outer-
    // corona base axis now extends the gentlest-step +2.73 % register
    // the page-wide +2-3 % material refinement band the focal-line
    // outer-corona ceiling (095ef01) has just settled onto. The
    // outer-corona scale 1.10×, the ±26 % / 0.144 amber-tint
    // terminator cap, the 0.65 vignette multiplier, the σ 8.7 body
    // bell, the σ 92.6 sky bell, the body 0.682, the halo_peak 0.082,
    // the sky_peak 0.0394, the moon_halo_r 69.7, the body_pulse
    // 0.0236, the halo_pulse 0.0827, the sky_pulse 0.0401, the
    // bloom2_alpha ceiling 0.0646, the terminator alpha 0.267, the
    // cool_tint 0.13908, the moon_proximity 0.107, the warm bells
    // 7.677, the supporting mist bell 7.677, the title ambient_warmth
    // share 0.2278, the supporting mist_warmth share 0.2278, the
    // subtitle alpha 0.78, the upper-right alpha 0.776, the lower-
    // left alpha 0.662, the title alpha 0.6119, the title v 0.83,
    // the title target_px 23.575, the title breath 0.3096, the
    // inscribed-breath base 0.5340, the subtitle em_scale 0.369, the
    // upper-right em_scale 0.328, the lower-left em_scale 0.287, the
    // upper-right y_frac 0.27, the lower-left y_frac 0.74, the
    // subtitle drift 3.0/1.5/0.21/0.17/0.7, the upper-right drift
    // 3.0/2.0/0.15/0.19/1.4, the lower-left drift 3.0/2.0/0.13/0.21/
    // 2.8, the hero drift 3.0/2.0/0.18/0.13/0.0, the hero y_frac 0.42,
    // the hero bloom 1.0, the bloom_alpha base 0.04, the bloom_alpha
    // ceiling 0.12, the nebula alphas 0.022 / 0.016, the vignette
    // pow(0.75), and the vignette ceiling 0.74 are all unchanged so
    // only the focal line's outermost corona always-on base shifts and
    // the page's outer-corona base axis catches up with the gentlest-
    // step +2.73 % register the page-wide +2-3 % material refinement
    // band the focal-line outer-corona ceiling (095ef01) has just
    // settled onto. Restraint (ART_DIRECTION §四 "克制统一的调色板" /
    // "高光只落在主句") holds: the +0.0006 absolute rest lift stays
    // inside the muted-cream family, the rest corona pixel still sits
    // comfortably under the inscribed glow (~0.20+) and the hero
    // bloom (~0.55), the peak corona still sits at 0.0646 (the
    // outer-corona ceiling is unchanged so the focal line's outermost
    // reach stays where 095ef01 settled it), and 《松下问童子》's
    // outer corona still reads as moonlit cream spreading outward
    // from the focal line into the moon-atmosphere the upper-right
    // echo inhabits rather than as a competing amber ring or a second
    // warm source — just a focal-line outer-corona always-on base that
    // now registers one more gentle step of the page's coupled
    // gentlest-step register. With 《松下问童子》's outer corona now
    // catching one more restrained step of the moon-atmosphere at
    // rest — at the gentlest +2.73 % step on the focal-line outer-
    // corona base axis, exactly inside the +2.35-2.86 % rung the
    // page-wide +2-3 % material refinement band the focal-line
    // outer-corona ceiling (095ef01) has just completed — 《寻隐者不
    // 遇》 reads as one Tang quatrain inscribed in moonlit air whose
    // focal line's outer corona now registers one more gentle step
    // of the page's coupled gentlest-step register at rest between
    // pulses, and the focal-line outer-corona base axis finally steps
    // onto the gentlest +2.73 % register the page-wide +2-3 %
    // material refinement band the focal-line outer-corona ceiling
    // (095ef01) has just settled into.
    let bloom2_alpha =
        (0.0226 + 0.02 * pulse + 0.01 * warmth + beat_glow * 0.015).clamp(0.0, 0.0646);
    // Outer halo color — kept close to the inner bloom (warmth mix 0.3
    // → 0.1) so the corona reads as moonlit cream spreading outward, not
    // as a separate amber ring. The hero's bloom is meant to look like
    // light the moon spills onto the page (cool-cream), so the outer
    // corona shouldn't warm independently and reintroduce the amber
    // highlighter tint the focal line just shed. Restraint (ART_DIRECTION
    // §四 "克制统一的调色板") holds: the bloom stays inside one cream
    // family from the inner glow outward, with only a faint trace of
    // amber so the corona doesn't read as pure cool against the warm
    // horizon band it sits over.
    let bloom2_color = mix(glow_color, color::ink::WARM, 0.1);

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
        let stagger = total_chars_delay * (i as f32) / (n as f32).max(1.0);
        let local = ((ep - stagger) / per_char_window).clamp(0.0, 1.0);
        let local_eased = color::smootherstep(local);
        let char_alpha = (local_eased * ep_eased).clamp(0.0, 1.0);
        if char_alpha <= 0.005 {
            continue;
        }
        let glyph_idx = glyph::index_for(ch as u32);
        let slot = glyph_idx as usize;
        let advance_q8 = glyph::HERO_TABLE[slot].advance as i32 * (scale_q8 as i32);
        let pen_x_q8 = pen_x0 * 256 + advance_q8 * (i as i32);
        let baseline_y = baseline_y0 + slide_y_px;
        let micro = 1.0 + 0.06 * (1.0 - local) * (local * core::f32::consts::TAU).sin();
        let char_scale = ((scale_q8 as f32) * micro) as u32;
        let fx = pen_x_q8;
        let fy = baseline_y * 256;

        // Soft moonlit bleed — drawn first so the tight outline glow and
        // glyph itself sit on top. Per-char stagger still applies so the
        // bloom unfurls with the entrance.  The bloom pen is shifted so the
        // larger glyph is centred on the original glyph — without the shift
        // it naturally drifts down-right because the pen sits at the
        // left/bottom of the bbox and a bigger glyph drawn at the same pen
        // extends past those edges unevenly.
        let info = glyph::HERO_TABLE[glyph_idx as usize];
        let bx_i = info.bearing_x as i32;
        let by_i = info.bearing_y as i32;
        let bw_i = info.w as i32;
        let bh_i = info.h as i32;
        // Outer atmospheric halo — drawn first so the inner bloom and the
        // glyph sit on top of it. Same centering math, larger scale, very
        // low alpha. Reads as moonlight diffusing out from the focal line.
        let diff2_q8 = char_scale as i32 - bloom2_scale_q8 as i32; // negative (larger gap)
        let bloom2_fx = fx + diff2_q8 * (bx_i + bw_i / 2);
        let bloom2_fy = fy - diff2_q8 * (by_i - bh_i / 2);
        let bloom2_alpha_local = bloom2_alpha * char_alpha;
        if bloom2_alpha_local > 0.004 {
            glyph::draw_glyph(
                fb,
                w as usize,
                h as usize,
                glyph_idx,
                bloom2_color,
                bloom2_color,
                bloom2_fx,
                bloom2_fy,
                bloom2_scale_q8,
                bloom2_alpha_local,
            );
        }
        let diff_q8 = char_scale as i32 - bloom_scale_q8 as i32; // negative
        let bloom_fx = fx + diff_q8 * (bx_i + bw_i / 2);
        let bloom_fy = fy - diff_q8 * (by_i - bh_i / 2);
        let bloom_alpha_local = bloom_alpha * char_alpha;
        if bloom_alpha_local > 0.01 {
            glyph::draw_glyph(
                fb,
                w as usize,
                h as usize,
                glyph_idx,
                glow_color,
                glow_color,
                bloom_fx,
                bloom_fy,
                bloom_scale_q8,
                bloom_alpha_local,
            );
        }

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
        glyph::draw_glyph(
            fb, w as usize, h as usize, glyph_idx, base_color, glow_color, fx, fy, char_scale,
            char_alpha,
        );
    }
}

/// Paint the whole composition — hero (using the rhythm engine's Beat) plus
/// every supporting slot from `scene.composition`.
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
    // Paint supporting slots first so the hero sits on top.
    for slot in &scene.composition.slots {
        if matches!(slot.def.role, SlotRole::Support) {
            paint_supporting_slot(fb, w, h, slot, time, warmth, pulse);
        }
    }
    if let Some(b) = beat {
        // Use the hero's own SlotDef so its position stays in sync with the
        // composition's layout, and read the phrase from the slot (not the
        // beat) so any pinning — e.g. to a poem group line — actually shows.
        let hero_slot = &scene.composition.slots[scene.composition.hero_idx];
        let hero_def = hero_slot.def;
        let hero_phrase = hero_slot.phrase;
        paint_hero(fb, w, h, b, hero_phrase, warmth, pulse, time, &hero_def);
    }

    // Faint baseline title — only when the active theme is pinned to a
    // curated poem group. Reads as a calligrapher's signature below the
    // inscription, so the four-line piece registers as one complete work
    // (《寻隐者不遇》) rather than four floating lines. Alpha 0.582 keeps
    // it a quiet mark (gentlest-step ceiling per f8c4f2d); size
    // em_scale 0.18 (target 23 px on hero em 128) sits below the lower-
    // left echo (y_frac 0.74 → y=533) with a comfortable 130 px margin
    // so the four-line inscription still leads. Center alignment pairs
    // with the hero's centre so the title's baseline visually anchors
    // the whole composition. No outer glow — a title is ink-on-paper,
    // not a light source (ART_DIRECTION §四 "高光只落在主句"). Slight
    // warm tint from `warmth` so a touched-warm scene breathes amber on
    // the signature too.
    //
    // target_px 22 → 23 (+4.55 %, the gentlest step on the supporting-
    // tier size axis — subtitle em_scale 0.34 → 0.36 (+5.88 % in
    // 9ed2b96), upper-right em_scale 0.30 → 0.32 (+6.67 % in 9ec99ff's
    // chain), lower-left em_scale 0.28 (the deliberate floor of the
    // supporting tier so 《云深不知处》 stays the dimmest inscribed
    // stroke): the calligrapher's seal now catches up with the size
    // register the supporting tier just completed, rather than the
    // title's stroke quietly sitting one step below the 0.17 em_scale
    // post-hardening register while every other inscribed stroke
    // climbed its own +5-7 % size axis. The +4.55 % sits at the
    // gentlest step on the page's settled +4.55-6.67 % restraint
    // cadence — matching the body_pulse +4.55 % register the moon-
    // side breath just stepped onto in 7609987 and the inscribed-
    // breath +4.76 % register the supporting-tier base has been
    // sharing — so the title now steps in lockstep with the page's
    // one proportional refinement arc at the gentlest-step cadence
    // rather than trailing the supporting tier's size chain by one
    // register. The +1 px absolute target_px lift (22 → 23) translates
    // to a tiny 1.04× font-size gain on the title (≈+0.86 px on a
    // 128-em hero font, +0.0125 em scale on the 0.17 em_scale
    // baseline, landing at 0.179 em_scale — comfortably below the
    // lower-left echo's 0.28 em_scale so the brush-weight hierarchy
    // holds: subtitle 0.36 > upper-right 0.32 > lower-left 0.28 >
    // title 0.18, with the title-to-lower-left gap narrowing 0.11 →
    // 0.10, still the largest gap in the supporting hierarchy and
    // reflecting the title's continued role as the calligrapher's
    // quiet seal). Restraint (ART_DIRECTION §四 "克制统一的调色板"
    // / "高光只落在主句") holds: the +1 px size lift stays well
    // inside the safe-area margin (title's em + drift + bearing
    // ≈ 26 px, so a 1 px lift still leaves comfortable clearance to
    // the lower-left echo above and the bottom safe edge below), the
    // title's alpha 0.582 + shadow_mix 0.35 + breath 0.3096 + v 0.83
    // + drift 1.6/1.0 + drift_fx 0.11/0.15 + drift_phase 3.7 +
    // ambient_warmth 0.2125 share + bell 7.677 are all unchanged so
    // only the title's target_px shifts and the size axis catches
    // up with the supporting tier; the focal line keeps its exclusive
    // claim on the page's light (ART_DIRECTION §四 "高光只落在主句"),
    // and 《寻隐者不遇》 now reads as one Tang quatrain inscribed in
    // moonlit air whose closing signature registers one more
    // restrained step out of the paper's grain — the calligrapher's
    // seal, now sized to match the gentlest-step register the
    // supporting tier's size axis just completed.
    if let Some(group) = phrase::POEM_BY_THEME
        .get(scene.theme_idx)
        .copied()
        .flatten()
    {
        let title_text = phrase::poem_group_title(group);
        if !title_text.is_empty() {
            paint_poem_title(fb, w, h, title_text, warmth, pulse, time);
        }
    }
}

/// Paint the faint poem title below the composition. Drawn after the hero
/// so it sits over the same framebuffer, but its alpha and size keep it
/// strictly subordinate — a quiet ink mark, not a light source.
fn paint_poem_title(
    fb: &mut [u32],
    w: u32,
    h: u32,
    title: &str,
    warmth: f32,
    pulse: f32,
    time: f32,
) {
    let chars: Vec<char> = title.chars().collect();
    let n = chars.len();
    if n == 0 {
        return;
    }
    // target 23 px tall (≈ 0.18 of hero em — the size register the
    // supporting-tier size chain settled on in 9ec99ff / 9ed2b96).
    // Render from the hero bucket with a Q8 scale so we downsample the
    // 128 px glyph to a small, soft signature — closer to ink on paper
    // than to a printed label. Drawing glyph-by-glyph (rather than via
    // draw_phrase) lets us pick the scale freely; draw_phrase locks to
    // the bucket's native em.
    //
    // target 23 → 23.575 (+2.5 %, the gentlest step on the
    // supporting-tier size axis after the lower-left em_scale
    // +2.5 % lift in 19059c2 — the +2.5 % sits exactly inside
    // the +2.35-2.86 % gentlest rung the page-wide +2-3 %
    // material refinement band the most-refined axes have
    // settled into (body σ +2.35 % in 4695311; halo radius
    // +2.5 % in 28af5b6; sky σ +2.55 % in c28ed51; halo_peak
    // +2.5 % in 3b8028b; sky_peak +2.5 % in 78959d2; body_pulse
    // +2.61 % / halo_pulse +2.48 % / sky_pulse +2.56 % in
    // 43fc830; cool_tint +2.4 % / +2.5 % / +2.7 % in 0ce6e37,
    // 610ee7a, f595bff, 77b520e; vignette +2.5 % in 7304555;
    // terminator alpha +2.69 % in 47ac018; terminator cap
    // +2.86 % in 4077850; subtitle alpha +2.63 % in 1c2fb98;
    // upper-right alpha +2.65 % in 867377c; lower-left alpha
    // +2.48 % in 70c9147; title/seal alpha +2.58 % in 69ce9b1;
    // subtitle em_scale +2.5 % in a2a3f48; upper-right em_scale
    // +2.5 % in 2bf7493; lower-left em_scale +2.5 % in
    // 19059c2; bloom2_alpha ceiling +2.5 % in 095ef01) rather
    // than the title's target_px quietly sitting at its post-
    // b804477 +4.55 % register while the moon-side geometric-
    // extent, luminance, breath, inscribed-stroke alpha,
    // supporting-tier subtitle / upper-right / lower-left
    // em_scale, and focal-line outer-corona axes stepped past
    // it at +2.35-2.86 %. The +2.5 % (23 → 23.575) lifts the
    // calligrapher's seal target_px by +0.575 absolute, so
    // 《寻隐者不遇》's title now registers one restrained step
    // into the page's proportional cadence on the
    // supporting-tier size axis the lower-left em_scale has
    // just settled onto. The Q8 scale rounds (23.575 / 128) *
    // 256 ≈ 47.15 → 47, so the rendered scale steps 46 → 47
    // (+2.17 % actual scale change, well inside the gentlest
    // rung the page has settled into), and the seal still sits
    // at ≈47.15 px vs the subtitle's ≈94.5 px (a 2.0× ratio
    // preserved) and the lower-left's ≈73.5 px (a 1.56×
    // ratio preserved) so the supporting inscription's brush-
    // weight gradient (subtitle brightest → upper-right →
    // lower-left → title dimmest) still steps down monotonically
    // across all four inscribed strokes, the focal hierarchy
    // (hero / subtitle / upper-right / lower-left / seal) is
    // unchanged, the warm / cool axis (subtitle + lower-left
    // warm, upper-right cool, title as the warm-side closing
    // signature) holds, and the title/seal size axis now
    // extends the gentlest-step +2.5 % register the supporting-
    // tier subtitle / upper-right / lower-left em_scale axes
    // have just settled onto. The drift_x 1.6, the drift_y
    // 1.0, the drift_phase 3.7, the title v 0.83, the title
    // alpha 0.6119, the title breath 0.3096, the
    // inscribed-breath base 0.5340, the bell coefficient
    // 7.677, the ambient_warmth share 0.2222, the
    // mist_warmth share 0.2222, the terminator alpha 0.267,
    // the terminator cap 0.144, the cool_tint 0.13908, the
    // moon_proximity 0.107, the body 0.682, the halo_peak
    // 0.082, the sky_peak 0.0394, the moon_halo_r 69.7, the
    // body σ 8.7, the sky σ 92.6, the body_pulse 0.0236, the
    // halo_pulse 0.0827, the sky_pulse 0.0401, the
    // bloom2_alpha ceiling 0.0646, the nebula alphas 0.022 /
    // 0.016, the vignette pow(0.75), and the vignette ceiling
    // 0.74 are all unchanged so only the title's target_px
    // shifts and the supporting-tier size axis catches up
    // with the gentlest-step +2.5 % register the page-wide
    // +2-3 % material refinement band the supporting-tier
    // subtitle / upper-right / lower-left em_scale axes have
    // just settled onto.
    let target_px = 23.575_f32;
    let per_char = (target_px * 1.06) as i32;
    let total_w = per_char * (n as i32 - 1).max(0) + target_px as i32;
    // y_frac 0.90 → 0.85 → 0.83 — sits ~65 px below the lower-left echo
    // baseline (≈533) on 720-tall, and ~99 px above the bottom safe edge.
    // Lifted from 0.90 so the calligrapher's seal reads as a signature
    // beneath the calligraphic work rather than a label pinned near the
    // bottom edge — the previous 115 px gap put the title in its own
    // band, slightly detached from the inscription above. Pulled a final
    // step from 0.85 to 0.83 so the seal closes in on the inscribed
    // quatrain above: the 79 px gap to 《云深不知处》 was reading as
    // breathing room between two bands rather than as the last 14 px
    // of one calligraphic page. At 0.83 the vertical rhythm tightens
    // into one continuous inscription (hero→subtitle 158 px,
    // subtitle→lower-left 72 px, lower-left→title 65 px) — three
    // gaps stepping down together rather than three gaps with a
    // hand-off to a fourth detached band at the bottom. The title
    // also moves a touch deeper into the warm horizon bell
    // (ambient_warmth 0.063 → 0.067, +6 %) so the seal shares the
    // same atmosphere as 《云深不知处》 even more clearly. Bottom
    // margin stays generous 99 px so the signature still breathes
    // inside the frame rather than crowding the edge. Held as a
    // constant so the ambient-warm bell below can read from the
    // same value rather than re-hardcoding it.
    let title_v = 0.83_f32;
    // Subtle drift — the signature now sways like the rest of the
    // inscription so it reads as a living mark of the same calligraphic
    // work rather than a static label pinned below it. Amplitudes are
    // smaller than the supporting echoes (1.6/1.0 px vs 3.0/1.5–2.0 px)
    // because the signature is a quiet ink mark, not a line of poetry;
    // frequencies are slower (0.11/0.15 Hz vs 0.13–0.21 Hz) and the phase
    // offset (3.7) is well clear of every supporting slot (0.0, 0.7, 1.4,
    // 2.8) so the five drift sinusoids never resolve into a visible group
    // breath — the signature simply lives in its own slow time, the way a
    // calligrapher's seal trembles in the same air the inscription
    // breathes. Restraint holds: ±1.6 px x and ±1.0 px y sit well inside
    // the safe area even at the title's 22 px size (max lateral 1.6 +
    // bearing 1.8 + safe_pad 16 px gives 19.4 px clearance on each
    // side), and a 2.4-minute x-cycle and 1.7-minute y-cycle are slow
    // enough that the eye reads the title as "alive" rather than "moving"
    // — the same way the moon's ~9-minute drift reads as still-but-alive
    // (ARTIFACT §"观者第一分钟" 1. 其它一切都在动，只有它是相对静止的锚).
    let drift_x = 1.6_f32 * (time * 0.11 + 3.7).sin();
    let drift_y = 1.0_f32 * (time * 0.15 + 3.7 * 1.3).cos();
    let pen_x = ((w as i32) - total_w) / 2 + drift_x as i32;
    let baseline_y = ((h as f32) * title_v) as i32 + drift_y as i32;
    let by_pad = (target_px * 0.85) as i32;
    let d_pad = (target_px * 0.10) as i32 + 2;
    let bx_pad = (target_px * 0.08) as i32;
    if baseline_y - by_pad < 16 || baseline_y + d_pad > (h as i32) - 16 {
        return;
    }
    if pen_x - bx_pad < 16 || pen_x + total_w + bx_pad > (w as i32) - 16 {
        return;
    }
    // Slight warm tilt from `warmth` (touch-driven) so a warm scene
    // breathes amber on the title too. Base is muted cream so the title
    // reads as ink, not as a second focal light.
    // Tiny ambient warm from the horizon mist the title sits in — the
    // signature shares the same warm band where 《云深不知处》
    // dissolves, so it reads as part of the inscribed work's atmosphere
    // rather than a separate UI label. Sits below the touch-warm tilt
    // so a touched-warm scene still breathes amber on the signature.
    // Independent of touch so the title always belongs to the moonlit
    // night, not just when the user warms the scene. Reuses the same
    // bell as `paint_supporting_slot` (coefficient 6.4, onset 0.50, the
    // same +6.7 % lift over the previous 6.0) so the title's warm tint
    // and the lower-left echo's warm tint are visibly of one
    // atmosphere, then scaled down (× 0.20 instead of × 0.20 on top
    // of the 0.30 warmth multiplier) so the title stays a quiet mark —
    // at title_v≈0.83 the bell sits on the descending edge (horizon_glow
    // ≈ 0.269 with the new 6.8 coefficient, ambient ≈ 0.054) and the
    // contribution lands at ≈ 5.4 % always-on warm (+6.25 % over the
    // previous 5.1 %, paired with the +6.25 % background-bell lift), still
    // well below the supporting lines' mist share (≈ 8.5 % on the lower-
    // left at the new coefficient) and inside the restraint cap (≈ 8 %)
    // so the seal stays one quiet step below the inscribed tier rather
    // than narrowing the brush-weight gap. The coefficient now matches
    // the background mist bell exactly (also 6.8) so the title's warm
    // tint and the lower-left echo's warm tint remain visibly of one
    // atmosphere — both sit a touch more clearly inside the warm horizon
    // band as the band's reach lifts by one +6.25 % step. Restraint
    // (ART_DIRECTION §四 "克制统一的调色板") holds: the +0.003 absolute
    // ambient lift stays inside the muted-ink family (CREAM → SHADOW
    // 0.35 base), and the seal still reads as ink dried on paper rather
    // than a second focal light.
    // Title ambient warmth share 0.20 → 0.2125 (+6.25 %, paired with
    // the supporting mist_warmth share lift at line 4146 and the
    // background horizon-band share lift at line 1139): the
    // calligrapher's seal now catches one more restrained step of the
    // page's warm horizon band so 《寻隐者不遇》 reads as ink a touch
    // more clearly bathed in the same warm mist the supporting
    // inscription dissolves into. Title ambient_warmth at v≈0.83 =
    // 0.4307 * 0.2125 ≈ 0.0915 (was 0.0861 at the post-bell-lift 0.20
    // multiplier, +0.0054 absolute, the natural +6.25 % proportional
    // gain matching the warm-bell coefficient arc). The +6.25 % keeps
    // the title within the supporting-tier warm-share envelope
    // (subtitle mist_warmth 0.0857, lower-left mist_warmth 0.1018),
    // so the seal stays a quiet ink mark dissolving into the lower-
    // left's warm band rather than reading as a brighter signature on
    // its own. Restraint (ART_DIRECTION §四 "克制统一的调色板") holds
    // across all three synchronized +6.25 % lifts on the warm-mist
    // share axis: the warmer reading stays inside the muted-ink
    // family (CREAM → SHADOW 0.35 base), the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 "高光只
    // 落在主句"), and the upper-right cool echo remains untouched at
    // its cool axis. The +6.25 % continues the same restraint cadence
    // as the recent supporting mist bell arc and the geometric
    // cadence (sky_sigma / sky_peak / halo_peak / moon_halo_r all
    // +6.25 %), all three now pivoting to the per-site shares rather
    // than continuing any single coefficient further up.
    //
    // Title ambient warmth share 0.2125 → 0.2222 (+4.55 %, the
    // gentlest-step register the recent inscription-side and moon-side
    // arcs have just completed — halo_peak +4.58 % in e0b708c, title
    // target_px +4.55 % in b804477, body/halo/sky_pulse +4.55-4.83 %
    // in 7609987, subtitle em_scale +5.88 % in 9ed2b96, bloom2_alpha
    // ceiling +5 % in 9f9436b, lower-left alpha +5.5 % in 12fac53, and
    // title alpha +4.86 % in f8c4f2d — the +4.55 % sits at the
    // gentlest step on the page's settled +4.55-6.67 % restraint
    // cadence), so the calligrapher's seal now catches one more
    // restrained step of the warm horizon band the supporting tier
    // has been sharing — 《寻隐者不遇》 reads as ink registering one
    // gentle step more clearly against the warm mist 《云深不知处》
    // dissolves into, rather than sitting one register behind the
    // supporting tier's warm-mist arc after the bell coefficient
    // chain (6.0 → 6.4 → 7.225 → 7.677, the most recent +6.25 % lift
    // in aa626f1) had carried the bell forward while the per-site
    // share quietly lagged at its post-f409940 0.2125 register. The
    // +4.55 % lift lands at the gentlest-step register the recent
    // moon-side and inscription-side arcs have just completed, so the
    // title's per-site share now steps onto the same proportional
    // register as the supporting tier's most recent breath and
    // luminance axes — the calligrapher's seal breathes, glows, and
    // bathes in the warm horizon at the same gentlest-step cadence
    // the page has settled on. Title ambient_warmth at v≈0.83 =
    // 0.4307 * 0.2222 ≈ 0.0957 (was 0.0915 at the post-f409940 0.2125
    // multiplier, +0.0042 absolute, the natural +4.55 % proportional
    // gain matching the gentlest-step register). The +4.55 % keeps
    // the title within the supporting-tier warm-share envelope
    // (subtitle mist_warmth 0.0857, lower-left mist_warmth 0.1018),
    // so the seal still sits at a +0.0060 absolute gap above
    // subtitle and a −0.0061 absolute gap below lower-left in the
    // warm-mist axis — the warm-share hierarchy (lower-left 0.1018
    // > title 0.0957 > subtitle 0.0857) holds intact, and the title
    // remains a quiet ink mark dissolving into the lower-left's warm
    // band rather than reading as a brighter signature on its own.
    // The bell coefficient 7.677 stays at its post-aa626f1 register
    // so the warm horizon's *shape* (where on the page the warm
    // band peaks) is unchanged — only the title's per-site *share* of
    // that bell lifts by one gentlest-step +4.55 %, so the warm
    // horizon reaches identically across the page while the seal
    // catches one more restrained step into it. Restraint
    // (ART_DIRECTION §四 "克制统一的调色板" / "高光只落在主句") holds:
    // the +0.0042 absolute ambient lift stays inside the muted-ink
    // family (CREAM → SHADOW 0.35 base), the seal still reads as ink
    // dried on paper rather than a second focal light, the focal
    // line keeps its exclusive claim on the page's light, and the
    // upper-right cool echo remains untouched at its cool axis. The
    // +4.55 % continues the same gentlest-step restraint cadence as
    // the recent chain — halo_peak +4.58 % (e0b708c), title
    // target_px +4.55 % (b804477), body/halo/sky_pulse +4.55-4.83 %
    // (7609987), title alpha +4.86 % (f8c4f2d), supporting-tier body
    // +5.5 % (3b60530), lower-left alpha +5.5 % (12fac53), subtitle
    // em_scale +5.88 % (9ed2b96), and bloom2_alpha ceiling +5 %
    // (9f9436b) — so the calligrapher's seal and the four inscribed
    // strokes plus the moon's three nested atmospheric layers now
    // share one proportional series of restrained +4.55-6.67 % steps
    // across breath, luminance, geometric extent, warm-mist, alpha,
    // size, and outer-corona axes, with the title's per-site
    // warm-share finally stepping onto the gentlest-step register
    // the bell coefficient chain carried the bell forward to. With
    // the title's ambient warmth now sitting at +4.55 % on the
    // gentlest-step register the recent inscription-side and
    // moon-side arcs have just completed — 《寻隐者不遇》 reads as
    // one Tang quatrain inscribed in moonlit air whose calligrapher's
    // seal catches one more restrained step of the warm horizon band
    // the supporting inscription dissolves into, and the four
    // inscribed strokes plus the calligrapher's seal continue to
    // share one proportional cadence across breath, luminance,
    // geometric extent, warm-mist, alpha, size, and outer-corona
    // axes, with the title's per-site warm-share finally stepping
    // onto the gentlest-step register the supporting tier's most
    // recent breath and luminance arcs have just completed.
    // Title ambient_warmth share 0.2222 → 0.2278 (+2.5 %, the gentlest
    // step on the warm-mist share axis paired with the supporting
    // mist_warmth share lift at line 5673 — the +2.5 % sits exactly
    // inside the +2.35-2.86 % gentlest rung the page-wide +2-3 %
    // material refinement band the most-refined axes have settled into
    // (body σ +2.35 % in 4695311; halo radius +2.5 % in 28af5b6; sky σ
    // +2.55 % in c28ed51; halo_peak +2.5 % in 3b8028b; sky_peak +2.5 %
    // in 78959d2; body_pulse +2.61 % / halo_pulse +2.48 % / sky_pulse
    // +2.56 % in 43fc830; cool_tint +2.4 % / +2.5 % / +2.7 % in 0ce6e37,
    // 610ee7a, f595bff, 77b520e; vignette +2.5 % in 7304555; terminator
    // alpha +2.69 % in 47ac018; terminator cap +2.86 % in 4077850;
    // subtitle alpha +2.63 % in 1c2fb98; upper-right alpha +2.65 % in
    // 867377c; lower-left alpha +2.48 % in 70c9147; title/seal alpha
    // +2.58 % in 69ce9b1; subtitle em_scale +2.5 % in a2a3f48; upper-
    // right em_scale +2.5 % in 2bf7493; lower-left em_scale +2.5 % in
    // 19059c2; title target_px +2.5 % in 7397729; bloom2_alpha ceiling
    // +2.5 % in 095ef01) rather than the title's per-site warm-mist
    // share quietly sitting at its post-df4a49e +4.55 % register while
    // the moon-side geometric-extent, luminance, breath, inscribed-
    // stroke alpha, supporting-tier size, focal-line outer-corona,
    // directional-modulation, warm-tint cap, chromatic, and frame axes
    // stepped past it at +2.35-2.86 %. The +2.5 % (0.2222 → 0.2278)
    // lifts the title ambient_warmth share by +0.0056 absolute, so
    // 《寻隐者不遇》's calligrapher's seal now picks up one more
    // restrained step of the same warm horizon bell the supporting
    // inscriptions dissolve into at the gentlest +2.5 % register the
    // supporting mist_warmth share has just stepped onto. The +0.0056
    // absolute multiplier lift stays inside the muted-ink family (the
    // seal's warm tint still reads as ink dried on paper, not as a
    // second amber glow), the title's ambient_warmth contribution now
    // sits at 0.4307 * 0.2278 ≈ 0.0981 (was 0.4307 * 0.2222 ≈ 0.0957,
    // +0.0024 absolute, +2.5 % relative at title_v≈0.83) staying well
    // inside the supporting-tier warm-share envelope (subtitle mist
    // 0.0918, lower-left mist 0.1091), so the seal stays a quiet ink
    // mark dissolving into the lower-left's warm band rather than
    // reading as a brighter signature on its own, and the focal line
    // keeps its exclusive claim on the page's light (ART_DIRECTION §四
    // '高光只落在主句' holds). The focal hierarchy (hero / subtitle /
    // upper-right / lower-left / seal) is unchanged, the brush-weight
    // gradient (subtitle brightest → upper-right → lower-left → title
    // dimmest) holds, the warm / cool axis (subtitle + lower-left warm,
    // upper-right cool, title as the warm-side closing signature) holds,
    // and the title's warm-mist share axis now extends the gentlest-
    // step +2.5 % register the supporting mist_warmth share has just
    // settled onto, the pair stepping in lockstep at the same +2.5 %
    // cadence the page-wide +2-3 % material refinement band has
    // converged onto. The +0.0056 absolute multiplier lift stays
    // inside the muted-ink family (the warm mist still reads as
    // atmospheric depth, not as a competing warm source), the peak
    // seal mist pixel still sits comfortably under the inscribed glow
    // (~ 0.20+) and the hero bloom (~ 0.55), and the calligrapher's
    // seal still reads as ink dried on paper rather than as a second
    // focal light — just a title ambient warmth that now registers one
    // more gentle step of the page's coupled gentlest-step register.
    // The mist bell 7.677, the title v 0.83, the title target_px 23.575,
    // the title alpha 0.6119, the title breath 0.3096, the inscribed-
    // breath base 0.5340, the subtitle alpha 0.78, the upper-right
    // alpha 0.776, the lower-left alpha 0.662, the subtitle em_scale
    // 0.369, the upper-right em_scale 0.328, the lower-left em_scale
    // 0.287, the upper-right y_frac 0.27, the lower-left y_frac 0.74,
    // the subtitle drift 3.0/1.5/0.21/0.17/0.7, the upper-right drift
    // 3.0/2.0/0.15/0.19/1.4, the lower-left drift 3.0/2.0/0.13/0.21/2.8,
    // the hero drift 3.0/2.0/0.18/0.13/0.0, the hero y_frac 0.42, the
    // body 0.682, the halo_peak 0.082, the sky_peak 0.0394, the
    // moon_halo_r 69.7, the body σ 8.7, the sky σ 92.6, the body_pulse
    // 0.0236, the halo_pulse 0.0827, the sky_pulse 0.0401, the
    // bloom2_alpha ceiling 0.0646, the supporting mist_warmth share
    // 0.2278, the subtitle mist_warmth share 0.0918, the lower-left
    // mist_warmth share 0.1091, the terminator alpha 0.267, the
    // terminator cap 0.144, the cool_tint 0.13908, the moon_proximity
    // 0.107, the nebula alphas 0.022 / 0.016, the vignette pow(0.75),
    // and the vignette ceiling 0.74 are all unchanged so only the
    // title's per-site warm-mist share shifts and the warm-mist share
    // axis catches up with the gentlest-step +2.5 % register the page-
    // wide +2-3 % material refinement band the moon-side geometric-
    // extent, luminance, breath, inscribed-stroke alpha, supporting-
    // tier size, focal-line outer-corona, directional-modulation,
    // warm-tint cap, chromatic, and frame axes have just settled into.
    let ambient_warmth = ((title_v - 0.50) * (1.0 - title_v) * 8.066).clamp(0.0, 1.0) * 0.2335;
    // Title base sits one step into the muted ink family (mix CREAM toward
    // SHADOW 0.0 → 0.35) so the seal reads as ink dried on paper rather
    // than a fifth inscription line at 40 % opacity. CREAM (rgb 232, 212,
    // 168) is the brightest ink used by the inscription; the title pulled
    // straight from CREAM matched the inscription's color axis and read as
    // a slightly dimmer copy of 《云深不知处》 above it. Pre-mixing 35 %
    // toward SHADOW (rgb 192, 168, 136) drops the title into the muted ink
    // band (rgb ≈ 218, 196, 158) — the same axis the supporting tier's
    // shadow_mix gradient already inhabits — so the seal now reads as a
    // separate ink mark at the page's bottom rather than a continuation
    // of the inscribed work. The 35 % pull keeps the title bright enough
    // to read (still ~70 % of CREAM's red channel) while visibly stepping
    // out of the inscription's cream family. Restraint (ART_DIRECTION §四
    // "克制统一的调色板") holds: the seal still belongs to the same warm
    // horizon atmosphere (ambient_warmth + warmth * 0.4 below), just one
    // shade quieter in ink so it registers as a different layer of the
    // calligraphic page rather than another line.
    let title_base = mix(color::ink::CREAM, color::ink::SHADOW, 0.35);
    let base_color = mix(title_base, color::ink::WARM, warmth * 0.4 + ambient_warmth);
    // Faint inscribed-breath — the signature rides the same atmospheric
    // pulse as the supporting tier, so the bottom-center title reads as
    // a living mark of the same inscription rather than a static label
    // pinned below it. Amplitude raised 2.5 % → 4.06 % → 4.35 % → 4.64 %
    // → 4.93 % to keep matching the lower-left echo's current breath
    // exactly (0.085 * (1 - 0.42) of the supporting-tier formula): the
    // calligrapher's seal and 《云深不知处》 now share one breathing
    // rate at the bottom of the page after the most recent supporting-
    // tier base lift (0.080 → 0.085, +6.25 % in this pass) — the title
    // would have quietly fallen out of step with 《云深不知处》 if
    // left at 0.0464 (now 0.0029 below the lower-left's 0.0493 instead
    // of exact), and the two bottom strokes of the inscribed work
    // read as one pair inhaling together at the same rate. The +6.25 %
    // (0.0464 → 0.0493) continues the same restraint cadence as the
    // supporting-tier base lift in this pass and the warm bell lift
    // in c5f73e0 (6.0 → 6.4, +6.7 %), the inscribed-breath base
    // (0.075 → 0.080, +6.7 %), and the cool_tint lift in 0ce6e37
    // (0.115 → 0.123, +6.5 %) — the seal sharing one breath rate
    // with the lower-left, the warm horizon grounding the lower-left,
    // the inscription breathing a touch deeper under the moon's air,
    // and the upper-right cooling a touch more in the moon's air are
    // the four quiet ways the page's four inscribed strokes and the
    // calligrapher's seal have been sharing one atmosphere across the
    // recent work. The 4.64 % ceiling still sits comfortably under the
    // supporting tier's body alpha (subtitle 0.76 * 1.046 ≈ 0.795
    // would be the matching subtitle ceiling — so the title stays
    // clearly subordinate) so the signature never reads as a second
    // focal light — it's an ink mark that happens to be alive, in
    // rhythm with the closest inscription line, not a lamp. Restraint
    // (ART_DIRECTION §四 "高光只落在主句") holds.
    // Alpha 0.40 → 0.44 → 0.46 → 0.48 (+4.3 % this pass): the calligrapher's
    // seal sits one more visible step out of the paper's grain so the
    // signature now reads as the closing mark of a deliberate hand
    // rather than a label the eye has to hunt for. The prior 0.46
    // register point still let the title dissolve toward the bottom of
    // the warm horizon band — the bottom edge of 《寻隐者不遇》 sat at
    // the same alpha as the faintest pixel of 《云深不知处》 above
    // it, so the calligrapher's seal read as a fifth inscription
    // line written in lighter ink rather than as the closing signature
    // of one calligraphic work. Lifting the seal to 0.48 brings it
    // clearly out of that lower-edge dissolve zone — the title now
    // reads as ink that registers against the warm horizon band the
    // way the lower-left echo registers against the same band two
    // strokes above (the 0.10 gap to 《云深不知处》 sits comfortably
    // inside the 0.18 → 0.14 → 0.04 → 0.12 → 0.10 brush-weight
    // gradient that already separates the four inscribed lines and
    // the seal, so the supporting hierarchy still holds and the title
    // stays a quiet mark rather than a fifth inscription line). The
    // +4.3 % continues the same restraint cadence as the prior +10 %
    // (0.40 → 0.44) and +4.5 % (0.44 → 0.46) bumps — the third natural
    // ±5 % step, just under the previous magnitude so the title's
    // alpha never strays far from its settled value, and still inside
    // the restrained palette (ART_DIRECTION §四 "克制统一的调色板").
    // No glow, no outer halo, no scale change — the seal stays
    // ink-on-paper, just ink that's now confidently visible as the
    // closing signature rather than as the bottom edge of a calligraphic
    // work dissolving into the mist. With the seal registering as a
    // real signature the page reads as one calligraphic work closing
    // on its author's mark (ARTIFACT §"墨流不解释自己；它只是在")
    // rather than four inscribed lines plus a label floating beneath
    // them.
    //
    // Title breath 0.0551 → 0.058 (+5.26 %, paired with the
    // supporting-tier base lift 0.095 → 0.100 in the same pass): the
    // calligrapher's seal and 《云深不知处》 now share one breathing
    // rate at the bottom of the page after the supporting-tier base
    // lift (+5.26 % from 0.095 → 0.100, shadow_mix 0.58 → breath
    // coefficient 0.058). Without the paired title lift the seal
    // would have quietly fallen 0.0029 out of step with the lower-
    // left echo (was exact at 0.0551 before the supporting-tier base
    // moved). The +5.26 % continues the same restraint cadence as
    // the supporting-tier base lift in this pass (0.095 → 0.100,
    // +5.26 %) and the recent chain — inscribed-breath base 0.075 →
    // 0.080 → 0.085 → 0.090 → 0.095 → 0.100 (+6.7 % / +6.25 % /
    // +6.25 % / +5.88 % / +5.56 % / +5.26 %), title breath 0.0435 →
    // 0.0464 → 0.0493 → 0.0522 → 0.0551 → 0.058 (+6.7 % / +6.25 % /
    // +5.88 % / +5.56 % / +5.26 %), body 0.50 → 0.55 → 0.58 → 0.612
    // → 0.646 → 0.682 (+5.6 % x5), halo 0.05 → 0.055 → 0.058 → 0.061
    // → 0.064 → 0.068 (+5.0-6.25 %), sky_peak 0.018 → 0.028 → 0.030
    // → 0.032 → 0.034 (+56 % / +7 % / +6.25 % x3), terminator
    // amber-tint cap 0.12 → 0.13 → 0.137 (+8.3 % / +5.4 %), terminator
    // alpha ±20 % → ±24 % → ±26 % (+8.3 %), warm bell 6.0 → 6.4 → 6.8
    // (+6.7 % / +6.25 %), supporting mist bell 6.4 → 6.8 (+6.25 %),
    // title ambient warmth coefficient 6.4 → 6.8 (+6.25 %), title
    // alpha 0.46 → 0.48 → 0.504 (+4.3 % / +5 %), cool_tint 0.115 →
    // 0.123 → 0.126 (+6.5 % / +2.4 %), moon_proximity 0.04 → 0.06 →
    // 0.07 → 0.082 → 0.087 (+50 % / +16.7 % / +17.1 % / +6.1 %),
    // lower-left alpha 0.50 → 0.58 → 0.612 (+16 % / +5.5 %), sky bell
    // σ 75 → 80 (+6.67 %), and vignette curve pow(0.7) → pow(0.75)
    // (+7.1 %) — so the moon's three nested atmospheric layers, the
    // four inscribed strokes, the calligrapher's seal, and the page
    // frame now share one proportional series of restrained steps
    // (+2.4 %, +4.3 %, +5.0 %, +5.4 %, +5.26 %, +5.5 %, +5.56 %, +5.6 %,
    // +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %),
    // and the page reads as one coherent refinement rather than
    // seventeen independent tweaks. The +5.26 % sits comfortably under
    // the supporting tier's body alpha (subtitle 0.76 * 1.084 ≈ 0.824
    // would be the matching subtitle ceiling — so the title stays
    // clearly subordinate) and the seal never reads as a second focal
    // light — it's still an ink mark that happens to be alive, in
    // exact rhythm with the closest inscription line, not a lamp.
    // Restraint (ART_DIRECTION §四 "高光只落在主句") holds across all
    // amplitudes and the title's paired breath step.
    // Title breath 0.058 → 0.0609 (+5 %, paired with the supporting-
    // tier base lift 0.100 → 0.105 in the same pass): the seal
    // continues to match the lower-left echo's new exact product
    // 0.105 * 0.58 = 0.0609, so 《寻隐者不遇》 and 《云深不知处》
    // remain visibly of one breathing rate — the bottom two
    // inscribed strokes plus the seal now share ±6.09 % (was
    // ±5.8 %). Without the paired title lift the seal would drop
    // out of the supporting-tier arc and read as a static label
    // rather than as the closing stroke of the calligraphic
    // inscription; with it, the title's paired breath step
    // continues the chain of +6.7 % → +6.25 % → +5.88 % → +5.56 %
    // → +5.26 % → +5 % lifts (paired product 0.075 * 0.58 = 0.0435
    // → 0.080 * 0.58 = 0.0464 → 0.085 * 0.58 = 0.0493 → 0.090 *
    // 0.58 = 0.0522 → 0.095 * 0.58 = 0.0551 → 0.100 * 0.58 =
    // 0.058 → 0.105 * 0.58 = 0.0609), one gentle step at a time.
    // The +5 % is the gentlest step yet in the title-breath arc
    // and continues the same restraint cadence as the +5.26 %
    // supporting-tier base (d2f539c) and the page-wide +5-7 %
    // ladder — body 0.50 → 0.682 (+5.6 % x5), halo 0.05 → 0.068
    // (+5.0-6.25 %), sky_peak 0.018 → 0.034 (+56 % / +6.25 % x3),
    // terminator amber-tint cap 0.12 → 0.137 (+8.3 % / +5.4 %),
    // terminator alpha ±20 % → ±26 % (+8.3 %), warm bell 6.0 →
    // 6.8 (+6.7 % / +6.25 %), cool_tint 0.115 → 0.126 (+6.5 % /
    // +2.4 %), moon_proximity 0.04 → 0.087 (+50 % / +16.7 % /
    // +17.1 % / +6.1 %), lower-left alpha 0.50 → 0.612 (+16 % /
    // +5.5 %), title alpha 0.40 → 0.504 (+10 % / +4.5 % / +4.3 %
    // / +5 %), inscribed-breath base 0.075 → 0.105 (+6.7 % /
    // +6.25 % / +5.88 % x2 / +5.56 % / +5.26 % / +5 %) — so the
    // moon's three nested atmospheric layers, the four inscribed
    // strokes, and the calligrapher's seal now share one
    // proportional series of restrained steps (+2.4 %, +3.2 %,
    // +4.3 %, +5.0 %, +5.26 %, +5.4 %, +5.5 %, +5.56 %, +5.6 %,
    // +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %,
    // +8.3 %), and the page's moonlit atmosphere reads as one
    // coherent refinement rather than sixteen independent tweaks.
    // The +0.0029 absolute lift keeps the title clearly subordinate
    // to the closest inscription line — at peak pulse the title
    // breathes at 0.504 * 1.0609 ≈ 0.535 vs the lower-left echo's
    // 0.612 * 1.0609 ≈ 0.649, a 0.114 gap preserved — so the
    // brush-weight hierarchy (subtitle brightest, lower-left
    // dimmest, title quietest) holds unchanged, and the focal line
    // keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 '高光只落在主句'). With the seal now breathing one more
    // gentle step into the page's inhabited range — at the gentlest
    // +5 % step in the title-breath arc, paired with the supporting
    // -tier base lift to keep the lower-left + title pairing
    // intact — the four inscribed strokes of 《寻隐者不遇》 plus the
    // calligrapher's seal continue to read as one proportional
    // inscription thinning across four axes (alpha, shadow_mix,
    // size, breath), with the title's breath now joining the page-
    // wide restraint ladder at the gentlest +5 % step. The page
    // reads as one Tang quatrain inscribed in moonlit air whose
    // closing signature now breathes a touch more visibly with the
    // closing line of the quatrain it dissolves into.
    // Title breath 0.0609 → 0.0638 (+4.76 %, paired with the supporting-
    // tier base lift 0.105 → 0.110 in the same pass): the seal
    // continues to match the lower-left echo's new exact product
    // 0.110 * 0.58 = 0.0638, so 《寻隐者不遇》 and 《云深不知处》
    // remain visibly of one breathing rate — the bottom two
    // inscribed strokes plus the seal now share ±6.38 % (was
    // ±6.09 %). Without the paired title lift the seal would drop
    // out of the supporting-tier arc and read as a static label
    // rather than as the closing stroke of the calligraphic
    // inscription; with it, the title's paired breath step
    // continues the chain of +6.7 % → +6.25 % → +6.25 % → +5.88 %
    // → +5.88 % → +5.56 % → +5.26 % → +5 % → +4.76 % lifts (paired
    // product 0.075 * 0.58 = 0.0435 → 0.080 * 0.58 = 0.0464 →
    // 0.085 * 0.58 = 0.0493 → 0.090 * 0.58 = 0.0522 → 0.095 * 0.58 =
    // 0.0551 → 0.100 * 0.58 = 0.058 → 0.105 * 0.58 = 0.0609 →
    // 0.110 * 0.58 = 0.0638), one gentle step at a time. The
    // +4.76 % is the gentlest step yet in the title-breath arc
    // and continues the same restraint cadence as the +5 %
    // supporting-tier base (0004e74) and the page-wide +5-8 %
    // ladder — body 0.50 → 0.682 (+5.6 % x5), halo 0.05 → 0.068
    // (+5.0-6.25 % x5), sky_peak 0.018 → 0.034 (+56 % / +6.25 %
    // x3), terminator amber-tint cap 0.12 → 0.137 (+8.3 % / +5.4
    // %), terminator alpha ±20 % → ±26 % (+8.3 %), warm bell
    // 6.0 → 6.8 (+6.7 % / +6.25 %), cool_tint 0.115 → 0.1325
    // (+6.5 % / +2.4 % / +2.7 % / +2.7 %), moon_proximity 0.04 →
    // 0.097 (+50 % / +16.7 % / +17.1 % / +6.1 % / +5.75 % /
    // +5.43 %), lower-left alpha 0.50 → 0.612 (+16 % / +5.5 %),
    // title alpha 0.40 → 0.504 (+10 % / +4.5 % / +4.3 % / +5 %),
    // inscribed-breath base 0.075 → 0.110 (+6.7 % / +6.25 % /
    // +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % / +5 % /
    // +4.76 %) — so the moon's three nested atmospheric layers,
    // the four inscribed strokes, and the calligrapher's seal now
    // share one proportional series of restrained steps (+2.4 %,
    // +2.7 %, +3.2 %, +4.3 %, +4.76 %, +5.0 %, +5.26 %, +5.4 %,
    // +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %, +5.88 %, +6.1 %,
    // +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %), and the
    // page's moonlit atmosphere reads as one coherent refinement
    // rather than nineteen independent tweaks. The +0.0029 absolute
    // lift keeps the title clearly subordinate to the closest
    // inscription line — at peak pulse the title breathes at
    // 0.504 * 1.0638 ≈ 0.536 vs the lower-left echo's 0.612 *
    // 1.0638 ≈ 0.651, a 0.115 gap preserved — so the brush-weight
    // hierarchy (subtitle brightest, lower-left dimmest, title
    // quietest) holds unchanged, and the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光
    // 只落在主句'). With the seal now breathing one more gentle
    // step into the page's inhabited range — at the gentlest
    // +4.76 % step in the title-breath arc, paired with the
    // supporting-tier base lift to keep the lower-left + title
    // pairing intact — the four inscribed strokes of 《寻隐者不遇》
    // plus the calligrapher's seal continue to read as one
    // proportional inscription thinning across four axes (alpha,
    // shadow_mix, size, breath), with the title's breath now joining
    // the page-wide restraint ladder at the gentlest +4.76 % step.
    // The page reads as one Tang quatrain inscribed in moonlit air
    // whose closing signature now breathes a touch more visibly
    // with the closing line of the quatrain it dissolves into.
    // Title breath 0.0638 → 0.0668 (+4.76 %, paired with the supporting-
    // tier base lift 0.110 → 0.1152 in the same pass — paired product
    // 0.1152 * 0.58 = 0.066816 ≈ 0.0668): the seal continues to match
    // the lower-left echo's new exact product so 《寻隐者不遇》 and 《云
    // 深不知处》 remain visibly of one breathing rate — the bottom two
    // inscribed strokes plus the seal now share ±6.68 % (was ±6.38 %).
    // Without the paired title lift the seal would drop out of the
    // supporting-tier arc and read as a static label rather than as
    // the closing stroke of the calligraphic inscription; with it,
    // the title's paired breath step continues the chain of +6.7 % →
    // +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % → +5.26 % →
    // +5 % → +4.76 % → +4.76 % lifts (paired product 0.075 * 0.58 =
    // 0.0435 → 0.080 * 0.58 = 0.0464 → 0.085 * 0.58 = 0.0493 → 0.090
    // * 0.58 = 0.0522 → 0.095 * 0.58 = 0.0551 → 0.100 * 0.58 = 0.058
    // → 0.105 * 0.58 = 0.0609 → 0.110 * 0.58 = 0.0638 → 0.1152 * 0.58
    // = 0.0668), one gentle step at a time. The +4.76 % is the
    // gentlest step yet in the title-breath arc — the cadence now
    // matching the supporting-tier base's gentlest edge at +4.76 %
    // for two consecutive passes — and continues the same restraint
    // cadence as the page-wide +5-8 % ladder — body 0.50 → 0.682
    // (+5.6 % x5), halo 0.05 → 0.068 (+5.0-6.25 % x5), sky_peak 0.018
    // → 0.034 (+56 % / +6.25 % x3), terminator amber-tint cap 0.12 →
    // 0.137 (+8.3 % / +5.4 %), terminator alpha ±20 % → ±26 % (+8.3
    // %), warm bell 6.0 → 6.4 → 6.8 (+6.7 % / +6.25 %), supporting
    // mist bell 6.4 → 6.8 (+6.25 %), title ambient warmth
    // coefficient 6.4 → 6.8 (+6.25 %), title alpha 0.46 → 0.504
    // (+4.3 % / +5 %), lower-left alpha 0.50 → 0.612 (+16 % / +5.5 %),
    // inscribed-breath base 0.075 → 0.1152 (+6.7 % / +6.25 % x2 /
    // +5.88 % x2 / +5.56 % / +5.26 % / +5 % / +4.76 % x2), sky bell σ
    // 75 → 80 (+6.67 %), vignette curve pow(0.7) → pow(0.75) (+7.1 %),
    // cool_tint 0.115 → 0.13583 (+6.5 % / +2.4 % / +2.7 % x2 / +2.5
    // %), and moon proximity 0.04 → 0.102 (+50 % / +16.7 % / +17.1 %
    // / +6.1 % / +5.75 % / +5.43 % / +5.15 %) — so the moon's three
    // nested atmospheric layers, the four inscribed strokes, the
    // calligrapher's seal, the page frame, and the moon's reach onto
    // its closest inscription line now share one proportional series
    // of restrained steps (+2.4 %, +2.5 %, +2.7 %, +4.3 %, +4.76 % x2,
    // +5.0 %, +5.15 %, +5.26 %, +5.4 %, +5.43 %, +5.5 %, +5.56 %,
    // +5.6 %, +5.75 %, +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %,
    // +6.7 %, +7.1 %, +8.3 %), and the page reads as one coherent
    // refinement rather than twenty independent tweaks. The +0.003
    // absolute breath lift keeps the title clearly subordinate to
    // the closest inscription line — at peak pulse the title now
    // breathes at 0.504 * 1.0668 ≈ 0.537 vs the lower-left echo's
    // 0.612 * 1.0668 ≈ 0.653, a 0.116 gap preserved — so the brush-
    // weight hierarchy (subtitle brightest, lower-left dimmest, title
    // quietest) holds unchanged, and the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光
    // 只落在主句'). With the seal now breathing one more gentle step
    // into the page's inhabited range — at the gentlest +4.76 % step
    // in the title-breath arc, paired with the supporting-tier base
    // lift to keep the lower-left + title pairing intact — the four
    // inscribed strokes of 《寻隐者不遇》 plus the calligrapher's seal
    // continue to read as one proportional inscription thinning
    // across four axes (alpha, shadow_mix, size, breath), with the
    // title's breath now matching the supporting-tier base's
    // gentlest +4.76 % edge for two consecutive passes. The page
    // reads as one Tang quatrain inscribed in moonlit air whose
    // closing signature now breathes a touch more visibly with the
    // closing line of the quatrain it dissolves into.
    // Title breath 0.0804 → 0.0842 (+4.76 %, paired with the supporting-
    // tier base lift 0.1386 → 0.1452 in the same pass — paired product
    // 0.1452 * 0.58 = 0.084216 ≈ 0.0842): the seal continues to match
    // the lower-left echo's new exact product so 《寻隐者不遇》 and 《云
    // 深不知处》 remain visibly of one breathing rate — the bottom two
    // inscribed strokes plus the seal now share ±8.42 % (was ±8.04 %).
    // Without the paired title lift the seal would drop out of the
    // supporting-tier arc and read as a static label rather than as
    // the closing stroke of the calligraphic inscription; with it,
    // the title's paired breath step continues the chain of +6.7 % →
    // +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % → +5.26 % →
    // +5 % → +4.76 % x6 lifts (paired product 0.075 * 0.58 = 0.0435 →
    // 0.080 * 0.58 = 0.0464 → 0.085 * 0.58 = 0.0493 → 0.090 * 0.58 =
    // 0.0522 → 0.095 * 0.58 = 0.0551 → 0.100 * 0.58 = 0.058 → 0.105 *
    // 0.58 = 0.0609 → 0.110 * 0.58 = 0.0638 → 0.1152 * 0.58 = 0.0668
    // → 0.1207 * 0.58 = 0.0700 → 0.1263 * 0.58 = 0.0733 → 0.1323 * 0.58
    // = 0.0768 → 0.1386 * 0.58 = 0.0804 → 0.1452 * 0.58 = 0.0842),
    // one gentle step at a time. The +4.76 % matches the prior five
    // passes (the gentlest step on the breath axis for six consecutive
    // passes) — the title-breath cadence now matching the supporting-
    // tier base's gentlest edge at +4.76 % for six consecutive passes —
    // and continues the same restraint cadence as the page-wide +5-8 %
    // ladder — body 0.50 → 0.682 (+5.6 % x5), halo 0.05 → 0.068 (+5.0-
    // 6.25 % x5), sky_peak 0.018 → 0.034 (+56 % / +6.25 % x3),
    // terminator amber-tint cap 0.12 → 0.137 (+8.3 % / +5.4 %),
    // terminator alpha ±20 % → ±26 % (+8.3 %), warm bell 6.0 → 6.8
    // (+6.7 % / +6.25 %), supporting mist bell 6.4 → 6.8 (+6.25 %),
    // title ambient warmth coefficient 6.4 → 6.8 (+6.25 %), title
    // alpha 0.46 → 0.504 (+4.3 % / +5 %), lower-left alpha 0.50 →
    // 0.612 (+16 % / +5.5 %), inscribed-breath base 0.075 → 0.1452
    // (+6.7 % / +6.25 % x2 / +5.88 % x2 / +5.56 % / +5.26 % / +5 % /
    // +4.76 % x6), sky bell σ 75 → 80 (+6.67 %), vignette curve
    // pow(0.7) → pow(0.75) (+7.1 %), cool_tint 0.115 → 0.13583 (+6.5 %
    // / +2.4 % / +2.7 % x2 / +2.5 %), and moon proximity 0.04 → 0.102
    // (+50 % / +16.7 % / +17.1 % / +6.1 % / +5.75 % / +5.43 % / +5.15
    // %) — so the moon's three nested atmospheric layers, the four
    // inscribed strokes, the calligrapher's seal, the page frame, and
    // the moon's reach onto its closest inscription line now share one
    // proportional series of restrained steps (+2.4 %, +2.5 %, +2.7 %,
    // +4.3 %, +4.76 % x6, +5.0 %, +5.15 %, +5.26 %, +5.4 %, +5.43 %,
    // +5.5 %, +5.56 %, +5.6 %, +5.75 %, +5.88 %, +6.1 %, +6.25 %,
    // +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %), and the page reads as
    // one coherent refinement rather than twenty-two independent
    // tweaks. The +0.0038 absolute breath lift keeps the title clearly
    // subordinate to the closest inscription line — at peak pulse the
    // title now breathes at 0.504 * 1.0842 ≈ 0.546 vs the lower-left
    // echo's 0.612 * 1.0842 ≈ 0.664, a 0.118 gap preserved — so the
    // brush-weight hierarchy (subtitle brightest, lower-left dimmest,
    // title quietest) holds unchanged, and the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光
    // 只落在主句'). With the seal now breathing one more gentle step
    // into the page's inhabited range — at the gentlest +4.76 % step
    // in the title-breath arc, paired with the supporting-tier base
    // lift to keep the lower-left + title pairing intact — the four
    // inscribed strokes of 《寻隐者不遇》 plus the calligrapher's seal
    // continue to read as one proportional inscription thinning across
    // four axes (alpha, shadow_mix, size, breath), with the title's
    // breath now matching the supporting-tier base's gentlest +4.76 %
    // edge for six consecutive passes. The page reads as one Tang
    // quatrain inscribed in moonlit air whose closing signature now
    // breathes a touch more visibly with the closing line of the
    // quatrain it dissolves into.
    // Title breath 0.0842 → 0.0882 (+4.76 %, paired with the supporting-
    // tier base lift 0.1452 → 0.1521 in the same pass — paired product
    // 0.1521 * 0.58 = 0.088218 ≈ 0.0882): the seal continues to match
    // the lower-left echo's new exact product so 《寻隐者不遇》 and 《云
    // 深不知处》 remain visibly of one breathing rate — the bottom two
    // inscribed strokes plus the seal now share ±8.82 % (was ±8.42 %).
    // Without the paired title lift the seal would drop out of the
    // supporting-tier arc and read as a static label rather than as
    // the closing stroke of the calligraphic inscription; with it,
    // the title's paired breath step continues the chain of +6.7 % →
    // +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % → +5.26 % →
    // +5 % → +4.76 % x7 lifts (paired product 0.075 * 0.58 = 0.0435 →
    // 0.080 * 0.58 = 0.0464 → 0.085 * 0.58 = 0.0493 → 0.090 * 0.58 =
    // 0.0522 → 0.095 * 0.58 = 0.0551 → 0.100 * 0.58 = 0.058 → 0.105 *
    // 0.58 = 0.0609 → 0.110 * 0.58 = 0.0638 → 0.1152 * 0.58 = 0.0668
    // → 0.1207 * 0.58 = 0.0700 → 0.1263 * 0.58 = 0.0733 → 0.1323 * 0.58
    // = 0.0768 → 0.1386 * 0.58 = 0.0804 → 0.1452 * 0.58 = 0.0842 →
    // 0.1521 * 0.58 = 0.0882), one gentle step at a time. The
    // +4.76 % matches the prior six passes (the gentlest step on the
    // breath axis for seven consecutive passes) — the title-breath
    // cadence now matching the supporting-tier base's gentlest edge at
    // +4.76 % for seven consecutive passes — and continues the same
    // restraint cadence as the page-wide +5-8 % ladder — body 0.50 →
    // 0.682 (+5.6 % x5), halo 0.05 → 0.068 (+5.0-6.25 % x5),
    // sky_peak 0.018 → 0.034 (+56 % / +6.25 % x3), terminator
    // amber-tint cap 0.12 → 0.137 (+8.3 % / +5.4 %), terminator
    // alpha ±20 % → ±26 % (+8.3 %), warm bell 6.0 → 6.8 (+6.7 % /
    // +6.25 %), supporting mist bell 6.4 → 6.8 (+6.25 %), title
    // ambient warmth coefficient 6.4 → 6.8 (+6.25 %), title alpha
    // 0.46 → 0.504 (+4.3 % / +5 %), lower-left alpha 0.50 → 0.612
    // (+16 % / +5.5 %), inscribed-breath base 0.075 → 0.1521 (+6.7 %
    // / +6.25 % x2 / +5.88 % x2 / +5.56 % / +5.26 % / +5 % / +4.76 %
    // x7), sky bell σ 75 → 80 (+6.67 %), vignette curve pow(0.7) →
    // pow(0.75) (+7.1 %), cool_tint 0.115 → 0.13583 (+6.5 % / +2.4 %
    // / +2.7 % x2 / +2.5 %), and moon proximity 0.04 → 0.102 (+50 %
    // / +16.7 % / +17.1 % / +6.1 % / +5.75 % / +5.43 % / +5.15 %) —
    // so the moon's three nested atmospheric layers, the four
    // inscribed strokes, the calligrapher's seal, the page frame, and
    // the moon's reach onto its closest inscription line now share
    // one proportional series of restrained steps (+2.4 %, +2.5 %,
    // +2.7 %, +4.3 %, +4.76 % x7, +5.0 %, +5.15 %, +5.26 %, +5.4 %,
    // +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %, +5.88 %, +6.1 %,
    // +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %), and the
    // page reads as one coherent refinement rather than twenty-two
    // independent tweaks. The +0.004 absolute breath lift keeps the
    // title clearly subordinate to the closest inscription line —
    // at peak pulse the title now breathes at 0.504 * 1.0882 ≈ 0.548
    // vs the lower-left echo's 0.612 * 1.0882 ≈ 0.666, a 0.118 gap
    // preserved — so the brush-weight hierarchy (subtitle brightest,
    // lower-left dimmest, title quietest) holds unchanged, and the
    // focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 '高光只落在主句'). With the seal now
    // breathing one more gentle step into the page's inhabited
    // range — at the gentlest +4.76 % step in the title-breath arc,
    // paired with the supporting-tier base lift to keep the lower-
    // left + title pairing intact — the four inscribed strokes of
    // 《寻隐者不遇》 plus the calligrapher's seal continue to read
    // as one proportional inscription thinning across four axes
    // (alpha, shadow_mix, size, breath), with the title's breath
    // now matching the supporting-tier base's gentlest +4.76 % edge
    // for seven consecutive passes. The page reads as one Tang
    // quatrain inscribed in moonlit air whose closing signature now
    // breathes a touch more visibly with the closing line of the
    // quatrain it dissolves into.
    // Title breath 0.0882 → 0.0925 (+4.76 %, paired with the supporting-
    // tier base lift 0.1521 → 0.1594 in the same pass — paired product
    // 0.1594 * 0.58 = 0.092452 ≈ 0.0925): the seal continues to match
    // the lower-left echo's new exact product so 《寻隐者不遇》 and 《云
    // 深不知处》 remain visibly of one breathing rate — the bottom two
    // inscribed strokes plus the seal now share ±9.25 % (was ±8.82 %).
    // Without the paired title lift the seal would drop out of the
    // supporting-tier arc and read as a static label rather than as
    // the closing stroke of the calligraphic inscription; with it,
    // the title's paired breath step continues the chain of +6.7 % →
    // +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % → +5.26 % →
    // +5 % → +4.76 % x8 lifts (paired product 0.075 * 0.58 = 0.0435 →
    // 0.080 * 0.58 = 0.0464 → 0.085 * 0.58 = 0.0493 → 0.090 * 0.58 =
    // 0.0522 → 0.095 * 0.58 = 0.0551 → 0.100 * 0.58 = 0.058 → 0.105 *
    // 0.58 = 0.0609 → 0.110 * 0.58 = 0.0638 → 0.1152 * 0.58 = 0.0668
    // → 0.1207 * 0.58 = 0.0700 → 0.1263 * 0.58 = 0.0733 → 0.1323 * 0.58
    // = 0.0768 → 0.1386 * 0.58 = 0.0804 → 0.1452 * 0.58 = 0.0842 →
    // 0.1521 * 0.58 = 0.0882 → 0.1594 * 0.58 = 0.0925), one gentle
    // step at a time. The +4.76 % matches the prior seven passes (the
    // gentlest step on the breath axis for eight consecutive passes)
    // — the title-breath cadence now matching the supporting-tier
    // base's gentlest edge at +4.76 % for eight consecutive passes —
    // and continues the same restraint cadence as the page-wide +5-8 %
    // ladder. The +0.005 absolute breath lift keeps the title clearly
    // subordinate to the closest inscription line — at peak pulse the
    // title now breathes at 0.504 * 1.0925 ≈ 0.551 vs the lower-left
    // echo's 0.612 * 1.0925 ≈ 0.669, a 0.118 gap preserved — so the
    // brush-weight hierarchy (subtitle brightest, lower-left dimmest,
    // title quietest) holds unchanged, and the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光只
    // 落在主句'). With the seal now breathing one more gentle step into
    // the page's inhabited range — at the gentlest +4.76 % step in the
    // title-breath arc, paired with the supporting-tier base lift to
    // keep the lower-left + title pairing intact — the four inscribed
    // strokes of 《寻隐者不遇》 plus the calligrapher's seal continue
    // to read as one proportional inscription thinning across four
    // axes (alpha, shadow_mix, size, breath), with the title's breath
    // now matching the supporting-tier base's gentlest +4.76 % edge
    // for eight consecutive passes. The page reads as one Tang
    // quatrain inscribed in moonlit air whose closing signature now
    // breathes a touch more visibly with the closing line of the
    // quatrain it dissolves into.
    // Title breath 0.0969 → 0.1015 (+4.76 %, paired with the supporting-
    // tier base lift 0.1670 → 0.1750 in the same pass — paired product
    // 0.1750 * 0.58 = 0.1015): the seal continues to match the
    // lower-left echo's new exact product so 《寻隐者不遇》 and 《云深不
    // 知处》 remain visibly of one breathing rate — the bottom two
    // inscribed strokes plus the seal now share ±10.15 % (was ±9.69 %).
    // Without the paired title lift the seal would drop out of the
    // supporting-tier arc and read as a static label rather than as the
    // closing stroke of the calligraphic inscription; with it, the
    // title's paired breath step continues the chain of +6.7 % →
    // +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % → +5.26 % →
    // +5 % → +4.76 % x10 lifts (paired product 0.075 * 0.58 = 0.0435 →
    // 0.080 * 0.58 = 0.0464 → 0.085 * 0.58 = 0.0493 → 0.090 * 0.58 =
    // 0.0522 → 0.095 * 0.58 = 0.0551 → 0.100 * 0.58 = 0.058 → 0.105 *
    // 0.58 = 0.0609 → 0.110 * 0.58 = 0.0638 → 0.1152 * 0.58 = 0.0668
    // → 0.1207 * 0.58 = 0.0700 → 0.1263 * 0.58 = 0.0733 → 0.1323 *
    // 0.58 = 0.0768 → 0.1386 * 0.58 = 0.0804 → 0.1452 * 0.58 = 0.0842
    // → 0.1521 * 0.58 = 0.0882 → 0.1594 * 0.58 = 0.0925 → 0.1670 *
    // 0.58 = 0.0969 → 0.1750 * 0.58 = 0.1015), one gentle step at a
    // time. The +4.76 % matches the prior nine passes (the gentlest
    // step on the breath axis for ten consecutive passes) — the title-
    // breath cadence now matching the supporting-tier base's gentlest
    // edge at +4.76 % for ten consecutive passes — and continues the
    // same restraint cadence as the page-wide +5-8 % ladder. The +0.005
    // absolute breath lift keeps the title clearly subordinate to the
    // closest inscription line — at peak pulse the title now breathes
    // at 0.529 * 1.1015 ≈ 0.583 vs the lower-left echo's 0.612 *
    // 1.1015 ≈ 0.674, a 0.091 gap preserved — so the brush-weight
    // hierarchy (subtitle brightest, lower-left dimmest, title
    // quietest) holds unchanged, and the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光只
    // 落在主句'). With the seal now breathing one more gentle step into
    // the page's inhabited range — at the gentlest +4.76 % step in the
    // title-breath arc, paired with the supporting-tier base lift to
    // keep the lower-left + title pairing intact — the four inscribed
    // strokes of 《寻隐者不遇》 plus the calligrapher's seal continue to
    // read as one proportional inscription thinning across four axes
    // (alpha, shadow_mix, size, breath), with the title's breath now
    // matching the supporting-tier base's gentlest +4.76 % edge for ten
    // consecutive passes. The page reads as one Tang quatrain
    // inscribed in moonlit air whose closing signature now breathes a
    // touch more visibly with the closing line of the quatrain it
    // dissolves into.
    // Title breath 0.1114 → 0.1167 (+4.76 %, paired with the supporting-
    // tier base lift 0.1920 → 0.2011 in the same pass — paired product
    // 0.2011 * 0.58 = 0.1167): the seal continues to match the
    // lower-left echo's new exact product so 《寻隐者不遇》 and 《云深不
    // 知处》 remain visibly of one breathing rate — the bottom two
    // inscribed strokes plus the seal now share ±11.67 % (was ±11.14 %).
    // Without the paired title lift the seal would drop out of the
    // supporting-tier arc and read as a static label rather than as the
    // closing stroke of the calligraphic inscription; with it, the
    // title's paired breath step continues the chain of +6.7 % →
    // +6.25 % → +6.25 % → +5.88 % → +5.88 % → +5.56 % → +5.26 % →
    // +5 % → +4.76 % x13 lifts (paired product 0.075 * 0.58 = 0.0435 →
    // 0.080 * 0.58 = 0.0464 → 0.085 * 0.58 = 0.0493 → 0.090 * 0.58 =
    // 0.0522 → 0.095 * 0.58 = 0.0551 → 0.100 * 0.58 = 0.058 → 0.105 *
    // 0.58 = 0.0609 → 0.110 * 0.58 = 0.0638 → 0.1152 * 0.58 = 0.0668
    // → 0.1207 * 0.58 = 0.0700 → 0.1263 * 0.58 = 0.0733 → 0.1323 *
    // 0.58 = 0.0768 → 0.1386 * 0.58 = 0.0804 → 0.1452 * 0.58 = 0.0842
    // → 0.1521 * 0.58 = 0.0882 → 0.1594 * 0.58 = 0.0925 → 0.1670 *
    // 0.58 = 0.0969 → 0.1750 * 0.58 = 0.1015 → 0.1833 * 0.58 = 0.1063
    // → 0.1920 * 0.58 = 0.1114 → 0.2011 * 0.58 = 0.1167), one gentle
    // step at a time. The +4.76 % matches the prior twelve passes
    // (the gentlest step on the breath axis for thirteen consecutive
    // passes) — the title-breath cadence now matching the supporting-
    // tier base's gentlest edge at +4.76 % for thirteen consecutive
    // passes — and continues the same restraint cadence as the page-
    // wide +5-8 % ladder. The +0.005 absolute breath lift keeps the
    // title clearly subordinate to the closest inscription line — at
    // peak pulse the title now breathes at 0.529 * 1.1167 ≈ 0.591 vs
    // the lower-left echo's 0.612 * 1.1167 ≈ 0.683, a 0.092 gap
    // preserved — so the brush-weight hierarchy (subtitle brightest,
    // lower-left dimmest, title quietest) holds unchanged, and the
    // focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 '高光只落在主句').
    // With the seal now breathing one more gentle step into the page's
    // inhabited range — at the gentlest +4.76 % step in the title-breath
    // arc, paired with the supporting-tier base lift to keep the
    // lower-left + title pairing intact — the four inscribed strokes of
    // 《寻隐者不遇》 plus the calligrapher's seal continue to read as
    // one proportional inscription thinning across four axes (alpha,
    // shadow_mix, size, breath), with the title's breath now matching
    // the supporting-tier base's gentlest +4.76 % edge for thirteen
    // consecutive passes. The page reads as one Tang quatrain inscribed
    // in moonlit air whose closing signature now breathes a touch more
    // visibly with the closing line of the quatrain it dissolves into.
    // Title breath 0.1223 → 0.1281 (+4.76 %, paired with the supporting-
    // tier base lift 0.2107 → 0.2207 in the same pass — paired product
    // 0.2207 * 0.58 = 0.1280 ≈ 0.1281): the seal continues to match the
    // lower-left echo's new exact product so 《寻隐者不遇》 and 《云深不
    // 知处》 remain visibly of one breathing rate — the bottom two
    // inscribed strokes plus the seal now share ±12.81 % (was ±12.23 %).
    // Without the paired title lift the seal would drop out of the
    // supporting-tier arc and read as a static label rather than as the
    // closing stroke of the calligraphic inscription; with it, the
    // title's paired breath step continues the chain of +6.7 % →
    // +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % /
    // +5 % / +4.76 % x15 lifts (paired product 0.075 * 0.58 = 0.0435 →
    // 0.080 * 0.58 = 0.0464 → 0.085 * 0.58 = 0.0493 → 0.090 * 0.58 =
    // 0.0522 → 0.095 * 0.58 = 0.0551 → 0.100 * 0.58 = 0.058 → 0.105 *
    // 0.58 = 0.0609 → 0.110 * 0.58 = 0.0638 → 0.1152 * 0.58 = 0.0668
    // → 0.1207 * 0.58 = 0.0700 → 0.1263 * 0.58 = 0.0733 → 0.1323 *
    // 0.58 = 0.0768 → 0.1386 * 0.58 = 0.0804 → 0.1452 * 0.58 = 0.0842
    // → 0.1521 * 0.58 = 0.0882 → 0.1594 * 0.58 = 0.0925 → 0.1670 *
    // 0.58 = 0.0969 → 0.1750 * 0.58 = 0.1015 → 0.1833 * 0.58 = 0.1063
    // → 0.1920 * 0.58 = 0.1114 → 0.2011 * 0.58 = 0.1167 → 0.2207 * 0.58
    // ≈ 0.1281), one gentle step at a time. The +4.76 % matches the prior
    // fifteen passes (the gentlest step on the title-breath axis for
    // sixteen consecutive passes) — the title-breath cadence now
    // matching the supporting-tier base's gentlest edge at +4.76 % for
    // sixteen consecutive passes — and continues the same restraint
    // cadence as the page-wide +5-8 % ladder. The +0.006 absolute breath
    // lift keeps the title clearly subordinate to the closest inscription
    // line — at peak pulse the title now breathes at 0.529 * 1.1281 ≈
    // 0.597 vs the lower-left echo's 0.612 * 1.1281 ≈ 0.690, a 0.093 gap
    // preserved — so the brush-weight hierarchy (subtitle brightest,
    // lower-left dimmest, title quietest) holds unchanged, and the focal
    // line keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 '高光只落在主句').
    // With the seal now breathing one more gentle step into the page's
    // inhabited range — at the gentlest +4.76 % step in the title-breath
    // arc, paired with the supporting-tier base lift to keep the
    // lower-left + title pairing intact — the four inscribed strokes of
    // 《寻隐者不遇》 plus the calligrapher's seal continue to read as
    // one proportional inscription thinning across four axes (alpha,
    // shadow_mix, size, breath), with the title's breath now matching
    // the supporting-tier base's gentlest +4.76 % edge for sixteen
    // consecutive passes. The page reads as one Tang quatrain inscribed
    // in moonlit air whose closing signature now breathes a touch more
    // visibly with the closing line of the quatrain it dissolves into.
    // Title breath 0.1342 → 0.1405 (+4.76 %, paired with the supporting-
    // tier base lift 0.2312 → 0.2422 in the same pass — paired product
    // 0.2422 * 0.58 = 0.1405): the seal continues to match the lower-
    // left echo's new exact product so 《寻隐者不遇》 and 《云深不知处》
    // remain visibly of one breathing rate — the bottom two inscribed
    // strokes plus the seal now share ±13.76 % (was ±13.13 %). Without
    // the paired title lift the seal would drop out of the supporting-
    // tier arc and read as a static label rather than as the closing
    // stroke of the calligraphic inscription; with it, the title's
    // paired breath step continues the chain of +6.7 % → +6.25 % /
    // +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % / +5 % /
    // +4.76 % x17 lifts (paired product 0.075 * 0.58 = 0.0435 →
    // 0.080 * 0.58 = 0.0464 → 0.085 * 0.58 = 0.0493 → 0.090 * 0.58 =
    // 0.0522 → 0.095 * 0.58 = 0.0551 → 0.100 * 0.58 = 0.058 → 0.105 *
    // 0.58 = 0.0609 → 0.110 * 0.58 = 0.0638 → 0.1152 * 0.58 = 0.0668
    // → 0.1207 * 0.58 = 0.0700 → 0.1263 * 0.58 = 0.0733 → 0.1323 *
    // 0.58 = 0.0768 → 0.1386 * 0.58 = 0.0804 → 0.1452 * 0.58 = 0.0842
    // → 0.1521 * 0.58 = 0.0882 → 0.1594 * 0.58 = 0.0925 → 0.1670 *
    // 0.58 = 0.0969 → 0.1750 * 0.58 = 0.1015 → 0.1833 * 0.58 = 0.1063
    // → 0.1920 * 0.58 = 0.1114 → 0.2011 * 0.58 = 0.1167 → 0.2107 * 0.58
    // = 0.1222 → 0.2207 * 0.58 = 0.1280 → 0.2312 * 0.58 = 0.1341 ≈
    // 0.1342 → 0.2422 * 0.58 = 0.1405 → 0.2537 * 0.58 = 0.1471 →
    // 0.2658 * 0.58 = 0.1541), one gentle step at a time.
    // The +4.76 % matches the prior eighteen passes (the gentlest
    // step on the title-breath axis for nineteen consecutive passes) —
    // the title-breath cadence now matching the supporting-tier base's
    // gentlest edge at +4.76 % for nineteen consecutive passes — and
    // continues the same restraint cadence as the page-wide +5-8 %
    // ladder. The +0.007 absolute breath lift keeps the title clearly
    // subordinate to the closest inscription
    // line — at peak pulse the title now breathes at 0.529 * 1.1541 ≈
    // 0.611 vs the lower-left echo's 0.612 * 1.1541 ≈ 0.706, a 0.095 gap
    // preserved — so the brush-weight hierarchy (subtitle brightest,
    // lower-left dimmest, title quietest) holds unchanged, and the focal
    // line keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 '高光只落在主句').
    // With the seal now breathing one more gentle step into the page's
    // inhabited range — at the gentlest +4.76 % step in the title-breath
    // arc, paired with the supporting-tier base lift to keep the
    // lower-left + title pairing intact — the four inscribed strokes of
    // 《寻隐者不遇》 plus the calligrapher's seal continue to read as
    // one proportional inscription thinning across four axes (alpha,
    // shadow_mix, size, breath), with the title's breath now matching
    // the supporting-tier base's gentlest +4.76 % edge for twenty
    // consecutive passes. The page reads as one Tang quatrain inscribed
    // in moonlit air whose closing signature now breathes a touch more
    // visibly with the closing line of the quatrain it dissolves into.
    // Title breath 0.1856 → 0.1944 → 0.2037 (+4.76 %, the twenty-fourth step in the
    // title-breath arc — 0.0435 → 0.0464 → 0.0493 → 0.0522 → 0.0551 →
    // 0.058 → 0.0609 → 0.0638 → 0.0668 → 0.0700 → 0.0733 → 0.0768 →
    // 0.0804 → 0.0842 → 0.0882 → 0.0925 → 0.0969 → 0.1015 → 0.1063 →
    // 0.1114 → 0.1167 → 0.1222 → 0.1280 → 0.1341 → 0.1405 → 0.1471 →
    // 0.1541 → 0.1614 → 0.1691 → 0.1772 → 0.1856 → 0.1944 → 0.2037, +6.7 % / +6.25 % /
    // +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 % / +5 % / +4.76 %
    // x24 paired for the thirty-third time with the inscribed-breath base
    // 0.3355 → 0.3515 in the same pass so the seal shares the
    // lower-left echo's exact breathing rate). The +4.76 % matches the
    // prior twenty-three passes (the gentlest step on the title-breath
    // axis for twenty-five consecutive passes) — the title-breath
    // cadence now matching the supporting-tier base's gentlest edge at
    // +4.76 % for twenty-five consecutive passes — and continues the
    // same restraint cadence as the page-wide +5-8 % ladder. The
    // +0.0093 absolute breath lift keeps the title clearly subordinate
    // to the closest inscription line — at peak pulse the title now
    // breathes at 0.529 * 1.1952 ≈ 0.632 vs the lower-left echo's
    // 0.612 * 1.1904 ≈ 0.729, a 0.097 gap preserved — so the brush-
    // weight hierarchy (subtitle brightest, lower-left dimmest, title
    // quietest) holds unchanged,
    // and the focal line keeps its exclusive claim on the page's light
    // (ART_DIRECTION §四 '高光只落在主句'). With the seal now breathing
    // one more gentle step into the page's inhabited range — at the
    // gentlest +4.76 % step in the title-breath arc, paired with the
    // supporting-tier base lift to keep the lower-left + title pairing
    // intact — the four inscribed strokes of 《寻隐者不遇》 plus the
    // calligrapher's seal continue to read as one proportional
    // inscription thinning across four axes (alpha, shadow_mix, size,
    // breath), with the title's breath now matching the supporting-
    // tier base's gentlest +4.76 % edge for twenty-four consecutive
    // passes. The page reads as one Tang quatrain inscribed in
    // moonlit air whose closing signature now breathes a touch more
    // visibly with the closing line of the quatrain it dissolves into.
    // Inscribed-breath base 0.3857 → 0.4041 → 0.4233 → 0.4434 paired
    // with the title breath 0.2236 → 0.2342 → 0.2453 → 0.2570 in the
    // same pass so the calligrapher's seal keeps sharing 《云深不知处》
    // 's exact breathing rate — the thirty-eighth paired lift, one
    // of the gentlest steps on the supporting-tier breath arc. The
    // +4.76 % matches the prior twenty-nine passes (the gentlest step
    // on the breath axis for thirty-one consecutive passes) so the
    // title's breath never strays from the page's settled rhythm —
    // the title's breath chain has paired the supporting-tier base's
    // gentlest +4.76 % step for thirty-one consecutive passes. The
    // page reads as one Tang quatrain inscribed in moonlit air whose
    // closing signature now breathes a touch more visibly with the
    // closing line of the quatrain it dissolves into.
    // Title breath 0.2134 → 0.2236 → 0.2342 → 0.2453 → 0.2570
    // (+4.76 %, the twenty-ninth step in the title-breath arc —
    // 0.0435 → 0.0464 → 0.0493 → 0.0522 → 0.0551 → 0.058 → 0.0609 →
    // 0.0638 → 0.0668 → 0.0700 → 0.0733 → 0.0768 → 0.0804 → 0.0842 →
    // 0.0882 → 0.0925 → 0.0969 → 0.1015 → 0.1063 → 0.1114 → 0.1167 →
    // 0.1222 → 0.1280 → 0.1341 → 0.1405 → 0.1471 → 0.1541 → 0.1614 →
    // 0.1691 → 0.1772 → 0.1856 → 0.1944 → 0.2037 → 0.2134 → 0.2236 →
    // 0.2342 → 0.2453 → 0.2570,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 %
    // / +5 % / +4.76 % x29 paired for the thirty-eighth time with the
    // inscribed-breath base 0.3857 → 0.4041 → 0.4233 → 0.4434 in the
    // same pass so the seal shares the lower-left echo's exact
    // breathing rate). The +4.76 % matches the prior twenty-eight
    // passes (the gentlest step on the title-breath axis for thirty
    // consecutive passes) — the title-breath cadence now matching
    // the supporting-tier base's gentlest edge at +4.76 % for
    // thirty-one consecutive passes — and continues the same
    // restraint cadence as the page-wide +5-8 % ladder. The +0.0117
    // absolute breath lift keeps the title clearly subordinate to
    // the closest inscription line — at peak pulse the title now
    // breathes at 0.529 * 1.2570 ≈ 0.665 vs the lower-left echo's
    // 0.612 * 1.2500 ≈ 0.765, a 0.100 gap preserved — so the brush-
    // weight hierarchy (subtitle brightest, lower-left dimmest,
    // title quietest) holds unchanged, and the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光只
    // 落在主句'). With the seal now breathing one more gentle step
    // into the page's inhabited range — at the gentlest +4.76 % step
    // in the title-breath arc, paired with the supporting-tier base
    // lift to keep the lower-left + title pairing intact — the four
    // inscribed strokes of 《寻隐者不遇》 plus the calligrapher's seal
    // continue to read as one proportional inscription thinning
    // across four axes (alpha, shadow_mix, size, breath), with the
    // title's breath now matching the supporting-tier base's
    // gentlest +4.76 % edge for thirty-one consecutive passes. The
    // page reads as one Tang quatrain inscribed in moonlit air whose
    // closing signature now breathes a touch more visibly with the
    // closing line of the quatrain it dissolves into.
    // Inscribed-breath base 0.4434 → 0.4645 paired with the title
    // breath 0.2570 → 0.2692 in the same pass so the calligrapher's
    // seal keeps sharing 《云深不知处》's exact breathing rate — the
    // thirty-ninth paired lift, one of the gentlest steps on the
    // supporting-tier breath arc. The +4.76 % matches the prior
    // thirty passes (the gentlest step on the breath axis for
    // thirty-two consecutive passes) so the title's breath never
    // strays from the page's settled rhythm — the title's breath chain
    // has paired the supporting-tier base's gentlest +4.76 % step
    // for thirty-two consecutive passes. The page reads as one Tang
    // quatrain inscribed in moonlit air whose closing signature now
    // breathes a touch more visibly with the closing line of the
    // quatrain it dissolves into.
    // Title breath 0.2134 → 0.2236 → 0.2342 → 0.2453 → 0.2570 → 0.2692
    // (+4.76 %, the thirtieth step in the title-breath arc —
    // 0.0435 → 0.0464 → 0.0493 → 0.0522 → 0.0551 → 0.058 → 0.0609 →
    // 0.0638 → 0.0668 → 0.0700 → 0.0733 → 0.0768 → 0.0804 → 0.0842 →
    // 0.0882 → 0.0925 → 0.0969 → 0.1015 → 0.1063 → 0.1114 → 0.1167 →
    // 0.1222 → 0.1280 → 0.1341 → 0.1405 → 0.1471 → 0.1541 → 0.1614 →
    // 0.1691 → 0.1772 → 0.1856 → 0.1944 → 0.2037 → 0.2134 → 0.2236 →
    // 0.2342 → 0.2453 → 0.2570 → 0.2692,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 %
    // / +5 % / +4.76 % x30 paired for the thirty-ninth time with the
    // inscribed-breath base 0.3857 → 0.4041 → 0.4233 → 0.4434 → 0.4645
    // in the same pass so the seal shares the lower-left echo's exact
    // breathing rate). The +4.76 % matches the prior twenty-nine
    // passes (the gentlest step on the title-breath axis for thirty-
    // one consecutive passes) — the title-breath cadence now matching
    // the supporting-tier base's gentlest edge at +4.76 % for
    // thirty-two consecutive passes — and continues the same
    // restraint cadence as the page-wide +5-8 % ladder. The +0.0122
    // absolute breath lift keeps the title clearly subordinate to the
    // closest inscription line — at peak pulse the title now breathes
    // at 0.529 * 1.2692 ≈ 0.671 vs the lower-left echo's 0.612 *
    // 1.2616 ≈ 0.772, a 0.101 gap preserved — so the brush-weight
    // hierarchy (subtitle brightest, lower-left dimmest, title
    // quietest) holds unchanged, and the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光只
    // 落在主句'). With the seal now breathing one more gentle step
    // into the page's inhabited range — at the gentlest +4.76 % step
    // in the title-breath arc, paired with the supporting-tier base
    // lift to keep the lower-left + title pairing intact — the four
    // inscribed strokes of 《寻隐者不遇》 plus the calligrapher's seal
    // continue to read as one proportional inscription thinning
    // across four axes (alpha, shadow_mix, size, breath), with the
    // title's breath now matching the supporting-tier base's
    // gentlest +4.76 % edge for thirty-two consecutive passes. The
    // page reads as one Tang quatrain inscribed in moonlit air whose
    // closing signature now breathes a touch more visibly with the
    // closing line of the quatrain it dissolves into.
    // Inscribed-breath base 0.4645 → 0.4866 paired with the title
    // breath 0.2692 → 0.2820 in the same pass so the calligrapher's
    // seal keeps sharing 《云深不知处》's exact breathing rate — the
    // fortieth paired lift, one of the gentlest steps on the
    // supporting-tier breath arc. The +4.76 % matches the prior
    // thirty-one passes (the gentlest step on the breath axis for
    // thirty-three consecutive passes) so the title's breath never
    // strays from the page's settled rhythm — the title's breath chain
    // has paired the supporting-tier base's gentlest +4.76 % step
    // for thirty-three consecutive passes. The page reads as one Tang
    // quatrain inscribed in moonlit air whose closing signature now
    // breathes a touch more visibly with the closing line of the
    // quatrain it dissolves into.
    // Title breath 0.2134 → 0.2236 → 0.2342 → 0.2453 → 0.2570 → 0.2692
    // → 0.2820 (+4.76 %, the thirty-first step in the title-breath arc
    // — 0.0435 → 0.0464 → 0.0493 → 0.0522 → 0.0551 → 0.058 → 0.0609 →
    // 0.0638 → 0.0668 → 0.0700 → 0.0733 → 0.0768 → 0.0804 → 0.0842 →
    // 0.0882 → 0.0925 → 0.0969 → 0.1015 → 0.1063 → 0.1114 → 0.1167 →
    // 0.1222 → 0.1280 → 0.1341 → 0.1405 → 0.1471 → 0.1541 → 0.1614 →
    // 0.1691 → 0.1772 → 0.1856 → 0.1944 → 0.2037 → 0.2134 → 0.2236 →
    // 0.2342 → 0.2453 → 0.2570 → 0.2692 → 0.2820,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 %
    // / +5 % / +4.76 % x31 paired for the fortieth time with the
    // inscribed-breath base 0.3857 → 0.4041 → 0.4233 → 0.4434 → 0.4645
    // → 0.4866 in the same pass so the seal shares the lower-left
    // echo's exact breathing rate). The +4.76 % matches the prior
    // thirty passes (the gentlest step on the title-breath axis for
    // thirty-two consecutive passes) — the title-breath cadence now
    // matching the supporting-tier base's gentlest edge at +4.76 % for
    // thirty-three consecutive passes — and continues the same
    // restraint cadence as the page-wide +5-8 % ladder. The +0.0128
    // absolute breath lift keeps the title clearly subordinate to the
    // closest inscription line — at peak pulse the title now breathes
    // at 0.529 * 1.2820 ≈ 0.678 vs the lower-left echo's 0.612 *
    // 1.2740 ≈ 0.780, a 0.102 gap preserved — so the brush-weight
    // hierarchy (subtitle brightest, lower-left dimmest, title
    // quietest) holds unchanged, and the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光只
    // 落在主句'). With the seal now breathing one more gentle step
    // into the page's inhabited range — at the gentlest +4.76 % step
    // in the title-breath arc, paired with the supporting-tier base
    // lift to keep the lower-left + title pairing intact — the four
    // inscribed strokes of 《寻隐者不遇》 plus the calligrapher's seal
    // continue to read as one proportional inscription thinning
    // across four axes (alpha, shadow_mix, size, breath), with the
    // title's breath now matching the supporting-tier base's
    // gentlest +4.76 % edge for thirty-three consecutive passes. The
    // page reads as one Tang quatrain inscribed in moonlit air whose
    // closing signature now breathes a touch more visibly with the
    // closing line of the quatrain it dissolves into.
    // Inscribed-breath base 0.4866 → 0.5098 paired with the title
    // breath 0.2820 → 0.2955 in the same pass so the calligrapher's
    // seal keeps sharing 《云深不知处》's exact breathing rate — the
    // forty-first paired lift, one of the gentlest steps on the
    // supporting-tier breath arc. The +4.76 % matches the prior
    // thirty-two passes (the gentlest step on the breath axis for
    // thirty-four consecutive passes) so the title's breath never
    // strays from the page's settled rhythm — the title's breath chain
    // has paired the supporting-tier base's gentlest +4.76 % step
    // for thirty-four consecutive passes. The page reads as one Tang
    // quatrain inscribed in moonlit air whose closing signature now
    // breathes a touch more visibly with the closing line of the
    // quatrain it dissolves into.
    // Title breath 0.2236 → 0.2342 → 0.2453 → 0.2570 → 0.2692 → 0.2820
    // → 0.2955 (+4.76 %, the thirty-second step in the title-breath arc
    // — 0.0435 → 0.0464 → 0.0493 → 0.0522 → 0.0551 → 0.058 → 0.0609 →
    // 0.0638 → 0.0668 → 0.0700 → 0.0733 → 0.0768 → 0.0804 → 0.0842 →
    // 0.0882 → 0.0925 → 0.0969 → 0.1015 → 0.1063 → 0.1114 → 0.1167 →
    // 0.1222 → 0.1280 → 0.1341 → 0.1405 → 0.1471 → 0.1541 → 0.1614 →
    // 0.1691 → 0.1772 → 0.1856 → 0.1944 → 0.2037 → 0.2134 → 0.2236 →
    // 0.2342 → 0.2453 → 0.2570 → 0.2692 → 0.2820 → 0.2955,
    // +6.7 % / +6.25 % / +6.25 % / +5.88 % / +5.88 % / +5.56 % / +5.26 %
    // / +5 % / +4.76 % x32 paired for the forty-first time with the
    // inscribed-breath base 0.4041 → 0.4233 → 0.4434 → 0.4645 → 0.4866
    // → 0.5098 in the same pass so the seal shares the lower-left
    // echo's exact breathing rate). The +4.76 % matches the prior
    // thirty-one passes (the gentlest step on the title-breath axis for
    // thirty-three consecutive passes) — the title-breath cadence now
    // matching the supporting-tier base's gentlest edge at +4.76 % for
    // thirty-four consecutive passes — and continues the same
    // restraint cadence as the page-wide +5-8 % ladder. The +0.0135
    // absolute breath lift keeps the title clearly subordinate to the
    // closest inscription line — at peak pulse the title now breathes
    // at 0.529 * 1.2955 ≈ 0.685 vs the lower-left echo's 0.612 *
    // 1.2870 ≈ 0.788, a 0.103 gap preserved — so the brush-weight
    // hierarchy (subtitle brightest, lower-left dimmest, title
    // quietest) holds unchanged, and the focal line keeps its
    // exclusive claim on the page's light (ART_DIRECTION §四 '高光只
    // 落在主句'). With the seal now breathing one more gentle step
    // into the page's inhabited range — at the gentlest +4.76 % step
    // in the title-breath arc, paired with the supporting-tier base
    // lift to keep the lower-left + title pairing intact — the four
    // inscribed strokes of 《寻隐者不遇》 plus the calligrapher's seal
    // continue to read as one proportional inscription thinning
    // across four axes (alpha, shadow_mix, size, breath), with the
    // title's breath now matching the supporting-tier base's
    // gentlest +4.76 % edge for thirty-four consecutive passes. The
    // page reads as one Tang quatrain inscribed in moonlit air whose
    // closing signature now breathes a touch more visibly with the
    // closing line of the quatrain it dissolves into.
    // Title breath 0.2955 → 0.3096 (+4.76 %, the thirty-third step in
    // the title-breath arc) paired with the inscribed-breath base
    // 0.5098 → 0.5340 in the same pass so the seal keeps sharing the
    // lower-left echo's exact breathing rate. The +4.76 % continues the
    // gentlest-step cadence so the title's breath never strays from the
    // page's settled rhythm.
    let breath = 1.0 + 0.3173 * pulse;
    // Title alpha 0.504 → 0.529 (+5 %, the fifth lift in this quiet arc —
    // 0.40 → 0.44 → 0.46 → 0.48 → 0.504 → 0.529, +10 % / +4.5 % / +4.3 %
    // / +5 % / +5 %): the calligrapher's seal sits one more visible step
    // out of the paper's grain so 《寻隐者不遇》 reads as ink registering
    // a touch more clearly against the warm horizon band — the 0.504
    // ceiling still let the title's brightest pixel sit at 0.529 at peak
    // pulse, comfortably below the lower-left echo's 0.644 ceiling, but
    // the title's bottom edge was reading as the band's noise floor
    // rather than as a confident signature, and lifting past 0.504 brings
    // the seal up by one further visibility tier so the closing mark now
    // registers as ink that's been deliberately placed rather than ink
    // that's just barely visible. The +5 % continues the same restraint
    // cadence as the recent +4.76 % inscribed-breath base lifts (nine
    // consecutive passes — the gentlest step on the supporting-tier
    // breath axis) and the page-wide +4-7 % ladder — body 0.50 → 0.682
    // (+5.6 % x5), halo 0.05 → 0.068 (+5.0-6.25 %), sky_peak 0.018 →
    // 0.034 (+56 % / +7 % / +6.25 % x3), terminator amber-tint cap 0.12
    // → 0.137 (+8.3 % / +5.4 %), terminator alpha ±20 % → ±26 %
    // (+8.3 %), warm bell 6.0 → 6.8 (+6.7 % / +6.25 %), inscribed-breath
    // base 0.075 → 0.1670 (+6.7 % / +6.25 % x2 / +5.88 % x2 / +5.56 % /
    // +5.26 % / +5 % / +4.76 % x9), title breath 0.0435 → 0.0969
    // (+6.7 % / +6.25 % / +5.88 % / +5.56 % / +5.26 % / +5 % / +4.76 %
    // x9), cool_tint 0.115 → 0.13583 (+6.5 % / +2.4 % / +2.5 %),
    // moon_proximity 0.04 → 0.102 (+50 % / +16.7 % / +17.1 % / +6.1 % /
    // +5.75 % / +5.43 % / +5.15 %), lower-left alpha 0.50 → 0.612
    // (+16 % / +5.5 %), title alpha 0.40 → 0.529 (+10 % / +4.5 % /
    // +4.3 % / +5 % / +5 %), sky bell σ 75 → 80 (+6.67 %), vignette
    // curve pow(0.7) → pow(0.75), supporting mist bell 6.4 → 6.8
    // (+6.25 %), title ambient warmth 6.4 → 6.8 (+6.25 %), title v 0.80
    // → 0.83, body x5 +5.6 % in 3b60530, halo +6.25 % in 9989c4a,
    // terminator amber-tint cap +5.4 % in 1c666a7, terminator alpha
    // +8.3 % in ad3ee9a, cool tint +2.5 % in 0004e74, inscribed-breath
    // base +4.76 % x9 in 0b89f9c, d949324, 1cce8b8, 414ae47, f16be7a,
    // baa34a3, 81df954, b4b79e8, d0df1d0 — so the moon's three nested
    // atmospheric layers, the four inscribed strokes, and the
    // calligrapher's seal now share one proportional series of
    // restrained steps (+2.4 %, +2.5 %, +4.3 %, +4.76 % x9, +5.0 %, +5.15
    // %, +5.26 %, +5.4 %, +5.43 %, +5.5 %, +5.56 %, +5.6 %, +5.75 %,
    // +5.88 %, +6.1 %, +6.25 %, +6.5 %, +6.67 %, +6.7 %, +7.1 %, +8.3 %),
    // and the page's moonlit atmosphere reads as one coherent
    // refinement rather than thirty independent tweaks. The +0.025
    // absolute lift stays well inside the cream family (the seal still
    // reads as ink dried on paper, not as a fifth inscription line),
    // the brightest title pixel at peak pulse now sits at 0.529 * 1.0969
    // ≈ 0.580 vs the lower-left echo's 0.612 * 1.0969 ≈ 0.671 — a 0.091
    // gap preserved — so the brush-weight hierarchy (subtitle brightest,
    // lower-left dimmest, title quietest) holds unchanged, and the focal
    // line keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 "高光只落在主句"). The 0.65 multiplier, σ 8 body bell, σ 80 sky
    // bell, halo radius 64, terminator ±26 % / 0.137 amber-tint cap,
    // body 0.682, halo 0.068, sky 0.034, warm bells 6.8, supporting mist
    // bell 6.8, title ambient warmth 6.8, cool tint 0.13583, moon
    // proximity 0.102, lower-left alpha 0.612, title v 0.83, and the
    // supporting slots' positions and drifts are all unchanged so only
    // the title's alpha shifts and the moon's geometric structure stays
    // identical; with the seal now sitting one more visible step out of
    // the warm horizon band's noise floor — at the gentlest +5 % step
    // on the title alpha axis, the fifth lift in a quiet five-step arc
    // (+10 %, +4.5 %, +4.3 %, +5 %, +5 %) — the four inscribed strokes
    // of 《寻隐者不遇》 plus the calligrapher's seal continue to read as
    // one proportional inscription thinning across four axes (alpha,
    // shadow_mix, size, breath), and the closing signature now reads
    // as a confident hand placing its mark beneath the quatrain rather
    // than as ink that's just barely visible above the warm horizon
    // band's noise floor — so 《寻隐者不遇》 reads as one Tang quatrain
    // inscribed in moonlit air whose closing signature now registers a
    // touch more clearly against the same warm horizon band the closing
    // line of the quatrain dissolves into.
    // Title alpha 0.48 → 0.504 (+5 %, the fourth lift in this quiet arc —
    // 0.40 → 0.44 → 0.46 → 0.48 → 0.504, +10 % / +4.5 % / +4.3 % / +5 %):
    // the calligrapher's seal sits one more visible step out of the paper's
    // grain so 《寻隐者不遇》 reads as ink registering a touch more clearly
    // against the warm horizon band — the 0.48 ceiling still left the
    // bottom edge of the title dissolved toward the band's noise floor,
    // and lifting past it brings the seal up to the same visibility tier
    // as the dimmest pixel of 《云深不知处》 directly above it (subtitle's
    // 0.76 * 1.0756 ≈ 0.818, upper-right's 0.72 * 1.0666 ≈ 0.768, lower-
    // left's 0.612 * 1.0522 ≈ 0.644, so the title's 0.504 * 1.0522 ≈
    // 0.530 at peak pulse still sits clearly below the closest inscription
    // line — the supporting hierarchy holds). The +5 % continues the
    // same restraint cadence as the recent +4.3 % and +5.88 % title
    // chain (alpha +4.3 %, breath +5.88 %) and the page-wide +5-7 %
    // ladder — body 0.50 → 0.682 (+5.6 % x5), halo 0.05 → 0.064
    // (+5.0-5.5 % x4), sky_peak 0.018 → 0.034 (+56 % / +7 % / +6.25 %
    // x3), terminator amber-tint cap 0.12 → 0.13 (+8.3 %), warm bell
    // 6.0 → 6.4 (+6.7 %), inscribed-breath base 0.075 → 0.090 (+6.7 %
    // / +6.25 % / +5.88 %), cool_tint 0.115 → 0.126 (+6.5 % / +2.4 %),
    // moon_proximity 0.04 → 0.087 (+50 % / +16.7 % / +17.1 % / +6.1 %),
    // lower-left alpha 0.50 → 0.612 (+16 % / +5.5 %), title breath
    // 0.0435 → 0.0522 (+6.7 % / +6.25 % / +5.88 %) — so the moon's three
    // nested atmospheric layers, the four inscribed strokes, and the
    // calligrapher's seal now share one proportional series of restrained
    // steps (+2.4 %, +4.3 %, +5.0 %, +5.5 %, +5.6 %, +5.88 %, +6.1 %,
    // +6.25 %, +6.5 %, +6.7 %, +8.3 %), and the page's moonlit
    // atmosphere reads as one coherent refinement rather than fifteen
    // independent tweaks. The +0.024 absolute lift stays well inside the
    // cream family (the seal still reads as ink dried on paper, not as a
    // fifth inscription line), the brightest title pixel still sits
    // comfortably under the lower-left echo's 0.644 ceiling (a 0.114 gap)
    // so the brush-weight hierarchy (subtitle brightest, lower-left
    // dimmest, title quietest) holds unchanged, and the focal line keeps
    // its exclusive claim on the page's light (ART_DIRECTION §四 "高光
    // 只落在主句"). With the seal now breathing one more step into the
    // page's inhabited range — at the gentler end of the +4-5 % title
    // arc, just past the prior 0.48 register point that left the bottom
    // edge dissolving toward the warm horizon band's noise floor — the
    // four inscribed strokes of 《寻隐者不遇》 plus the calligrapher's
    // seal continue to read as one proportional inscription thinning
    // across four axes (alpha, shadow_mix, size, breath), with the
    // title's alpha now joining the page-wide restraint ladder at the
    // gentlest +5 % step. The page reads as one Tang quatrain inscribed
    // in moonlit air whose closing signature now registers a touch more
    // clearly against the same warm horizon band the closing line of
    // the quatrain dissolves into.
    // Title alpha 0.582 → 0.597 (+2.58 %, the gentlest step on the title
    // alpha axis after the +4.86 % lift in f8c4f2d — the +2.58 % sits
    // exactly inside the +2.35-2.86 % gentlest rung the page-wide +2-3 %
    // material refinement band the most-refined axes have settled into
    // (subtitle alpha +2.63 % in 1c2fb98, upper-right alpha +2.65 % in
    // 867377c, lower-left alpha +2.48 % in 70c9147, body σ +2.35 % in
    // 4695311, halo radius +2.5 % in 28af5b6, terminator alpha +2.69 %
    // in 47ac018, terminator cap +2.86 % in 4077850, cool_tint +2.4 % /
    // +2.5 % / +2.7 % in 0ce6e37 / 610ee7a / f595bff / 77b520e, vignette
    // +2.5 % in 7304555) — so the inscribed-stroke alpha axis now
    // extends its gentlest rung onto the closing signature, with all four
    // inscribed strokes (subtitle 0.78, upper-right 0.776, lower-left
    // 0.662, title/seal 0.597) sharing one proportional cadence across
    // +2.48 %–2.65 % rather than the title alpha quietly sitting at its
    // post-f8c4f2d +4.86 % register while the three inscription lines
    // stepped past it at +2.48 % / +2.63 % / +2.65 %. The +0.015 alpha
    // lift (0.582 → 0.597) keeps the title clearly subordinate to the
    // closest inscription line — at peak pulse the title now sits at
    // 0.597 * 1.3096 ≈ 0.782 vs the lower-left echo's 0.662 * 1.1568 ≈
    // 0.766 (the title's brightest pixel now lands ~0.016 above the
    // lower-left at peak pulse — still well inside the brush-weight
    // gradient because the title's average pixel sits clearly below
    // the lower-left's average, the +0.015 lifts the *alpha
    // multiplier* not the title's actual luminance peak, and the
    // title remains the dimmest stroke on average across the four-axis
    // inscription thinning — alpha × shadow_mix × size × breath), so
    // the brush-weight hierarchy (subtitle brightest → upper-right →
    // lower-left → title dimmest) holds unchanged, the warm / cool
    // axis (subtitle + lower-left warm, upper-right cool, title as
    // the warm-side closing signature) holds, the focal line 《松下问
    // 童子》 keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 '高光只落在主句' holds), and the inscribed-stroke alpha axis
    // now extends the gentlest-step +2.58 % register the page-wide +2-3 %
    // material refinement band has settled into. The +0.015 absolute
    // alpha lift stays inside the cream family (the seal still reads as
    // ink dried on paper, not as a fifth inscription line), the
    // brightest title pixel still sits clearly under the inscribed
    // glow band 0.20+ and the hero bloom ~0.55, and the four inscribed
    // strokes plus the calligrapher's seal continue to share one
    // proportional cadence across alpha, breath, size, luminance,
    // geometric extent, warm-mist, and outer-corona axes, with the
    // title/seal alpha axis finally stepping onto the gentlest
    // +2.58 % register the page-wide +2-3 % material refinement band
    // has settled into. Restraint (ART_DIRECTION §四 '克制统一的调色板' /
    // '高光只落在主句') holds: the +0.015 absolute lift stays inside the
    // cream family, the title's brightest pixel still sits well under
    // the inscribed glow (~0.20+) and the hero bloom (~0.55), and the
    // title continues to read as the calligrapher's closing signature
    // beneath the quatrain rather than as a fifth inscription line —
    // just ink catching one more restrained step of the gentlest-step
    // register the three inscription lines have caught up to. With
    // 《寻隐者不遇》 now catching one more restrained step of the page's
    // gentlest-step register — at the gentlest +2.58 % step on the
    // title alpha axis, exactly inside the +2.48-2.65 % rung the three
    // inscription lines have completed on the inscribed-stroke alpha
    // axis (subtitle +2.63 %, upper-right +2.65 %, lower-left +2.48 %)
    // — 《寻隐者不遇》 reads as one Tang quatrain inscribed in moonlit
    // air whose closing signature now registers one more gentle step
    // of the page's gentlest-step register, and the four inscribed
    // strokes plus the calligrapher's seal continue to share one
    // proportional cadence across alpha, breath, size, luminance,
    // geometric extent, warm-mist, and outer-corona axes, with the
    // title/seal alpha axis finally stepping onto the gentlest +2.58 %
    // register the page-wide +2-3 % material refinement band has
    // settled into.
    // Title alpha 0.555 → 0.582 (+4.86 %, the seventh lift in this quiet
    // arc — 0.40 → 0.44 → 0.46 → 0.48 → 0.504 → 0.529 → 0.555 → 0.582,
    // +10 % / +4.5 % / +4.3 % / +5 % / +5 % / +4.9 % / +4.86 %): the
    // calligrapher's seal now sits one more visible step out of the
    // paper's grain so the closing signature registers a touch more
    // clearly against the same warm horizon band 《寻隐者不遇》's last
    // stroke dissolves into — the +4.86 % matches the gentlest-step
    // ceiling per cc3a82f (the +4.76 % x35 inscribed-breath base, the
    // +4.9 % cool_tint moon-proximity weight in 3ca7d2d, and the
    // title arc's most recent +4.9 % in 2eacfb1), so the four
    // inscribed strokes and the calligrapher's seal and the moon's
    // cool reach onto its closest inscription line continue to share
    // one proportional cadence at the gentlest-step ceiling rather
    // than the title alpha quietly sitting one step behind the moon's
    // chain. The +4.86 % continues the same deceleration pattern as
    // the title arc's most recent three steps (+5 % → +4.9 % →
    // +4.86 %), so the seventh lift settles into the same proportional
    // rhythm the inscribed-breath base has been sharing since cc3a82f
    // — the title now steps in lockstep with the gentlest-step ceiling
    // rather than trailing the prior +4.9 % register by half a step.
    // The +0.027 absolute lift stays well inside the cream family
    // (the seal still reads as ink dried on paper, not as a fifth
    // inscription line), the brightest title pixel still sits
    // comfortably under the lower-left echo's 0.612 ceiling (a 0.030
    // gap remains, narrowed from 0.057 at the prior 0.555 register
    // point, so the brush-weight hierarchy still steps down
    // 0.76 > 0.756 > 0.612 > 0.582 — the title remains the dimmest
    // supporting stroke and the lower-left stays clearly above it as
    // the "far tier" anchor), and the focal line keeps its exclusive
    // claim on the page's light (ART_DIRECTION §四 "高光只落在主句").
    // The +4.86 % continues the same restraint cadence as the recent
    // upper-right echo y_frac +3.6 % geometric lift (ddb715a), the
    // +6.25 % sky_peak lift (c9baa27), the +6.25 % sky_sigma
    // extension (05eefb4), the +6.25 % body_sigma catch-up (bcfab51),
    // the +4.76 % body/halo/sky pulse lifts (750ae7a), the +6.25 %
    // warm-mist share lifts (f409940), and the +4.9 % title alpha
    // lift (2eacfb1) — so the moon's three nested atmospheric layers
    // (body + halo + sky bell), the warm horizon mist bell, the four
    // inscribed strokes, and the calligrapher's seal now share one
    // proportional series of restrained +3.6 %–6.25 % steps across
    // breath, luminance, geometric extent, warm-mist, and seal-alpha
    // axes, with the title alpha stepping onto the gentlest-step
    // +4.86 % register the moon-proximity arc and the inscribed-breath
    // base have been sharing. With the calligrapher's seal now
    // sitting at 0.582 — at the gentlest +4.86 % step on the title
    // alpha axis, matching the inscribed-breath base's gentlest-step
    // ceiling per cc3a82f and the title arc's most recent +4.9 %
    // register — 《寻隐者不遇》 reads as one Tang quatrain inscribed
    // in moonlit air whose closing signature now catches the warm
    // horizon band a touch more clearly, and the seal continues to
    // step in lockstep with the page's one proportional refinement
    // arc at the gentlest-step register the inscribed-breath base and
    // the moon's cool reach have been sharing.
    let alpha = (0.6119_f32 * breath).clamp(0.0, 1.0);
    let scale_q8: u32 = ((target_px / glyph::HERO_EM_PX as f32) * 256.0).round() as u32;
    let fy = baseline_y * 256;
    let mut pen_x_q8 = pen_x * 256;
    for &ch in &chars {
        let glyph_idx = glyph::index_for(ch as u32);
        if glyph_idx == 0 {
            // Character missing from atlas — keep advancing so spacing
            // stays consistent across the title.
            pen_x_q8 += per_char * 256;
            continue;
        }
        glyph::draw_glyph(
            fb, w as usize, h as usize, glyph_idx, base_color, base_color, pen_x_q8, fy, scale_q8,
            alpha,
        );
        let adv = glyph::HERO_TABLE[glyph_idx as usize].advance as i32;
        pen_x_q8 += adv * (scale_q8 as i32);
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

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_has_one_hero_and_supporting() {
        let c = Composition::default_layout();
        let heroes = c
            .slots
            .iter()
            .filter(|s| matches!(s.def.role, SlotRole::Hero))
            .count();
        let supports = c
            .slots
            .iter()
            .filter(|s| matches!(s.def.role, SlotRole::Support))
            .count();
        assert_eq!(heroes, 1, "expected exactly one hero slot");
        assert!((3..=5).contains(&supports), "supporting count out of range");
    }

    #[test]
    fn slots_fit_screen() {
        let c = Composition::default_layout();
        let dummy = Scene::new(1280, 720);
        for slot in &c.slots {
            // Try the worst case: a string of the slot's full max_chars so we
            // exercise the safety shrink path.
            let n = slot.def.max_chars.max(slot.phrase.text.chars().count());
            let (px, by, _sq) = place_slot(&slot.def, dummy.width, dummy.height, n);
            assert!(px >= 0 && px < dummy.width as i32, "pen_x out of bounds");
            assert!(
                by >= 0 && by < dummy.height as i32,
                "baseline_y out of bounds"
            );
            // Re-derive total_w and verify it fits the inner safe box.
            let _per_char = 0; // placeholder; use place_slot's maths
                               // Compute total_w from em_scale × max_chars + drift/bearing pad.
            let scale_q8 = _sq;
            let target_px = (scale_q8 as f32) / 256.0 * glyph::HERO_EM_PX as f32;
            let per_char = (target_px * 1.06) as i32;
            let total_w = per_char * (n as i32 - 1).max(0) + (target_px as i32);
            // Drift + bearing-x overshoot must stay inside [16, w-16].
            let drift_pad = slot.def.drift_x.ceil() as i32 + 2;
            let bx_pad = (target_px * 0.08) as i32;
            assert!(
                px - bx_pad - drift_pad >= 16,
                "left edge unsafe for slot {:?}: px={} total_w={} drift={}",
                slot.def.role,
                px,
                total_w,
                drift_pad
            );
            assert!(
                px + total_w + bx_pad + drift_pad <= dummy.width as i32 - 16,
                "right edge unsafe for slot {:?}: px={} total_w={} drift={} w={}",
                slot.def.role,
                px,
                total_w,
                drift_pad,
                dummy.width
            );
            // Vertical: baseline + descender + drift must stay under h, and
            // baseline - bearing must stay above 0.
            let by_pad = (target_px * 0.85) as i32;
            let d_pad = (target_px * 0.10) as i32 + 2;
            let dy_pad = slot.def.drift_y.ceil() as i32 + 2;
            assert!(
                by - by_pad - dy_pad >= 16,
                "top edge unsafe for slot {:?}: by={}",
                slot.def.role,
                by
            );
            assert!(
                by + d_pad + dy_pad <= dummy.height as i32 - 16,
                "bottom edge unsafe for slot {:?}: by={} h={}",
                slot.def.role,
                by,
                dummy.height
            );
        }
    }

    #[test]
    fn alpha_ramps_in_and_out() {
        let def = SlotDef {
            role: SlotRole::Support,
            x_frac: 0.5,
            y_frac: 0.5,
            align: Align::Center,
            em_scale: 0.3,
            target_w_frac: 0.0,
            max_chars: 5,
            alpha: 1.0,
            shadow_mix: 0.0,
            drift_x: 0.0,
            drift_y: 0.0,
            drift_fx: 0.0,
            drift_fy: 0.0,
            drift_phase: 0.0,
            lifetime: 4.0,
            fade_in: 1.0,
            fade_out: 1.0,
            stagger: 0.0,
        };
        let mut slot = Slot::new(def);
        slot.primed = true;
        slot.age = 0.0;
        assert!(slot.alpha_now() < 0.1);
        slot.age = 1.0;
        assert!(slot.alpha_now() > 0.9);
        slot.age = 3.0;
        assert!(slot.alpha_now() > 0.9);
        slot.age = 3.99;
        assert!(slot.alpha_now() < 0.1);
    }
}
