//! Curated phrase bank — short, evocative Chinese fragments.
//!
//! Phrases are 2–8 characters with actual sense (not isolated single glyphs).
//! Each entry also carries a "mood" tag the renderer can use to gently tint
//! the palette. Mood is a `(warmth, hush)` tuple in [0,1] — warmth shifts the
//! palette toward amber/cream; hush pushes it toward cooler shadow tones.

/// 2-D mood vector: warmth (warm vs cool) and hush (active vs still).
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

/// A single phrase: UTF-8 string + mood + optional `glow` modifier (intensifies
/// outer glow on the beat).
#[derive(Debug)]
pub struct Phrase {
    pub text: &'static str,
    pub mood: Mood,
    pub glow: f32,
}

/// Hand-curated phrase bank.
///
/// `hush` 0..=1 — high means quiet/still phrases, low means more active.
/// `warmth` 0..=1 — high means warm/red, low means cool/blue.
pub const PHRASES: &[Phrase] = &[
    // ── 2 char
    Phrase {
        text: "月色",
        mood: Mood::new(0.20, 0.85),
        glow: 0.7,
    },
    Phrase {
        text: "静夜",
        mood: Mood::new(0.10, 0.95),
        glow: 0.6,
    },
    Phrase {
        text: "无声",
        mood: Mood::new(0.05, 1.00),
        glow: 0.5,
    },
    Phrase {
        text: "寒山",
        mood: Mood::new(0.15, 0.90),
        glow: 0.5,
    },
    Phrase {
        text: "空山",
        mood: Mood::new(0.05, 0.95),
        glow: 0.5,
    },
    Phrase {
        text: "远海",
        mood: Mood::new(0.30, 0.80),
        glow: 0.6,
    },
    Phrase {
        text: "落花",
        mood: Mood::new(0.55, 0.70),
        glow: 0.7,
    },
    Phrase {
        text: "暮雪",
        mood: Mood::new(0.25, 0.85),
        glow: 0.6,
    },
    Phrase {
        text: "春风",
        mood: Mood::new(0.55, 0.40),
        glow: 0.8,
    },
    Phrase {
        text: "长夜",
        mood: Mood::new(0.05, 1.00),
        glow: 0.4,
    },
    Phrase {
        text: "朝露",
        mood: Mood::new(0.40, 0.85),
        glow: 0.6,
    },
    Phrase {
        text: "寒灯",
        mood: Mood::new(0.75, 0.80),
        glow: 0.9,
    },
    // ── 3 char
    Phrase {
        text: "风过无声",
        mood: Mood::new(0.20, 0.95),
        glow: 0.5,
    },
    Phrase {
        text: "月色入海",
        mood: Mood::new(0.30, 0.85),
        glow: 0.7,
    },
    Phrase {
        text: "万物静默",
        mood: Mood::new(0.10, 1.00),
        glow: 0.5,
    },
    Phrase {
        text: "山色如梦",
        mood: Mood::new(0.40, 0.80),
        glow: 0.7,
    },
    Phrase {
        text: "雨落无声",
        mood: Mood::new(0.15, 0.95),
        glow: 0.5,
    },
    Phrase {
        text: "灯火可亲",
        mood: Mood::new(0.85, 0.55),
        glow: 0.9,
    },
    Phrase {
        text: "一念万年",
        mood: Mood::new(0.05, 0.95),
        glow: 0.4,
    },
    Phrase {
        text: "夜凉如水",
        mood: Mood::new(0.10, 0.90),
        glow: 0.5,
    },
    Phrase {
        text: "此心光明",
        mood: Mood::new(0.65, 0.70),
        glow: 0.8,
    },
    Phrase {
        text: "光阴似水",
        mood: Mood::new(0.35, 0.75),
        glow: 0.6,
    },
    // ── 4 char
    Phrase {
        text: "万物静默如谜",
        mood: Mood::new(0.10, 1.00),
        glow: 0.5,
    },
    Phrase {
        text: "行到水穷处",
        mood: Mood::new(0.25, 0.85),
        glow: 0.6,
    },
    Phrase {
        text: "坐看云起时",
        mood: Mood::new(0.45, 0.70),
        glow: 0.7,
    },
    Phrase {
        text: "星河长明",
        mood: Mood::new(0.20, 0.90),
        glow: 0.6,
    },
    Phrase {
        text: "岁月如歌",
        mood: Mood::new(0.55, 0.55),
        glow: 0.8,
    },
    Phrase {
        text: "山高月小",
        mood: Mood::new(0.30, 0.85),
        glow: 0.6,
    },
    Phrase {
        text: "水落石出",
        mood: Mood::new(0.20, 0.80),
        glow: 0.5,
    },
    Phrase {
        text: "花开花落",
        mood: Mood::new(0.55, 0.65),
        glow: 0.7,
    },
    Phrase {
        text: "春去秋来",
        mood: Mood::new(0.40, 0.60),
        glow: 0.6,
    },
    Phrase {
        text: "海天一色",
        mood: Mood::new(0.30, 0.80),
        glow: 0.6,
    },
    Phrase {
        text: "风清月白",
        mood: Mood::new(0.25, 0.85),
        glow: 0.6,
    },
    Phrase {
        text: "千里共婵娟",
        mood: Mood::new(0.55, 0.75),
        glow: 0.8,
    },
    Phrase {
        text: "灯火阑珊",
        mood: Mood::new(0.75, 0.70),
        glow: 0.9,
    },
    Phrase {
        text: "浮生若梦",
        mood: Mood::new(0.45, 0.85),
        glow: 0.7,
    },
    Phrase {
        text: "且听风吟",
        mood: Mood::new(0.40, 0.75),
        glow: 0.7,
    },
    Phrase {
        text: "渔舟唱晚",
        mood: Mood::new(0.65, 0.55),
        glow: 0.8,
    },
    Phrase {
        text: "禅心若雪",
        mood: Mood::new(0.30, 0.90),
        glow: 0.6,
    },
    Phrase {
        text: "暮色苍茫",
        mood: Mood::new(0.45, 0.75),
        glow: 0.7,
    },
    // ── 5 char
    Phrase {
        text: "松下问童子",
        mood: Mood::new(0.50, 0.70),
        glow: 0.7,
    },
    Phrase {
        text: "言师采药去",
        mood: Mood::new(0.45, 0.65),
        glow: 0.6,
    },
    Phrase {
        text: "只在此山中",
        mood: Mood::new(0.35, 0.85),
        glow: 0.6,
    },
    Phrase {
        text: "云深不知处",
        mood: Mood::new(0.20, 0.95),
        glow: 0.5,
    },
    Phrase {
        text: "江上数峰青",
        mood: Mood::new(0.40, 0.80),
        glow: 0.7,
    },
    Phrase {
        text: "鸟宿池边树",
        mood: Mood::new(0.40, 0.85),
        glow: 0.6,
    },
    Phrase {
        text: "僧敲月下门",
        mood: Mood::new(0.55, 0.80),
        glow: 0.8,
    },
    // ── 6 char
    Phrase {
        text: "夜深知雪重",
        mood: Mood::new(0.30, 0.90),
        glow: 0.6,
    },
    Phrase {
        text: "时闻折竹声",
        mood: Mood::new(0.45, 0.85),
        glow: 0.7,
    },
    Phrase {
        text: "孤舟蓑笠翁",
        mood: Mood::new(0.35, 0.85),
        glow: 0.6,
    },
    Phrase {
        text: "独钓寒江雪",
        mood: Mood::new(0.25, 0.95),
        glow: 0.5,
    },
    Phrase {
        text: "大江东去",
        mood: Mood::new(0.50, 0.50),
        glow: 0.8,
    },
    Phrase {
        text: "浪淘尽千古",
        mood: Mood::new(0.45, 0.55),
        glow: 0.8,
    },
    Phrase {
        text: "花未全开月未圆",
        mood: Mood::new(0.55, 0.75),
        glow: 0.8,
    },
    Phrase {
        text: "人生若只如初见",
        mood: Mood::new(0.55, 0.70),
        glow: 0.8,
    },
    // ── 7 char
    Phrase {
        text: "春风又绿江南岸",
        mood: Mood::new(0.60, 0.50),
        glow: 0.8,
    },
    Phrase {
        text: "明月何时照我还",
        mood: Mood::new(0.45, 0.75),
        glow: 0.7,
    },
    Phrase {
        text: "故人入我梦",
        mood: Mood::new(0.50, 0.80),
        glow: 0.7,
    },
    Phrase {
        text: "明我长相忆",
        mood: Mood::new(0.55, 0.75),
        glow: 0.7,
    },
    Phrase {
        text: "不敢高声语",
        mood: Mood::new(0.45, 0.85),
        glow: 0.6,
    },
    // ── 8 char
    Phrase {
        text: "不敢高声语恐惊天上人",
        mood: Mood::new(0.50, 0.90),
        glow: 0.7,
    },
    Phrase {
        text: "海上生明月",
        mood: Mood::new(0.35, 0.80),
        glow: 0.6,
    },
    Phrase {
        text: "天涯共此时",
        mood: Mood::new(0.55, 0.70),
        glow: 0.8,
    },
    Phrase {
        text: "举头望明月",
        mood: Mood::new(0.45, 0.75),
        glow: 0.7,
    },
    Phrase {
        text: "低头思故乡",
        mood: Mood::new(0.55, 0.80),
        glow: 0.7,
    },
];

/// Look up a phrase index by beat number. Wraps around; advance each beat.
pub fn phrase_for_beat(beat: u64) -> &'static Phrase {
    let n = PHRASES.len() as u64;
    &PHRASES[(beat % n) as usize]
}

/// Owned counterpart to `Phrase` — used by `parse_line`, which receives an
/// arbitrary `&str` and cannot return a `'static` borrow.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OwnedPhrase {
    pub text: String,
    pub mood: Mood,
    pub glow: f32,
}

/// Parse the `--phrases` file format: one phrase per line, optionally followed
/// by `; warmth,hush,glow`. Used only when caller wants to inject custom
/// phrases; curated bank is always the fallback.
#[allow(dead_code)]
pub fn parse_line(s: &str) -> Option<OwnedPhrase> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    match s.split_once(';') {
        Some((t, m)) => {
            let mut it = m.trim().split(',');
            let w: f32 = it.next().unwrap_or("0.5").trim().parse().unwrap_or(0.5);
            let h: f32 = it.next().unwrap_or("0.5").trim().parse().unwrap_or(0.5);
            let g: f32 = it.next().unwrap_or("0.6").trim().parse().unwrap_or(0.6);
            Some(OwnedPhrase {
                text: t.trim().to_string(),
                mood: Mood::new(w.clamp(0.0, 1.0), h.clamp(0.0, 1.0)),
                glow: g.clamp(0.0, 1.0),
            })
        }
        None => Some(OwnedPhrase {
            text: s.to_string(),
            mood: Mood::new(0.5, 0.7),
            glow: 0.6,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph;

    #[test]
    fn curated_bank_covers_2_to_8_chars() {
        let mut seen = std::collections::HashSet::new();
        for p in PHRASES {
            let chars = p.text.chars().count();
            assert!(
                (2..=8).contains(&chars),
                "phrase out of range: {} ({} chars)",
                p.text,
                chars
            );
            assert!(seen.insert(p.text), "duplicate phrase: {}", p.text);
        }
    }
    #[test]
    fn every_glyph_known() {
        for p in PHRASES {
            for cp in p.text.chars() {
                let u = cp as u32;
                let _ = glyph::index_for(u);
            }
        }
    }
    #[test]
    fn phrase_wrap() {
        let a = phrase_for_beat(0).text;
        let b = phrase_for_beat(PHRASES.len() as u64).text;
        assert_eq!(a, b);
    }
}
