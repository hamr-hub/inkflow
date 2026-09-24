// inkflow · mood.rs
//
// Touch-driven + autonomous mood vector (warmth, energy, idle) and the
// parameters that drive the visual scene (hue, hue drift, fog density).
//
// "Warmth" is roughly the screen-horizontal mean of touch contacts
// (left = cool, right = warm). "Energy" is the integrated contact
// speed plus a per-contact contribution, exponentially decayed toward
// 0 in the absence of touch. Both are EMA-blended so the scene never
// snaps on a single touch event but also reacts within ~1 second.
//
// This module is pure data + a few helper functions. The actual
// touch-state struct lives in `crate::evdev::TouchState` because the
// touch reader thread owns it; we expose a small façade that reads
// from the shared TouchState and produces the per-frame scene inputs.

use crate::evdev::TouchState;

/// Per-frame mood envelope, computed once and consumed by both the
/// renderer (for hue / fog density) and the LLM worker (for prompt
/// conditioning).
#[derive(Clone, Debug)]
pub struct FrameMood {
    /// Cool↔warm, normalized 0..1. 0 = cold, 1 = warm.
    pub warmth: f32,
    /// Energy / motion, 0..1. Drives spawn rate, particle intensity,
    /// prompt energy bucket.
    pub energy: f32,
    /// Idle contribution even when no touches are present. This keeps
    /// the ambient alive between touch events. Output range 0..0.25.
    pub idle: f32,
    /// Number of live touch contacts this frame (after stale-touch
    /// filtering).
    pub contacts: usize,
    /// Human-readable touch device name (for telemetry), or empty if
    /// no touch device is connected.
    pub touch_device: String,
}

impl FrameMood {
    /// Effective energy used by the renderer = max(real energy, idle).
    /// Idle is the autonomous "breathing" floor.
    #[inline]
    pub fn effective_energy(&self) -> f32 {
        self.energy.max(self.idle)
    }
}

/// Warmth drift slow sine — gives the cool↔warm palette an autonomous
/// breath even when no one is touching. Period ≈ 140 s, amplitude ±0.22
/// around 0.5. The drift is consumed by the renderer's hue formula.
#[inline]
pub fn autonomous_warmth_target(t: f32) -> f32 {
    0.5 + 0.22 * (t * 0.045).sin()
}

/// Idle energy contribution. Slow sine on a different period so it
/// doesn't lock-step with the warmth drift. Output 0..0.25.
#[inline]
pub fn idle_energy(t: f32) -> f32 {
    ((t * 0.13).sin() * 0.5 + 0.5) * 0.25
}

/// Hue derived from warmth + slow drift. Hue is in 0..1, suitable for
/// `Rgba::from_hsl(hue, sat, lit)`.
#[inline]
pub fn hue_at(t: f32, warmth: f32) -> f32 {
    let drift = (t * 0.025).sin() * 0.18;
    (0.58 - warmth * 0.5 + drift).rem_euclid(1.0)
}

/// Apply the per-frame EMA / decay / staleness logic to the touch
/// state. Caller passes the dt (clamped to [1ms, 50ms]) and the wall
/// seconds `t`. Returns the new mood envelope.
///
/// The state-mutation rules are:
/// - `energy *= 0.965 ^ (dt * 60)` so a frame of 16.7 ms decays by
///   ~3.4%; higher fps decays less, lower fps decays more, but the
///   "half-life" of energy at any reasonable rate is ~20 frames.
/// - `warmth` blends toward `autonomous_warmth_target(t)` with α=0.015
///   and is hard-clamped to [0.2, 0.8] so user touches can pull but
///   never push the palette outside the "still readable" range.
/// - Touches older than 2 s are dropped (stale) so a finger left
///   parked on the screen doesn't pin the palette.
pub fn tick(st: &mut TouchState, dt: f32, t: f32) -> FrameMood {
    // Energy decay — frame-rate normalized so the constant behaves the
    // same at 30 fps and 120 fps.
    let decay = 0.965f32.powf(dt * 60.0);
    st.energy *= decay;

    // Warmth drift toward the autonomous target.
    let warm_target = autonomous_warmth_target(t);
    st.warmth = (st.warmth * 0.985 + warm_target * 0.015).clamp(0.2, 0.8);

    // Drop stale contacts (>2 s old).
    let stale = st.last.map(|l| l.elapsed().as_secs() > 2).unwrap_or(true);
    if stale {
        st.contacts.clear();
        st.mouse = None;
    }

    // Mouse-as-touch counts as a contact too so a single button press
    // visibly registers in spawn rate, particle budget, and telemetry.
    let mouse_active = st.mouse.is_some();
    let live_contacts = st.contacts.len() + if mouse_active { 1 } else { 0 };

    FrameMood {
        warmth: st.warmth,
        energy: st.energy,
        idle: idle_energy(t),
        contacts: live_contacts,
        touch_device: st.device.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autonomous_warmth_stays_in_unit_interval() {
        for t in (0..10_000).map(|i| i as f32 * 0.001) {
            let w = autonomous_warmth_target(t);
            assert!((0.2..=0.8).contains(&w), "t={t} -> w={w}");
        }
    }

    #[test]
    fn hue_at_is_unit_interval() {
        for t in (0..10_000).map(|i| i as f32 * 0.001) {
            let h = hue_at(t, 0.5);
            assert!((0.0..1.0).contains(&h), "t={t} -> h={h}");
        }
    }

    #[test]
    fn idle_energy_is_bounded() {
        for t in (0..10_000).map(|i| i as f32 * 0.001) {
            let i = idle_energy(t);
            assert!((0.0..=0.25).contains(&i), "t={t} -> i={i}");
        }
    }

    #[test]
    fn effective_energy_is_at_least_idle() {
        for e in [0.0, 0.1, 0.5, 0.9] {
            let fm = FrameMood {
                warmth: 0.5,
                energy: e,
                idle: 0.1,
                contacts: 0,
                touch_device: String::new(),
            };
            assert!(fm.effective_energy() >= 0.1);
        }
    }
}
