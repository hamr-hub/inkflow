//! Curated content — five complete five-char Tang quatrains.
//!
//! Every theme is pinned 1:1 to one complete work, so the screen always
//! reads as one poem in reading order rather than a fragment pool.

/// Warmth/hush envelope for a line (both 0..1).
#[derive(Clone, Copy, Debug)]
pub struct Mood {
    pub warmth: f32,
    pub hush: f32,
}
impl Mood {
    pub const fn new(warmth: f32, hush: f32) -> Self {
        Self { warmth, hush }
    }
}

/// One inscribed line.
#[derive(Clone, Copy, Debug)]
pub struct Phrase {
    pub text: &'static str,
    pub mood: Mood,
    pub glow: f32,
}

const fn p(text: &'static str, warmth: f32, hush: f32, glow: f32) -> Phrase {
    Phrase {
        text,
        mood: Mood::new(warmth, hush),
        glow,
    }
}

/// All lines, grouped per work. Indices are stable (used by poem groups).
pub const PHRASES: &[Phrase] = &[
    // ── 贾岛《寻隐者不遇》
    p("松下问童子", 0.45, 0.75, 0.7),
    p("言师采药去", 0.40, 0.70, 0.6),
    p("只在此山中", 0.35, 0.85, 0.6),
    p("云深不知处", 0.20, 0.95, 0.5),
    // ── 柳宗元《江雪》
    p("千山鸟飞绝", 0.20, 0.95, 0.5),
    p("万径人踪灭", 0.15, 0.95, 0.4),
    p("孤舟蓑笠翁", 0.25, 0.90, 0.6),
    p("独钓寒江雪", 0.20, 0.95, 0.5),
    // ── 李白《静夜思》
    p("床前明月光", 0.45, 0.75, 0.7),
    p("疑是地上霜", 0.35, 0.85, 0.6),
    p("举头望明月", 0.30, 0.85, 0.6),
    p("低头思故乡", 0.55, 0.75, 0.7),
    // ── 王之涣《登鹳雀楼》
    p("白日依山尽", 0.45, 0.75, 0.7),
    p("黄河入海流", 0.50, 0.65, 0.8),
    p("欲穷千里目", 0.35, 0.85, 0.6),
    p("更上一层楼", 0.55, 0.60, 0.8),
    // ── 孟浩然《春晓》
    p("春眠不觉晓", 0.45, 0.70, 0.7),
    p("处处闻啼鸟", 0.40, 0.70, 0.7),
    p("夜来风雨声", 0.30, 0.80, 0.6),
    p("花落知多少", 0.50, 0.65, 0.7),
];

/// Look up a phrase by beat number.
pub fn phrase_for_beat(beat: u64) -> &'static Phrase {
    &PHRASES[(beat % PHRASES.len() as u64) as usize]
}

// ============================================================
// Themes — one complete work each
// ============================================================

pub const THEME_NAMES: &[&str] = &[
    "moonlit",
    "river-snow",
    "quiet-night",
    "stork-tower",
    "spring-dawn",
];

pub const THEMES: &[&[u16]] = &[
    &[0, 1, 2, 3],
    &[4, 5, 6, 7],
    &[8, 9, 10, 11],
    &[12, 13, 14, 15],
    &[16, 17, 18, 19],
];

/// Pick from `theme` while avoiding the given phrase indices.
pub fn pick_from_theme(rng: u32, theme: usize, avoid: &[u16]) -> &'static Phrase {
    pick_from_theme_by_len(rng, theme, usize::MAX, avoid)
}

/// Pick from `theme` a phrase whose char count is <= max_chars, avoiding the
/// given indices. A pool with no qualifying phrases falls back to the pool.
pub fn pick_from_theme_by_len(
    rng: u32,
    theme: usize,
    max_chars: usize,
    avoid: &[u16],
) -> &'static Phrase {
    let pool = THEMES[theme % THEMES.len()];
    let sub: Vec<u16> = pool
        .iter()
        .copied()
        .filter(|&i| PHRASES[i as usize].text.chars().count() <= max_chars)
        .collect();
    let candidates = if sub.is_empty() { pool } else { &sub };
    for attempt in 0..6u32 {
        let r = rng.wrapping_add(attempt.wrapping_mul(0x9E3779B1));
        let idx = candidates[r as usize % candidates.len()];
        if !avoid.contains(&idx) {
            return &PHRASES[idx as usize];
        }
    }
    &PHRASES[candidates[0] as usize]
}

// ============================================================
// Ordered poem groups
// ============================================================

#[derive(Copy, Clone)]
pub struct PoemGroup {
    pub title: &'static str,
    pub lines: &'static [u16],
}

pub const POEM_GROUPS: &[PoemGroup] = &[
    PoemGroup {
        title: "寻隐者不遇",
        lines: &[0, 1, 2, 3],
    },
    PoemGroup {
        title: "江雪",
        lines: &[4, 5, 6, 7],
    },
    PoemGroup {
        title: "静夜思",
        lines: &[8, 9, 10, 11],
    },
    PoemGroup {
        title: "登鹳雀楼",
        lines: &[12, 13, 14, 15],
    },
    PoemGroup {
        title: "春晓",
        lines: &[16, 17, 18, 19],
    },
];

/// Theme → poem group; every theme is pinned to a complete work.
pub const POEM_BY_THEME: &[usize] = &[0, 1, 2, 3, 4];

pub fn poem_group_line_indices(group: usize) -> &'static [u16] {
    if group >= POEM_GROUPS.len() {
        return &[];
    }
    POEM_GROUPS[group].lines
}

pub fn poem_group_title(group: usize) -> &'static str {
    if group >= POEM_GROUPS.len() {
        return "";
    }
    POEM_GROUPS[group].title
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph;

    #[test]
    fn every_line_unique_five_chars() {
        let mut seen = std::collections::HashSet::new();
        for phrase in PHRASES {
            assert_eq!(phrase.text.chars().count(), 5);
            assert!(seen.insert(phrase.text), "duplicate: {}", phrase.text);
            assert!(phrase.mood.warmth <= 1.0 && phrase.mood.hush <= 1.0);
        }
    }

    #[test]
    fn every_glyph_known() {
        for phrase in PHRASES {
            for cp in phrase.text.chars() {
                assert_ne!(glyph::index_for(cp as u32), 0, "missing {cp}");
            }
        }
    }

    #[test]
    fn phrase_wrap() {
        assert_eq!(
            phrase_for_beat(0).text,
            phrase_for_beat(PHRASES.len() as u64).text
        );
    }

    #[test]
    fn themes_and_groups_align() {
        assert_eq!(THEMES.len(), POEM_GROUPS.len());
        assert_eq!(THEMES.len(), POEM_BY_THEME.len());
        for (theme, &group) in POEM_BY_THEME.iter().enumerate() {
            assert_eq!(THEMES[theme], POEM_GROUPS[group].lines);
        }
        assert!(poem_group_line_indices(POEM_GROUPS.len()).is_empty());
        assert_eq!(poem_group_line_indices(0), POEM_GROUPS[0].lines);
    }

    #[test]
    fn pick_avoids_and_stays_in_pool() {
        let pool = THEMES[0];
        for seed in 0..32u32 {
            let picked = pick_from_theme(seed, 0, &[pool[0]]);
            assert_ne!(picked.text, PHRASES[pool[0] as usize].text);
            let in_pool = pool
                .iter()
                .any(|&i| PHRASES[i as usize].text == picked.text);
            assert!(in_pool);
        }
    }
}
