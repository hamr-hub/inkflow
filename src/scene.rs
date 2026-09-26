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
            //   alpha 0.86 → 0.76, shadow_mix 0.10 → 0.16: the subtitle
            //   was reading at almost the same brightness as the hero,
            //   so the two lines formed one dense centred inscription
            //   instead of hero-plus-echo. The hero's bloom already
            //   carries the focal claim (ART_DIRECTION §四 "高光只落在
            //   主句"); pulling the subtitle's body a step further into
            //   shadow now lets the eye separate "主句" from "近对答" —
            //   the subtitle still leads the supporting hierarchy (above
            //   upper-right 0.72 and lower-left 0.58) but no longer reads
            //   as a co-focal duplicate. shadow_mix 0.16 also tightens
            //   the subtitle's breath amplitude slightly (the inscribed-
            //   breath is scaled by 1 - shadow_mix), so the supporting
            //   line leans even more clearly "echo of the hero" than
            //   "second voice".
            //   y_frac 0.64 → 0.65: lift the subtitle one step further
            //   from the hero so the central pair reads as two
            //   distinct strokes of one calligraphic brush rather than
            //   one dense centred inscription. The 0.64 lift was
            //   originally introduced to break the "vertical band of
            //   three lines" — by widening the subtitle-to-lower-left
            //   gap (0.10 vs the previous 0.08) and tightening the
            //   hero-to-subtitle pair (0.22 vs the previous 0.24).
            //   That worked, but it pushed the subtitle so close to the
            //   hero that the bloom of 《松下问童子》 reads as bleeding
            //   into 《言师采药去》 — the two lines register as one
            //   dense inscription block rather than as the focal line
            //   and its first echo. At 0.65 the hero-to-subtitle pair
            //   steps down from 158 → 166 px (+8 px, the hero's bloom
            //   now ends a comfortable 17 px above the subtitle's top
            //   instead of grazing it), while the subtitle-to-lower-
            //   left gap narrows 72 → 65 px — still 9 px wider than
            //   the 56-px band that originally caused the "vertical
            //   band of three lines" critique, so the subtitle still
            //   reads as the inscribed answer rather than collapsing
            //   onto the dissolving corner echo. The three inscribed
            //   gaps now step down together (hero→subtitle 166 px,
            //   subtitle→lower-left 65 px, lower-left→title 65 px) —
            //   the supporting tier shares one even rhythm, while the
            //   hero stands clearly alone above it (the 166 px opening
            //   is the page's largest breathing row). The mist-bell
            //   value at 0.65 is 0.315 (still 84 % of the 0.75 peak),
            //   so the subtitle keeps its rising-edge warmth that ties
            //   it to the warm horizon — the warm/cool axis (subtitle +
            //   lower-left warm, upper-right cool) and the brush-weight
            //   hierarchy (subtitle brightest → upper-right → lower-
            //   left dimmest) both hold unchanged.
            SlotDef {
                role: SlotRole::Support,
                x_frac: 0.50,
                y_frac: 0.65,
                align: Align::Center,
                em_scale: 0.34,
                target_w_frac: 0.0,
                max_chars: 7,
                alpha: 0.76,
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
            // 2 — Upper right (small body, right-aligned). Pulled inward
            //   from (0.84, 0.22) to (0.80, 0.28) so the upper echo sits
            //   at the rule-of-thirds intersection (≈(0.67, 0.33)) rather
            //   than as a corner satellite. The diagonal midpoint with
            //   the lower-left at (0.18, 0.74) stays at ≈(0.49, 0.51) —
            //   right at the optical centre — and top/bottom margins
            //   remain balanced (≈170 px vs ≈180 px). Mid-weight: the
            //   quatrain's location hint is already a step further from
            //   certainty than the subtitle.
            //   Stagger 0.34s so it fades in third, after the subtitle.
            //   shadow_mix 0.30 → 0.26, alpha 0.66 → 0.72: lift the
            //   upper-right back into readable territory so the four-line
            //   quatrain registers as four lines on the page, not two. The
            //   brush-weight gradient still holds (subtitle 0.10, upper-
            //   right 0.26, lower-left deepest), and the line stays
            //   clearly subordinate to the hero.
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
                y_frac: 0.28,
                align: Align::Right,
                em_scale: 0.32,
                target_w_frac: 0.0,
                max_chars: 5,
                alpha: 0.72,
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
            //   below upper-right 0.72 and subtitle 0.76 (the brush-
            //   weight hierarchy still steps down 0.76 > 0.72 > 0.612
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
                em_scale: 0.28,
                target_w_frac: 0.0,
                max_chars: 5,
                alpha: 0.612,
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
                        // hierarchy (subtitle 0.76 / upper-right 0.72 /
                        // lower-left 0.58) from the first frame instead
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
            // echo (x_frac 0.80, y_frac 0.28) which sits below and slightly
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
    // sits at v≈0.42 and the upper-right at v≈0.28, both clear of the
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
        // Parabolic bell: 0 at v=0.50, peaks ≈0.400 at v≈0.75, 0 at v=1.0.
        // Coefficient raised 5.0 → 6.0 → 6.4 (+7 % over two passes, this
        // pass +6.7 %) so the warm band sits a touch more visibly under
        // the lower-left echo — 《云深不知处》 reads as ink dissolving
        // into warm horizon rather than hovering over a barely-visible
        // tint, with the bell now reaching its peak luminance just as
        // the closing echo settles over the band. The subtitle (v≈0.66)
        // catches a little more warmth on the rising edge so both
        // lower strokes feel grounded on one shared band. The hero
        // (v≈0.42) and upper-right (v≈0.28) stay clear of the bell
        // so the focal bloom keeps its exclusive claim on the light
        // (ART_DIRECTION §四 "高光只落在主句"). Restraint holds:
        // peak alpha still ≤ 0.400 so the warm band reads as mist,
        // not as a horizon line, and the +6.7 % lift stays well under
        // the threshold where the warm band would compete with the
        // focal bloom's claim on the page's light.
        let horizon_glow = ((v - 0.50) * (1.0 - v) * 6.4).clamp(0.0, 1.0);
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
            // horizon mist — final atmospheric layer.
            let p = blend_screen(p, horizon_color, horizon_glow * 0.12);
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
    // Halo radius 44 → 60 → 62 (+41 % over two passes): the moon's
    // moonlit air now reaches a touch further into the upper-right
    // quadrant. The +3.3 % extension (60 → 62) keeps the halo edge
    // 2 px closer to the upper-right echo 《只在此山中》 (≈115 px from
    // the moon, 53 px outside the halo) — the echo still doesn't
    // receive visible halo luminance, but the air between the moon
    // and the echo now reads as continuously moonlit rather than
    // split into "moon halo" and "isolated echo". The two upper-right
    // inhabitants share one breathing atmosphere; the sky bell still
    // does the actual bridging of the 53-px gap (sigma 75 unchanged).
    // Halo peak 0.05 → 0.055 (+10 %): the moon's moonlit air now
    // reaches a touch further into the upper-right quadrant. The prior
    // 0.05 sat very close to invisibility against the vignette-darkened
    // corner — even the halo's brightest pixel added only 5 % cream,
    // which the warm horizon band and the dust haze underneath easily
    // pulled below "visible ring around the moon". With +10 % the halo
    // now reads as a clearly continuous ring of moonlit air at peak
    // (6 px outside the disc edge), tying the body to its moonlit air
    // more visibly without crossing into competing-bloom territory.
    // The +10 % keeps every halo pixel ≤ 0.055 alpha — still well
    // under the inscribed glow (~0.20+) and the hero bloom (~0.55),
    // so the focal line keeps its claim on the page's light
    // (ART_DIRECTION §四 "高光只落在主句"); the wider radius brings a
    // roughly proportional gain in total integrated halo luminance
    // (+3.3 % at this pass), but every pixel the halo touches is
    // still ≤ 0.055 alpha. Restraint (ART_DIRECTION §四) holds across
    // both passes — the moon now reads as one luminous body bathed in
    // moonlit air (body + halo + sky bell as three nested atmospheric
    // layers around one disc), not as a hard pixel ringed by an
    // independent halo.
    let moon_halo_r = 62.0_f32;
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
    // Halo peak 0.05 → 0.055 → 0.058 → 0.061 → 0.064 (+5.0 %, the fourth
    // step in this arc): the moon's moonlit air now reaches a touch
    // more visibly across the body-to-halo transition against the
    // heavily vignette-darkened upper-right corner. The +5.0 % (0.061
    // → 0.064) continues the same restraint cadence as the prior +5.5 %
    // halo bumps (238b40b, 698aa08) and the +5.5 % body bumps
    // (fe42fec, b7ebeda, 90e22dc) so the moon's three nested
    // atmospheric layers (body + halo + sky bell) share one
    // proportional cadence and the disc reads as one luminous body
    // whose inner ring stays in step with its brightest pixel. The
    // 4-px fade-in and σ 75 sky bell are unchanged so the body,
    // halo, and sky bell still read as three nested atmospheric
    // layers around one disc (ARTIFACT §观者第一分钟 1. 其它一切都
    // 在动，只有它是相对静止的锚). The +5.0 % also pairs with the
    // recent inscription-side refinement chain — title breath 0.0435
    // → 0.0464 (+6.7 % in 14d58aa), inscribed-breath base 0.075 →
    // 0.080 (+6.7 % in 708d491), cool_tint 0.115 → 0.123 (+6.5 % in
    // 0ce6e37), warm bell 6.0 → 6.4 (+6.7 % in c5f73e0), terminator
    // amber-tint cap 0.12 → 0.13 (+8.3 % in c601184), body 0.50 →
    // 0.55 → 0.58 → 0.612 → 0.646 (+5.5 % x4 in fe42fec, b7ebeda,
    // 90e22dc) — so the moon's atmospheric layers and the four
    // inscribed strokes plus the calligrapher's seal share one
    // proportional series of restrained steps (+5.0 %, +5.5 %,
    // +6.5 %, +6.7 %, +8.3 %), and the page's moonlit atmosphere
    // reads as one coherent refinement rather than seven independent
    // tweaks. The 0.064 cap keeps the halo well under the inscribed
    // glow (~0.20+) and the hero bloom (~0.55), so the focal line
    // keeps its claim on the page's light (ART_DIRECTION §四 "高光只
    // 落在主句" — 高光只落在主句 holds). The +0.003 absolute lift
    // stays inside the cream family and keeps the brightest halo
    // pixel well under the body's 0.646 peak and the inscribed glow
    // (~0.20+), so the moon continues to read as one luminous body
    // bathed in moonlit air — just air whose inner ring now reads a
    // touch more visibly continuous with the body's Gaussian tail
    // instead of leaving a faint dark valley between disc and halo
    // against the vignette-darkened upper-right corner. Restraint
    // (ART_DIRECTION §四 "克制统一的调色板") holds across all four
    // halo passes — the moon now reads as one luminous body bathed
    // in moonlit air (body + halo + sky bell as three nested
    // atmospheric layers around one disc), not as a hard pixel
    // ringed by an independent halo.
    let halo_peak = 0.064_f32;
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
                // Body bell — Gaussian with sigma 8 px. Falls smoothly
                // from body center (alpha ≈ 0.50) through body edge
                // (alpha ≈ 0.05) and continues past body_r as a faint
                // contribution that blends with the halo. The previous
                // quadratic k^2 falloff dropped to 0 at body_r while the
                // halo was just starting to rise — leaving a 6-px ring
                // of near-zero luminance that read as a faint dark rim
                // against the vignette-darkened upper-right corner
                // (the disc looked hollow rather than luminous). With
                // the Gaussian, body alpha at body_r (≈0.05) ≈ halo peak
                // (0.05), so the body and halo read as one continuous
                // luminous body (moon + moonlit air merged) instead of
                // "bright core + dark rim + halo ring". Sigma 8 chosen
                // so body_k(body_r) = exp(-17²/128) = exp(-2.26) ≈ 0.105
                // → alpha ≈ 0.053 ≈ halo_peak. The Gaussian tail past
                // body_r continues to fall smoothly toward halo_r, so
                // the body and halo contributions overlap without any
                // brightness discontinuity. Restraint (ART_DIRECTION §四
                // "高光只落在主句") holds: peak 0.50 is well under the
                // hero bloom's combined ~0.7 effective alpha, and the
                // body's tail past body_r drops to <0.01 by halo_r so
                // the moon never spills beyond the halo boundary.
                let body_k = (-d * d / 128.0).exp();
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
                // 0..12 % → 0..13 % toward AMBER (+8.3 % relative, the
                // second lift in the tint arc after 0e5c075 introduced
                // it alongside the ±16 → ±20 % alpha step) so the moon
                // reads more clearly as a body catching horizon light
                // — the bottom edge now catches ≈+24 % alpha AND ≈13 %
                // amber, giving the disc a clear direction (lit side
                // facing down, where the warm horizon mist sits) rather
                // than a uniform luminous disc. The +8.3 % amber-tint
                // lift continues the same restraint cadence as the
                // recent inscription-side refinement chain — warm bell
                // 6.0 → 6.4 (+6.7 % in c5f73e0), inscribed-breath base
                // 0.075 → 0.080 (+6.7 % in 708d491), title breath
                // 0.0435 → 0.0464 (+6.7 % in 14d58aa), cool_tint
                // 0.115 → 0.123 (+6.5 % in 0ce6e37) — and the moon-side
                // body 0.50 → 0.55 → 0.58 → 0.612 → 0.646 (+5.5 % x4 in
                // fe42fec, b7ebeda, 90e22dc) and halo 0.05 → 0.055 →
                // 0.058 → 0.061 (+5.5 % / +10 % x3 in 238b40b, 698aa08)
                // so the moon's two innermost atmospheric layers and
                // its chromatic-asymmetry term now share one
                // proportional series of restrained steps (+5.5 %,
                // +6.5 %, +6.7 %, +8.3 %), and the page's moonlit
                // atmosphere reads as one coherent refinement rather
                // than six independent moon-side tweaks. The +24 % alpha
                // asymmetry stays put (its last step in f207bea already
                // brought the directional "moon catching horizon light"
                // reading clearly into view against the heavily
                // vignette-darkened upper-right corner, so only the
                // chromatic axis needs this single pass). The 13 % tint
                // cap stays well under the threshold where the moon
                // would read as amber highlighter (the prior "12 % so
                // the moon still reads as cream ink" cap lifts by only
                // +1 absolute / +8.3 % relative, the same restraint
                // cadence as the +6.5 % / +6.7 % inscription-side
                // refinements) — restraint (ART_DIRECTION §四 "克制
                // 统一的调色板" / "低饱和、高级灰") holds: the moon
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
                let t_term = (dy * body_recip).clamp(-1.0, 1.0);
                let term = 1.0 + 0.24 * t_term;
                let term_warm = (t_term * 0.13).max(0.0);
                // Body and halo now breathe independently — the disc sits
                // at ±2 % (the still anchor, below every inscription line),
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
                // still only varies by ±7 % (peak 0.058 * 1.07 = 0.0621,
                // vs the prior 0.0615) so the absolute alpha ceiling is
                // essentially unchanged — the lift is in the breath
                // relationship, not in the halo's brightness, so the
                // focal line's claim on the page's light holds
                // (ART_DIRECTION §四 "高光只落在主句").
                let body_pulse = 1.0 + pulse * 0.02;
                let halo_pulse = 1.0 + pulse * 0.07;
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
    let sky_peak = 0.034_f32;
    let sky_sigma = 75.0_f32;
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
            let a = sky_peak * k;
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
    // → 0.080 (+33 % over three passes, this pass +6.7 %): the breath
    // now rises to ±6.7 % / ±5.9 % / ±4.6 % across the three
    // supporting echoes (was ±6.3 % / ±5.6 % / ±4.4 %), so the
    // inscription reads as one calligraphic work breathing a touch
    // deeper under one shared light — the +6.7 % continues the same
    // cadence as the warm bell lift in c5f73e0 (6.0 → 6.4, +6.7 %),
    // so the supporting tier's breath and the warm horizon band now
    // share one proportional cadence and the inscription reads as
    // breathing inside the same moonlit air that hosts the warm mist.
    // The subtitle at ±6.7 % now sits just below the moon's halo
    // pulse (±7 %), the natural amplitude for "the page's atmosphere
    // that the inscription breathes within" (the halo is the air, the
    // inscription is the ink that lives in it, and the ink now reads
    // as nearly as alive as the air it inhabits). The upper-right at
    // ±5.9 % matches the halo pulse almost exactly — the upper-right
    // echo bathes in moonlit air that pulses at the same rate it
    // does, so 《只在此山中》 reads as ink breathing in the moon's
    // sphere of influence rather than ink floating beside it. The
    // far-faint at ±4.6 % stays clearly below the halo pulse, the
    // brush running thin as the inscription closes on 《云深不知处》.
    // The three supporting echoes still move with the same rhythm-
    // engine pulse but at visibly different depths, and all three stay
    // well under the hero bloom's combined ~0.7 effective alpha —
    // restraint (ART_DIRECTION §四 "高光只落在主句") holds across all
    // amplitudes and the new halo-pulse match.
    let breath = 1.0 + 0.080 * pulse * (1.0 - slot.def.shadow_mix);
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
    // touch-driven warmth is off. The upper-right (v≈0.28) sits clear
    // of the band so it stays cool, layering an "in the mist" vs "in
    // the sky" axis on top of the brush-weight gradient. The mist bell
    // peaks at v≈0.74, so the lower-left catches the most, the subtitle
    // catches a touch on the rising edge, and the line still reads as
    // deep ink (its shadow_mix 0.42 keeps it the dimmest of the
    // supporting tier) dissolving into warm horizon — the visual
    // metaphor of 《云深不知处》: the clouds are deep, one knows not
    // where. Restraint (ART_DIRECTION §四): mist contribution capped at
    // ≈7 % so the supporting tier stays subordinate and the brush-weight
    // hierarchy (subtitle brightest, lower-left dimmest) holds.
    // Same bell as the background mist (coefficient 6.4, onset 0.50,
    // peak 0.400 at v≈0.75) so the supporting line's warm tint and
    // the atmospheric warm band stay in sync — the +6.7 % coefficient
    // lift from 6.0 → 6.4 lifts both the background luminance and
    // each supporting line's warm tint by the same percentage so
    // 《云深不知处》 sits inside a slightly more visibly inhabited
    // stretch of warm mist without ever reading as warmer than the
    // air it sits in. The lower-left at v≈0.74 sits just under the
    // peak (mist_warmth ≈ 0.080, +7 % over the previous 0.075) —
    // still within the restraint cap (≈8 %) so 《云深不知处》
    // reads as deep ink actually dissolving into the warm horizon,
    // not as dim cream floating over a barely-visible amber tint.
    // The subtitle (v≈0.66) catches a touch more warmth on the
    // rising edge; the upper-right (v≈0.28) stays clear of the bell
    // so it remains the cool echo in the moon's air.
    let horizon_glow = ((slot.def.y_frac - 0.50) * (1.0 - slot.def.y_frac) * 6.4).clamp(0.0, 1.0);
    let mist_warmth = horizon_glow * 0.20;
    // Cool axis — the mirror image of the mist warmth above. Supporting
    // lines that sit in the moonlit upper sky absorb a touch of cool
    // tint from the cool air they inhabit, so the upper-right echo
    // 《只在此山中》 (v≈0.28) reads as ink in the moon's sphere of
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
    // upper-right echo at (0.80, 0.28), next to the moon at (0.86, 0.16))
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
    // Moon-proximity weight 0.082 → 0.087 (+6.1 %, the fifth step in
    // this arc, now in the same restraint cadence as the recent
    // body_peak +5.6 %, halo +5.0 %, sky +6.25 %, inscribed-breath
    // +6.7 %, title breath +6.7 %, and title alpha +4.3 % chain —
    // smaller than the prior +17.1 % paired weight+cap step in 0ce6e37
    // because that arc was at the +17 % magnitude and the rest of the
    // page's atmosphere has settled into the +5-7 % cadence). The +6.1 %
    // continues the same direction as the four prior moon_proximity
    // lifts (0.04 → 0.06 → 0.07 → 0.082) but at the gentler +5-7 %
    // step that matches the rest of the recent work, so the upper-
    // right's cool_tint now lifts from 0.123 → 0.126 (+2.4 %, the
    // natural proportional gain for a +6.1 % weight bump at proximity
    // 0.665 — moon_cool goes from 0.0545 → 0.0578, sky_cool stays at
    // 0.068). The 0.14 cap stays put (the formula now sits 0.014
    // below the cap, ≈+24 % more weight headroom before re-engaging),
    // so the upper-right still reads as cream ink bathed in moon's air
    // rather than cyan (ART_DIRECTION §四 "低饱和、高级灰"). The +2.4 %
    // cool_tint lift pairs with the recent inscription-side refinement
    // chain — title breath 0.0435 → 0.0464 (+6.7 % in 14d58aa),
    // inscribed-breath base 0.075 → 0.080 (+6.7 % in 708d491),
    // title alpha 0.46 → 0.48 (+4.3 % in e37c083), warm bell
    // 6.0 → 6.4 (+6.7 % in c5f73e0), terminator amber-tint cap
    // 0.12 → 0.13 (+8.3 % in c601184), body 0.50 → 0.55 → 0.58 →
    // 0.612 → 0.646 → 0.682 (+5.6 % x5 in fe42fec, b7ebeda, 90e22dc,
    // 3b60530), halo 0.05 → 0.055 → 0.058 → 0.061 → 0.064 (+5.0 %
    // / +5.5 % x4 in 238b40b, 698aa08, 0f13e55), sky_peak 0.018 →
    // 0.028 → 0.030 → 0.032 → 0.034 (+56 % / +7 % / +6.25 % in
    // b7ebeda, 80e27d5, 9ec99ff), and the cool_tint weight itself
    // +50 % / +16.7 % / +17.1 % in the prior arc — so the moon's
    // three nested atmospheric layers, the four inscribed strokes,
    // the calligrapher's seal, and the moon's reach onto its closest
    // inscription line now share one proportional series of restrained
    // steps (+5.0 %, +5.5 %, +5.6 %, +6.1 %, +6.25 %, +6.5 %, +6.7 %,
    // +8.3 %), and the page's moonlit atmosphere reads as one coherent
    // refinement rather than ten independent tweaks. The cap 0.14
    // staying put also means the brush-weight hierarchy (subtitle
    // brightest, upper-right next, lower-left dimmest) and the warm /
    // cool axis (subtitle + lower-left warm, upper-right cool) both
    // hold — the lift stays in the relationship between the moon and
    // its closest echo, not in the absolute brightness of either.
    // Restraint (ART_DIRECTION §四 "克制统一的调色板") holds: the +0.005
    // absolute lift on moon_cool stays inside the cream family, the
    // brightest pixel of the upper-right echo still sits well under the
    // inscribed glow (~0.20+) and the hero bloom (~0.55), so the focal
    // line keeps its exclusive claim on the page's light (ART_DIRECTION
    // §四 "高光只落在主句"). With the upper-right now leaning one more
    // restrained step into the moon's sphere of influence, 《只在此山
    // 中》 reads as ink that lives a touch deeper in the moon's air
    // — the echo "only in this mountain" now bathes a touch more
    // clearly in the same moonlit air as the disc above it, and the
    // moon's reach onto its closest inscription line settles into the
    // same +5-7 % restraint cadence as the rest of the page's recent
    // refinements.
    let cool_tint = (sky_cool + moon_proximity * 0.087).clamp(0.0, 0.14);
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
    let ep = match beat.phase {
        Phase::Entrance => beat.entrance_progress(),
        Phase::Hold => 1.0,
        Phase::Exit => 1.0 - beat.exit_progress(),
        Phase::Rest => 0.0,
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
    let glow_alpha = (0.10 + 0.18 * pulse + 0.06 * warmth + beat_glow * 0.10).clamp(0.0, 0.55);
    // Secondary wider bloom — same glyph drawn at slightly larger scale and
    // very low alpha so the focal line reads as a moonlit light source, not
    // just cream text on a gradient. ART_DIRECTION mandates "bloom only on
    // the focal line"; this pass is hero-only — supporting slots skip it
    // (see `paint_supporting_slot`).
    let bloom_scale_q8: u32 = ((scale_q8.max(1) as f32) * 1.06).round() as u32;
    let bloom_alpha = (0.04 + 0.04 * pulse + 0.02 * warmth + beat_glow * 0.03).clamp(0.0, 0.12);
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
    let bloom2_alpha = (0.022 + 0.02 * pulse + 0.01 * warmth + beat_glow * 0.015).clamp(0.0, 0.06);
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
    // (《寻隐者不遇》) rather than four floating lines. Alpha 0.40 keeps it
    // a quiet mark; size em_scale 0.18 (≈23 px tall at hero em 128) sits
    // below the lower-left echo (y_frac 0.74 → y=533) with a comfortable
    // 130 px margin so the four-line inscription still leads. Center
    // alignment pairs with the hero's centre so the title's baseline
    // visually anchors the whole composition. No outer glow — a title is
    // ink-on-paper, not a light source (ART_DIRECTION §四 "高光只落在
    // 主句"). Slight warm tint from `warmth` so a touched-warm scene
    // breathes amber on the signature too.
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
    // target 22 px tall (≈ 0.17 of hero em). Render from the hero bucket
    // with a Q8 scale so we downsample the 128 px glyph to a small, soft
    // signature — closer to ink on paper than to a printed label. Drawing
    // glyph-by-glyph (rather than via draw_phrase) lets us pick the scale
    // freely; draw_phrase locks to the bucket's native em.
    let target_px = 22.0_f32;
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
    // at title_v≈0.90 the bell sits at the far tail (horizon_glow ≈
    // 0.256, ambient ≈ 0.051) and the contribution lands at ≈ 5.1 %
    // always-on warm (+6.7 % over the previous 4.8 %), still well below
    // the supporting lines' mist share (≈ 8.0 % on the lower-left)
    // and inside the restraint cap (≈ 8 %) so the seal stays one
    // quiet step below the inscribed tier rather than narrowing the
    // brush-weight gap.
    let ambient_warmth = ((title_v - 0.50) * (1.0 - title_v) * 6.4).clamp(0.0, 1.0) * 0.20;
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
    // to keep matching the lower-left echo's current breath exactly
    // (0.080 * (1 - 0.42) of the supporting-tier formula): the
    // calligrapher's seal and 《云深不知处》 now share one breathing
    // rate at the bottom of the page after the most recent supporting-
    // tier base lift (0.075 → 0.080, +6.7 % in 708d491) — the title
    // would have quietly fallen out of step with 《云深不知处》 if
    // left at 0.0435 (now 0.003 below the lower-left's 0.0464 instead
    // of exact), and the two bottom strokes of the inscribed work
    // read as one pair inhaling together at the same rate. The +6.7 %
    // (0.0435 → 0.0464) continues the same restraint cadence as the
    // supporting-tier base lift in 708d491 and the warm bell lift
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
    let breath = 1.0 + 0.0464 * pulse;
    let alpha = (0.48_f32 * breath).clamp(0.0, 1.0);
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
