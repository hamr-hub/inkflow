//! Beat scheduler — drives the phrase lifecycle (entrance → hold → exit) and
//! keeps the global pulse / warmth / tempo state.
//!
//! All time is in seconds; the engine does not allocate per frame. `Beat` is a
//! `Copy` value you can stash on the stack.

use crate::phrase::Phrase;

/// Phrase phase within the current beat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Empty / resting between phrases.
    Rest,
    /// Typing in / pop-in / slide-in.
    Entrance,
    /// Static readable hold.
    Hold,
    /// Fade / slide / collapse out.
    Exit,
}

/// Tunable beat schedule (seconds). `enter + hold + exit + rest` is one cycle.
#[derive(Clone, Copy, Debug)]
pub struct Tempo {
    pub enter: f32, // entrance duration
    pub hold: f32,  // hold (read) duration
    pub exit: f32,  // exit duration
    /// Breath between phrases — the previous hero stays rendered at a
    /// low "ghost" alpha during this gap so the page never loses its
    /// focal anchor (ART_DIRECTION §一 "可读性：任何帧截图都能看清句子").
    pub rest: f32,
    /// Effective cycle length (enter + hold + exit + rest with warmth
    /// modulation). Kept as a derived mirror so existing readers
    /// (tests, telemetry) still see a sensible "beat period" value.
    pub live_period: f32,
    /// Warmth in [0,1] — compresses `rest` by up to 25 %.
    pub warmth: f32,
    /// Pulse in [0,1] — momentary spike on each new beat.
    pub pulse: f32,
}

impl Default for Tempo {
    fn default() -> Self {
        let enter = 0.55_f32;
        let hold = 2.65_f32;
        let exit = 0.80_f32;
        let rest = 1.5_f32;
        Self {
            enter,
            hold,
            exit,
            rest,
            live_period: enter + hold + exit + rest,
            warmth: 0.0,
            pulse: 0.0,
        }
    }
}

impl Tempo {
    /// Apply a touch tap — bump warmth, pulse, slightly compress the rest gap.
    pub fn touch(&mut self, magnitude: f32) {
        let m = magnitude.clamp(0.0, 1.0);
        self.warmth = (self.warmth + m * 0.55).clamp(0.0, 1.0);
        self.pulse = (self.pulse + m).clamp(0.0, 1.0);
        // Compress the breath by up to 25 % proportional to warmth, so a
        // touched scene reads as "the next phrase is coming sooner" while
        // the entrance / hold / exit phases stay calm. Floor at 0.3 so the
        // ghost phase never collapses below a single breath.
        let factor = 1.0 - 0.25 * self.warmth;
        self.rest = (self.rest * factor).max(0.3);
        self.live_period = self.enter + self.hold + self.exit + self.rest;
    }
    /// Sustain tick — warmth decays, pulse decays, rest springs back.
    pub fn relax(&mut self, dt: f32) {
        self.warmth = (self.warmth - dt * 0.06).max(0.0);
        self.pulse = (self.pulse - dt * 0.55).max(0.0);
        // Spring the breath back to the calm 1.5 s register after touch.
        self.rest += (1.5 - self.rest) * (dt * 0.4).min(1.0);
        self.live_period = self.enter + self.hold + self.exit + self.rest;
    }
}

/// State of the phrase on screen, kept tiny and `Copy`.
#[derive(Clone, Copy, Debug)]
pub struct Beat {
    pub index: u64, // monotonically increasing
    pub phase: Phase,
    pub t_in_phase: f32, // seconds elapsed inside the current phase
    pub t_in_beat: f32,  // seconds since this beat started
    pub phrase: &'static Phrase,
    pub enter: f32, // snapshot of the phase length when the beat started
    pub hold: f32,
    pub exit: f32,
}

impl Beat {
    pub fn entrance_progress(&self) -> f32 {
        let p = self.t_in_phase / self.enter.max(0.0001);
        p.clamp(0.0, 1.0)
    }
    pub fn exit_progress(&self) -> f32 {
        let p = self.t_in_phase / self.exit.max(0.0001);
        p.clamp(0.0, 1.0)
    }
    /// Hold completeness 0..1 over the hold phase.
    pub fn hold_progress(&self) -> f32 {
        let p = self.t_in_phase / self.hold.max(0.0001);
        p.clamp(0.0, 1.0)
    }
}

/// Phrase lifecycle engine. Holds the current `Beat` and produces the next one.
pub struct Engine {
    pub tempo: Tempo,
    pub beat: Option<Beat>,
    /// Total beats seen so far.
    pub beat_count: u64,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            tempo: Tempo::default(),
            beat: None,
            beat_count: 0,
        }
    }

    pub fn current_tempo(&self) -> Tempo {
        self.tempo
    }

    /// Advance by `dt` seconds. Returns the *new* current beat (or None
    /// before the first `force_beat()` call). Mutates `self.beat` in place.
    /// Once a beat is alive the engine keeps `self.beat = Some(_)` across
    /// every phase including the post-exit `Rest`, so the focal line stays
    /// rendered at a low "ghost" alpha between phrases — the page never
    /// loses its anchor.
    pub fn advance(&mut self, dt: f32) -> Option<Beat> {
        self.tempo.relax(dt);
        match self.beat {
            None => {
                // Pre-first-beat idle. `force_beat()` is the only path that
                // starts the very first beat.
            }
            Some(mut b) => {
                b.t_in_beat += dt;
                b.t_in_phase += dt;
                // Phase transitions use the durations snapshotted at beat start
                // so tempo changes mid-beat don't yank the phase.
                let e = b.enter;
                let h = b.hold;
                let x = b.exit;
                let r = self.tempo.rest;
                match b.phase {
                    Phase::Entrance if b.t_in_phase >= e => {
                        b.phase = Phase::Hold;
                        b.t_in_phase = 0.0;
                    }
                    Phase::Hold if b.t_in_phase >= h => {
                        b.phase = Phase::Exit;
                        b.t_in_phase = 0.0;
                    }
                    Phase::Exit if b.t_in_phase >= x => {
                        // Beat exit complete — hold the previous phrase as a
                        // low-alpha "ghost rest" instead of dropping the
                        // focal line entirely. The next beat will replace
                        // it via start_beat() once the breath elapses.
                        b.phase = Phase::Rest;
                        b.t_in_phase = 0.0;
                    }
                    Phase::Rest if b.t_in_phase >= r => {
                        // Ghost rest complete — start the next beat.
                        self.start_beat();
                        return self.beat;
                    }
                    _ => {}
                }
                self.beat = Some(b);
            }
        }
        self.beat
    }

    fn start_beat(&mut self) {
        let p = crate::phrase::phrase_for_beat(self.beat_count);
        self.beat = Some(Beat {
            index: self.beat_count,
            phase: Phase::Entrance,
            t_in_phase: 0.0,
            t_in_beat: 0.0,
            phrase: p,
            enter: self.tempo.enter,
            hold: self.tempo.hold,
            exit: self.tempo.exit,
        });
        // Each new beat pulses.
        self.tempo.pulse = (self.tempo.pulse + 0.55).clamp(0.0, 1.0);
        self.beat_count += 1;
    }

    /// Force a beat to start immediately. Used by the headless harness so
    /// frame 0 already shows the entrance.
    pub fn force_beat(&mut self) {
        self.start_beat();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn touch_warms_and_speeds() {
        let mut t = Tempo::default();
        let orig = t.live_period;
        t.touch(1.0);
        assert!(t.warmth > 0.5);
        assert!(t.live_period < orig);
    }
    #[test]
    fn phase_progress() {
        let mut e = Engine::new();
        e.force_beat();
        let b = e.beat.unwrap();
        assert_eq!(b.phase, Phase::Entrance);
        assert!(b.entrance_progress() >= 0.0);
    }
}
