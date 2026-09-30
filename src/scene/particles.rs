//! Atmospheric particles — slow drifting dust motes and touch-burst sparks.
//!
//! Both kinds of particle are plain `Copy` structs kept in `Vec`s with a fixed
//! cap; the renderer is allocation-free once the scene is constructed.

/// Slow drifting dust mote — the ambient mist that carries a slow breath
/// across the screen even when nothing is touching it.
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

/// Touch burst particle — short-lived sparks that read as a brush stroke when
/// the touch is hard and as a quiet puff when it is soft.
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

/// Hard cap on concurrent sparks — once exceeded, the oldest are drained.
pub const SPARK_CAP: usize = 64;
