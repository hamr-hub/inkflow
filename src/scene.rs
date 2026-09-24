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
            SlotDef {
                role: SlotRole::Support,
                x_frac: 0.50,
                y_frac: 0.66,
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
                alpha: 0.58,
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
                // On the FIRST pinned beat, leave the slot's age alone — the
                // initial `-stagger` set in `Scene::new` keeps the line
                // invisible until its turn arrives, so the quatrain unfurls
                // in reading order (hero → subtitle → upper-right →
                // lower-left). On subsequent pinned beats, sit at mid-life
                // so the inscription is fixed at peak alpha.
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
            dust.push(Dust {
                x: rng.unit() * width as f32,
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
                        // Stagger their visible birth by giving them negative
                        // age so they fade in over the first second rather
                        // than all at once.
                        slot.age = -slot.def.stagger;
                        slot.primed = false;
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
        // Parabolic bell: 0 at v=0.48, peaks ≈0.338 at v≈0.74, 0 at v=1.0.
        // Start shifted from v=0.50→v=0.48 so the bell's rising edge
        // reaches up to v≈0.66 (the subtitle's baseline) and the new
        // peak lands directly under v≈0.74 (the lower-left echo).
        // The warm horizon now grounds both lower strokes on a shared
        // mist band: 《言师采药去》 catches a touch of the leading edge
        // as it rises (horizon_glow ≈0.306 at v=0.66, 54 % more than
        // before) and 《云深不知处》 sits under the warmest part of the
        // bell, so the inscription's closing stroke reads as ink
        // dissolving into mist rather than floating over empty dark.
        // The hero (v≈0.42) and upper-right (v≈0.28) stay clear of the
        // band so the focal bloom keeps its exclusive claim on the
        // light (ART_DIRECTION §四 "高光只落在主句").
        let horizon_glow = ((v - 0.48) * (1.0 - v) * 5.0).clamp(0.0, 1.0);
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
    // subtitle (closest to focal, shadow_mix 0.10) catches the most
    // breath, the upper-right (0.26) less, the far-faint (0.42) the
    // least — same hierarchy that already governs their ink density,
    // now extending to motion. Amplitude is small (≤ 4 %) so the
    // supporting lines stay subordinate and never bloom; restraint
    // (ART_DIRECTION §四) holds.
    let breath = 1.0 + 0.04 * pulse * (1.0 - slot.def.shadow_mix);
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
    let horizon_glow = ((slot.def.y_frac - 0.48) * (1.0 - slot.def.y_frac) * 5.0).clamp(0.0, 1.0);
    let mist_warmth = horizon_glow * 0.20;
    let warmth_tint = (warmth * (1.0 - slot.def.shadow_mix) * 0.30 + mist_warmth).clamp(0.0, 1.0);
    let base_color = mix(raw_base, color::ink::WARM, warmth_tint);
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

    let base_color = mix(color::ink::CREAM, color::ink::WARM, warmth * 0.5);
    let glow_color = mix(color::ink::GLOW, color::ink::WARM, warmth * 0.6);
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
    let bloom2_scale_q8: u32 = ((scale_q8.max(1) as f32) * 1.13).round() as u32;
    let bloom2_alpha = (0.022 + 0.02 * pulse + 0.01 * warmth + beat_glow * 0.015).clamp(0.0, 0.06);
    // A touch warmer than the inner bloom so the outer corona reads as
    // amber lamplight spilling onto the page — closer to ink::WARM than
    // ink::GLOW, but still inside the cream family.
    let bloom2_color = mix(glow_color, color::ink::WARM, 0.3);

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
