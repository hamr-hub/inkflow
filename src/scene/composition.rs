//! Inscription composition — one hero line plus three supporting strokes
//! from the same poem in reading order.
//!
//! Pinned works (poem groups) hold every line on screen near peak alpha;
//! themes without a pinned work swap phrases into the supporting slots at
//! the end of each one's lifetime.

mod layout;
mod slot;

pub use layout::{BEATS_PER_LINE, BEATS_PER_WORK};
pub use slot::{Align, Slot, SlotDef, SlotRole};

use crate::phrase::{self, Phrase};

/// One hero plus three supporting slots.
pub struct Composition {
    pub slots: Vec<Slot>,
    pub hero_idx: usize,
    pub beats_since_theme: u32,
    next_support_to_refresh: usize,
    first_pinned_beat_done: bool,
}

impl Composition {
    pub fn default_layout() -> Self {
        let (slots, hero_idx) = layout::build_default();
        Self {
            slots,
            hero_idx,
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
        // After settling, alpha stays substantial relative to the slot's
        // declared peak — the ramp settles around 50 % of lifetime in
        // pinned mode, where both fade-in and fade-out are saturated.
        let max = c.slots[1].def.alpha;
        assert!(
            a2 > max * 0.5,
            "alpha settled to {a2} but expected > {} (half of max {max})",
            max * 0.5
        );
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
