//! Deterministic random source so the scene looks "natural" but stable across
//! runs. The same seed reproduces the same sequence, headless captures stay
//! pixel-stable across machines, and the touch bursts feel different every
//! time without ever feeling choreographed.

/// Linear congruential generator with a PCG-style multiplier + increment.
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
