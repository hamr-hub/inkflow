//! Scene state — slots, dust, sparks and their lifecycle.
//!
//! The screen carries one complete inscribed work: a hero line in the focal
//! area plus three supporting lines from the same poem in reading order.
//! Painting lives in [`crate::background`] and [`crate::compose`].

use crate::color;
use crate::phrase::{self, Phrase};

/// Deterministic LCG so the scene looks "natural" but stable across runs.
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

/// Slow drifting dust mote.
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

/// Touch burst particle.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotRole {
    Hero,
    Support,
}

/// Fixed geometry of one inscription slot.
#[derive(Clone, Copy, Debug)]
pub struct SlotDef {
    pub role: SlotRole,
    pub x_frac: f32,
    pub y_frac: f32,
    pub align: Align,
    pub em_scale: f32,
    pub target_w_frac: f32,
    pub max_chars: usize,
    pub alpha: f32,
    pub shadow_mix: f32,
    pub drift_x: f32,
    pub drift_y: f32,
    pub drift_fx: f32,
    pub drift_fy: f32,
    pub drift_phase: f32,
    pub lifetime: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub stagger: f32,
}

/// A live slot: geometry plus the current phrase and lifecycle age.
pub struct Slot {
    pub def: SlotDef,
    pub phrase: &'static Phrase,
    pub age: f32,
    pub last_idx: u16,
    pub primed: bool,
}

impl Slot {
    fn new(def: SlotDef) -> Self {
        Self {
            def,
            phrase: phrase::phrase_for_beat(0),
            age: -def.stagger,
            last_idx: u16::MAX,
            primed: false,
        }
    }

    pub fn alpha_now(&self) -> f32 {
        if self.age < 0.0 {
            return 0.0;
        }
        let ramp_in = (self.age / self.def.fade_in.max(0.001)).clamp(0.0, 1.0);
        let ramp_out =
            ((self.def.lifetime - self.age) / self.def.fade_out.max(0.001)).clamp(0.0, 1.0);
        color::smootherstep(ramp_in) * color::smootherstep(ramp_out) * self.def.alpha
    }

    pub fn drift(&self, t: f32) -> (f32, f32) {
        let dx = (t * self.def.drift_fx + self.def.drift_phase).sin() * self.def.drift_x;
        let dy = (t * self.def.drift_fy + self.def.drift_phase * 1.3).cos() * self.def.drift_y;
        (dx, dy)
    }
}

/// One hero plus three supporting slots.
pub struct Composition {
    pub slots: Vec<Slot>,
    pub hero_idx: usize,
    pub beats_since_theme: u32,
    next_support_to_refresh: usize,
    first_pinned_beat_done: bool,
}

/// Beats spent on one poem line before the hero advances to the next.
pub const BEATS_PER_LINE: u64 = 4;
/// Beats for one full pass of a four-line poem; then the work rotates.
const BEATS_PER_WORK: u32 = 16;

impl Composition {
    pub fn default_layout() -> Self {
        let defs: Vec<SlotDef> = vec![
            SlotDef {
                role: SlotRole::Hero,
                x_frac: 0.50,
                y_frac: 0.42,
                align: Align::Center,
                em_scale: 0.0,
                target_w_frac: 0.58,
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
            // Subtitle — closest echo, brightest of the supporting strokes.
            SlotDef {
                role: SlotRole::Support,
                x_frac: 0.50,
                y_frac: 0.66,
                align: Align::Center,
                em_scale: 0.369,
                target_w_frac: 0.0,
                max_chars: 7,
                alpha: 0.62,
                shadow_mix: 0.18,
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
            // Upper-right echo — shares the moon's stillness so the
            // upper-right reads as one constellation (moon + echo).
            SlotDef {
                role: SlotRole::Support,
                x_frac: 0.82,
                y_frac: 0.27,
                align: Align::Right,
                em_scale: 0.32,
                target_w_frac: 0.0,
                max_chars: 5,
                alpha: 0.533,
                shadow_mix: 0.30,
                drift_x: 1.2,
                drift_y: 0.9,
                drift_fx: 0.13,
                drift_fy: 0.16,
                drift_phase: 1.4,
                lifetime: 12.0,
                fade_in: 0.6,
                fade_out: 0.7,
                stagger: 0.34,
            },
            // Lower-left echo — faintest; lifted from the floor so the poem's
            // quietest voice still reads as a fourth line, not a margin note.
            SlotDef {
                role: SlotRole::Support,
                x_frac: 0.18,
                y_frac: 0.74,
                align: Align::Left,
                em_scale: 0.22,
                target_w_frac: 0.0,
                max_chars: 5,
                alpha: 0.4305,
                shadow_mix: 0.46,
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
            next_support_to_refresh: 1,
            first_pinned_beat_done: false,
        }
    }

    /// Age supporting slots. Pinned works hold the strokes near peak alpha;
    /// unpinned pools swap phrases at end of life.
    pub fn step(&mut self, dt: f32, rng: u32, theme_idx: usize) {
        let pinned = phrase::POEM_BY_THEME
            .get(theme_idx)
            .map(|&g| !phrase::poem_group_line_indices(g).is_empty())
            .unwrap_or(false);
        for slot in self.slots.iter_mut() {
            if slot.def.role == SlotRole::Hero {
                continue;
            }
            slot.age += dt;
            if pinned {
                if slot.age > slot.def.lifetime * 0.8 {
                    slot.age = slot.def.lifetime * 0.5;
                }
                continue;
            }
            if slot.primed && slot.age >= slot.def.lifetime {
                slot.phrase = phrase::pick_from_theme_by_len(
                    rng,
                    theme_idx,
                    slot.def.max_chars,
                    &[slot.last_idx],
                );
                slot.last_idx = phrase_index_of(slot.phrase);
                slot.age = 0.0;
            }
            if !slot.primed && slot.age >= 0.0 {
                slot.primed = true;
            }
        }
    }

    /// Place the hero phrase for this beat; for pinned works lay the other
    /// poem lines into the supporting slots in reading order. Returns the
    /// (possibly rotated) theme index.
    pub fn on_hero_beat(
        &mut self,
        beat_index: u64,
        rng: u32,
        hero_phrase: &'static Phrase,
        theme_idx: usize,
    ) -> usize {
        let lines = phrase::POEM_BY_THEME
            .get(theme_idx)
            .map(|&g| phrase::poem_group_line_indices(g))
            .filter(|l| !l.is_empty());

        let hero_line = lines.map(|l| {
            let pos = ((beat_index / BEATS_PER_LINE) as usize) % l.len();
            l[pos]
        });
        let (hero_idx, hero_static) = match hero_line {
            Some(i) => (i, &phrase::PHRASES[i as usize]),
            None => (phrase_index_of(hero_phrase), hero_phrase),
        };
        let hero_slot = &mut self.slots[self.hero_idx];
        hero_slot.phrase = hero_static;
        hero_slot.last_idx = hero_idx;

        let support_indices: Vec<usize> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.def.role == SlotRole::Support)
            .map(|(i, _)| i)
            .collect();

        if let Some(poem_lines) = lines {
            let hero_pos = ((beat_index / BEATS_PER_LINE) as usize) % poem_lines.len();
            for (cursor, &slot_idx) in support_indices.iter().enumerate() {
                let line_pos = (hero_pos + 1 + cursor) % poem_lines.len();
                let line_idx = poem_lines[line_pos];
                let slot = &mut self.slots[slot_idx];
                slot.phrase = &phrase::PHRASES[line_idx as usize];
                slot.last_idx = line_idx;
                slot.primed = true;
                if self.first_pinned_beat_done {
                    slot.age = slot.def.lifetime * 0.5;
                }
            }
            self.first_pinned_beat_done = true;
        } else if let Some(&slot_idx) = support_indices.get(self.next_support_to_refresh) {
            self.next_support_to_refresh =
                (self.next_support_to_refresh + 1) % support_indices.len();
            let slot = &mut self.slots[slot_idx];
            slot.phrase = phrase::pick_from_theme_by_len(
                rng,
                theme_idx,
                slot.def.max_chars,
                &[slot.last_idx],
            );
            slot.last_idx = phrase_index_of(slot.phrase);
            slot.age = slot.def.lifetime * 0.5;
            slot.primed = true;
        }

        self.beats_since_theme += 1;
        if self.beats_since_theme >= BEATS_PER_WORK {
            self.beats_since_theme = 0;
            ((beat_index as usize) / BEATS_PER_WORK as usize) % phrase::THEMES.len()
        } else {
            theme_idx
        }
    }
}

fn phrase_index_of(p: &Phrase) -> u16 {
    let base = phrase::PHRASES.as_ptr() as usize;
    let off = (p as *const Phrase as usize - base) / core::mem::size_of::<Phrase>();
    off as u16
}

/// Whole scene state.
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub rng: Lcg,
    pub dust: Vec<Dust>,
    pub sparks: Vec<Spark>,
    pub ambient_pulse: f32,
    pub warmth: f32,
    pub theme_idx: usize,
    pub composition: Composition,
    pub moon_x: f32,
    pub moon_y: f32,
    pub moon_phase: f32,
}

impl Scene {
    pub fn new(width: u32, height: u32) -> Self {
        let mut rng = Lcg::new(0x00C0_FFEE_BEEF);
        let mut dust = Vec::with_capacity(48);
        for _ in 0..30 {
            dust.push(Dust {
                x: rng.unit() * width as f32,
                y: rng.unit() * height as f32,
                r: 0.6 + rng.unit() * 1.8,
                a: 0.05 + rng.unit() * 0.16,
                phase: rng.unit() * core::f32::consts::TAU,
                speed: 0.04 + rng.unit() * 0.12,
                hue: color::star::WARM,
            });
        }
        for _ in 0..18 {
            let y = (0.55 + rng.unit() * 0.30) * height as f32;
            let x = rng.unit().powf(1.4) * width as f32;
            dust.push(Dust {
                x,
                y,
                r: 0.5 + rng.unit() * 1.2,
                a: 0.04 + rng.unit() * 0.12,
                phase: rng.unit() * core::f32::consts::TAU,
                speed: 0.03 + rng.unit() * 0.08,
                hue: color::star::WARM,
            });
        }

        let initial_theme = 0usize;
        let mut composition = Composition::default_layout();
        let group = phrase::POEM_BY_THEME[initial_theme];
        let lines = phrase::poem_group_line_indices(group);
        {
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
                        let idx = lines[(1 + cursor) % lines.len()];
                        slot.phrase = &phrase::PHRASES[idx as usize];
                        slot.last_idx = idx;
                        slot.age = slot.def.lifetime * 0.5;
                        slot.primed = true;
                        cursor += 1;
                    }
                }
            }
        }

        Self {
            width,
            height,
            rng,
            dust,
            sparks: Vec::with_capacity(64),
            ambient_pulse: 0.0,
            warmth: 0.0,
            theme_idx: initial_theme,
            composition,
            moon_x: width as f32 * 0.86,
            moon_y: height as f32 * 0.16,
            moon_phase: 0.0,
        }
    }

    /// Touch burst: warm or cool sparks depending on the current mood.
    pub fn touch(&mut self, px: f32, py: f32, intensity: f32) {
        let n = (12.0 + intensity * 28.0) as usize;
        let warm = self.warmth > 0.05;
        for _ in 0..n {
            let ang = self.rng.unit() * core::f32::consts::TAU;
            let sp = 40.0 + self.rng.unit() * 220.0 * (0.5 + intensity);
            let hue = if warm {
                if self.rng.unit() < 0.7 {
                    color::drop::AMBER_HI
                } else {
                    color::drop::AMBER
                }
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

    pub fn step(&mut self, dt: f32, warmth: f32, pulse: f32) {
        self.warmth = warmth;
        self.ambient_pulse += dt * 0.4;
        self.moon_phase += dt * 0.012;
        let (w, h) = (self.width as f32, self.height as f32);
        for d in self.dust.iter_mut() {
            d.phase += dt * d.speed;
            d.x += d.phase.cos() * 0.4 * dt * 6.0;
            d.y += (d.phase.sin() * 0.3 * dt * 4.0) + dt * 0.3;
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
            d.a = (d.a * (1.0 + pulse * 0.2)).clamp(0.0, 0.5);
        }
        let drag = 0.85_f32.powf(dt * 60.0);
        for s in self.sparks.iter_mut() {
            s.life -= dt;
            s.x += s.vx * dt;
            s.y += s.vy * dt;
            s.vx *= drag;
            s.vy *= drag;
            s.vy += dt * 30.0;
        }
        self.sparks.retain(|s| s.life > 0.0);
        self.composition.step(dt, self.rng.next(), self.theme_idx);
    }
}

pub use crate::background::paint_background;
pub use crate::compose::paint_composition;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_has_one_hero_and_supporting() {
        let c = Composition::default_layout();
        assert_eq!(c.slots.len(), 4);
        assert_eq!(c.slots[0].def.role, SlotRole::Hero);
        assert!(c.slots[1..].iter().all(|s| s.def.role == SlotRole::Support));
    }

    #[test]
    fn slots_fit_screen() {
        let c = Composition::default_layout();
        for s in &c.slots {
            assert!(s.def.x_frac > 0.05 && s.def.x_frac < 0.95);
            assert!(s.def.y_frac > 0.1 && s.def.y_frac < 0.9);
            assert!(s.def.alpha <= 1.0);
        }
    }

    #[test]
    fn alpha_ramps_in_and_out() {
        let mut c = Composition::default_layout();
        let theme = 1; // pinned theme — strokes hold near peak
        c.step(0.016, 0, theme);
        let a0 = c.slots[1].alpha_now();
        c.step(1.0, 0, theme);
        let a1 = c.slots[1].alpha_now();
        assert!(a1 > a0);
        c.step(20.0, 0, theme);
        let a2 = c.slots[1].alpha_now();
        assert!(a2 > 0.3);
    }

    #[test]
    fn pinned_beats_walk_the_poem_in_order() {
        let mut c = Composition::default_layout();
        c.on_hero_beat(0, 0, &phrase::PHRASES[0], 0);
        assert_eq!(c.slots[0].phrase.text, "松下问童子");
        c.on_hero_beat(4, 0, &phrase::PHRASES[0], 0);
        assert_eq!(c.slots[0].phrase.text, "言师采药去");
        c.on_hero_beat(8, 0, &phrase::PHRASES[0], 0);
        assert_eq!(c.slots[0].phrase.text, "只在此山中");
        c.on_hero_beat(12, 0, &phrase::PHRASES[0], 0);
        assert_eq!(c.slots[0].phrase.text, "云深不知处");
    }
}
