// inkflow · scene.rs
//
// Per-frame ambience state. Hoisted, pre-allocated, fixed-cap:
//   - glyphs[] holds at most 260 floating CJK characters
//   - particles[] holds at most 260 sparks
//   - stars[] is rebuilt exactly once when screen dimensions arrive
//
// glyphs / particles live in VecDeque so push is O(1) at the cap —
// pop_front evicts the oldest in constant time and push_back appends
// the new entry, so a saturated LLM stream never touches a memmove.
// Stars stay in Vec because they're seeded once and iterated forever.

use crate::font::Rgba;
use std::collections::VecDeque;

pub const GLYPH_CAP: usize = 260;
pub const PARTICLE_CAP: usize = 260;
pub const STAR_COUNT: usize = 90;

#[derive(Clone, Copy)]
pub struct Glyph {
    pub ch: &'static str,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub life: f32,
    pub max_life: f32,
    pub size: f32,
    pub phase: f32,
}

#[derive(Clone, Copy)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub life: f32,
    pub max_life: f32,
    pub r: f32,
}

#[derive(Clone, Copy)]
pub struct Star {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub base: f32,
    pub phase: f32,
    pub period: f32,
    pub hue_offset: f32,
}

pub struct Scene {
    pub glyphs: VecDeque<Glyph>,
    pub particles: VecDeque<Particle>,
    pub stars: Vec<Star>,
}

impl Scene {
    pub fn new() -> Self {
        // VecDeque with a hard cap-equivalent capacity gives us O(1)
        // push_back / pop_front. The set never grows past the cap, so
        // we pre-size to GLYPH_CAP (not +slack) — push_back after the
        // buffer is full would panic, but our push_* call sites always
        // pop_front first when at cap, so the invariant holds.
        let g: VecDeque<Glyph> = VecDeque::with_capacity(GLYPH_CAP);
        let p: VecDeque<Particle> = VecDeque::with_capacity(PARTICLE_CAP);
        Self {
            glyphs: g,
            particles: p,
            stars: Vec::with_capacity(STAR_COUNT + 4),
        }
    }

    pub fn seed_stars(&mut self, sw: f32, sh: f32) {
        if !self.stars.is_empty() {
            return;
        }
        self.stars.resize(STAR_COUNT, Star::sentinel());
        for (i, s) in self.stars.iter_mut().enumerate() {
            let i = i as u64;
            s.x = lcg(i + 1) * sw;
            s.y = lcg(i + 1001).powf(1.4) * sh;
            s.r = 0.7 + lcg(i + 2003) * 1.4;
            s.base = 0.18 + lcg(i + 3001) * 0.22;
            s.phase = lcg(i + 4001) * core::f32::consts::TAU;
            s.period = 4.0 + lcg(i + 5003) * 10.0;
            s.hue_offset = (lcg(i + 6007) - 0.5) * 0.24;
        }
    }

    pub fn push_glyph(&mut self, g: Glyph) {
        // O(1) at cap: pop the oldest off the front, then append the new
        // glyph at the back. Replaces the previous Vec::remove(0) which
        // memmove'd every entry down by one (~36 B × 259 = ~9.3 KB) on
        // every LLM-streamed char past the 260-entry cap.
        if self.glyphs.len() >= GLYPH_CAP {
            self.glyphs.pop_front();
        }
        self.glyphs.push_back(g);
    }

    pub fn push_particle(&mut self, p: Particle) {
        // Same O(1) drop-oldest pattern as push_glyph. The previous
        // implementation drained the entire prefix down to fit, which
        // under high-energy / multi-touch contact pressure can run on
        // most frames; this is now constant work.
        if self.particles.len() >= PARTICLE_CAP {
            self.particles.pop_front();
        }
        self.particles.push_back(p);
    }
}

impl Star {
    fn sentinel() -> Self {
        Star {
            x: 0.,
            y: 0.,
            r: 0.,
            base: 0.,
            phase: 0.,
            period: 1.,
            hue_offset: 0.,
        }
    }
}

// deterministic LCG used for spawn jitter — the same seed always lands on
// the same glyph characteristics so the stream looks stable across runs.
#[inline]
pub fn lcg(seed: u64) -> f32 {
    let x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((x >> 33) as f32) / (1u64 << 31) as f32
}

// Hue → rgba helper (re-export).
#[allow(dead_code)]
#[inline]
pub fn hue_rgba(h: f32, s: f32, l: f32) -> Rgba {
    Rgba::from_hsl(h, s, l)
}
