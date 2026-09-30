//! Slot geometry and lifecycle — one inscription position plus the phrase
//! it currently carries.

use crate::color;
use crate::phrase::{self, Phrase};

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
    pub(crate) fn new(def: SlotDef) -> Self {
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
