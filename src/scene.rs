//! Scene state — slots, dust, sparks and their lifecycle.
//!
//! The screen carries one complete inscribed work: a hero line in the focal
//! area plus three supporting lines from the same poem in reading order.
//! Painting lives in [`crate::background`] and [`crate::compose`].

mod composition;
mod particles;
mod rng;

pub use composition::{Align, Composition, Slot, SlotDef, SlotRole, BEATS_PER_LINE};
pub use particles::{Dust, Spark, SPARK_CAP};
pub use rng::Lcg;

use crate::color;
use crate::phrase;

/// Whole scene state. Owns one composition plus the per-frame atmospheric
/// particles and a screen-size vignette scratch buffer the background paints
/// reuse every frame.
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub rng: Lcg,
    pub dust: Vec<Dust>,
    pub sparks: Vec<Spark>,
    /// `nx * nx` for every screen column — depends only on width, so it is
    /// built once in `new` and read every frame by the vignette loop.
    pub col_nx2: Vec<f32>,
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

        // Per-column vignette scalar lives for the scene's whole life; the
        // background loop reads it every frame instead of rebuilding a `Vec`.
        let mut col_nx2 = vec![0.0_f32; width as usize];
        let w_f = width as f32;
        for (x, slot) in col_nx2.iter_mut().enumerate() {
            let nx = (x as f32 / w_f - 0.5) * 2.0;
            *slot = nx * nx;
        }

        Self {
            width,
            height,
            rng,
            dust,
            sparks: Vec::with_capacity(SPARK_CAP),
            col_nx2,
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
        if self.sparks.len() > SPARK_CAP {
            let extra = self.sparks.len() - SPARK_CAP;
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
