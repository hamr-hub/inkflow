// inkflow · fallback.rs
//
// Legacy fallback pools kept as documentation of the pre-v0.2.1 word bank.
// The current runtime picks fragments from `poetry::renderable_phrases()` instead.
// Marked `#[allow(dead_code)]` so the module compiles even though nothing
// in the binary references it — the comment is the value.
#![allow(dead_code)]

//
// Local-word pool + warm/cool/slow/fast picker.
//
// When ollama is unreachable, slow, or returning zero-token streams, the
// render loop still needs to keep painting. This module owns the static
// "mood pools" used by `local_glyph` and the picker that maps the
// current (warmth, energy) reading to a fragment of two-or-three
// characters. The pool contents are deliberately short CJK phrases —
// single characters like 雾 月 夜 — so the bitmap font can always
// render them without falling back to the "墨" sentinel.
//
// All output is `&'static str` because the pools are static literals.
// No allocation ever happens on this code path, so it is safe to call
// from inside the per-frame spawn loop on the render thread.
//
// Pool order: [静 (quiet), 动 (motion), 冷 (cool), 暖 (warm)] — the
// index constants mirror the original `POOLS[0..=3]` order so we don't
// have to change call sites. The pool contents were expanded in
// v0.2.1 with the 5-style framework from ARTIFACT.md (婉约 / 豪放 /
// 禅寂 / 稚拙 / 苍茫) — the picker now biases toward one of these
// depending on the (warmth, energy) reading so the local-fallback
// stream reads as stylistically coherent, not as a random pile of
// characters.

/// Quiet pool — low energy, low/no specific warmth direction.
pub const QUIET_WORDS: &str = "雾 月 夜 潮 呼吸 微光 沉睡 鲸落 尘埃 影 钟摆 雨前 纸页 \
     苔 林 木 叶 泉 雪落 远钟 云根 幽径 落花 鸿影 薄暮 清露 听蝉 听雪 静 寂 默 虚 远";

/// Motion pool — high energy, regardless of warmth.
pub const MOTION_WORDS: &str = "风 焰 河 奔 裂帛 星陨 心跳 浪尖 闪电 迁徙 鼓 惊鸟 火 \
     渡口 弦 雷 潮涌 雷鸣 烟火 龙吟 震颤 飞溅 雪崩 迸裂 翻涌 流火 疾行 疾 涌 旋 飙 怒";

/// Cool pool — low energy, cool direction.
pub const COOL_WORDS: &str = "雪 蓝 冰 星 霜 铁 墨 深空 孤 井 石英 冬 海沟 玻璃 \
     月背 银 寒 朔风 凝霜 寒潭 远岭 苍 凛 薄冰 星河 落雪 静海 寒 冽 清 凝 透 旷";

/// Warm pool — low energy, warm direction.
pub const WARM_WORDS: &str = "灯 橘 麦 陶 体温 琥珀 黄昏 花信 茧 炊烟 蜜 绒 烛 \
     岸 掌心 茶 暖 炉火 夕照 茶烟 旧书 木质 余温 棉 晨曦 晚风 温 软 融 润 透 曦";

/// Index constants for [`pool_for`]. Reserved for callers that
/// want to reference the pool order explicitly (e.g. a future
/// parameterised renderer).
#[allow(dead_code)]
pub mod idx {
    pub const QUIET: usize = 0;
    pub const MOTION: usize = 1;
    pub const COOL: usize = 2;
    pub const WARM: usize = 3;
}

/// Returns the pool string for the (warmth, energy, tick) triple. The
/// tick value is used as a stable seed so consecutive calls produce
/// different but reproducible fragments.
#[inline]
pub fn pool_for(warmth: f32, energy: f32, tick: u64) -> &'static str {
    // Every third tick picks from a motion-shaped pool, otherwise a
    // temperature-shaped pool. This mixes rhythm so the stream reads as
    // "occasionally charged, mostly calm" rather than "ticking between
    // two banks".
    if tick.is_multiple_of(3) {
        if energy > 0.5 {
            MOTION_WORDS
        } else {
            QUIET_WORDS
        }
    } else if warmth > 0.5 {
        WARM_WORDS
    } else {
        COOL_WORDS
    }
}

/// Pick one fragment from the appropriate bank for the current mood.
/// Returns `"墨"` only if the pool is empty (which never happens with
/// the static pools above; the sentinel exists so a future pool edit
/// can never panic the render thread).
#[inline]
pub fn local_glyph(warmth: f32, energy: f32, tick: u64) -> &'static str {
    let bank = pool_for(warmth, energy, tick);
    // `split_whitespace` on a `&'static str` allocates a `Vec`, which
    // would be a per-frame allocation in the spawn loop. Walk the slice
    // by hand to keep this zero-alloc — we only need the (n-th)
    // whitespace-delimited token.
    pick_word(bank, tick)
}

/// Knuth-style multiplicative hash so consecutive ticks land on
/// visually distinct fragments. The constant 2654435761 is the
/// 32-bit fixed-point golden ratio multiplier — produces a good
/// scatter on small integer seeds without pulling in a real RNG.
const HASH_MUL: u64 = 2654435761;

fn pick_word(bank: &'static str, tick: u64) -> &'static str {
    // Two passes: first count words, then return the n-th. For the
    // ~30-word pools we ship this is two cache-line scans — still
    // free compared to a single Vec allocation.
    let mut count = 0usize;
    for (i, _) in bank.split_whitespace().enumerate() {
        count = i + 1;
    }
    if count == 0 {
        return "墨";
    }
    let idx = ((tick.wrapping_mul(HASH_MUL)) as usize) % count;

    // Walk again, but return at idx. We can't safely use `.nth(idx)`
    // because the iterator is owned and we want a sub-slice of the
    // original `&'static str`.
    let mut cur = 0usize;
    let mut start = 0usize;
    let bytes = bank.as_bytes();
    let mut in_word = false;
    let mut last_word_end = bank.len();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b' ' || b == b'\t' || b == b'\n' {
            if in_word {
                if cur == idx {
                    return &bank[start..i];
                }
                cur += 1;
                in_word = false;
            }
        } else if !in_word {
            start = i;
            in_word = true;
        }
        last_word_end = i + 1;
    }
    if in_word && cur == idx {
        return &bank[start..last_word_end];
    }
    // Should be unreachable given count > 0 above. The sentinel keeps
    // the render loop safe even if a future edit introduces a bug.
    "墨"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bank_returns_sentinel() {
        assert_eq!(pick_word("", 0), "墨");
        assert_eq!(pick_word("   ", 7), "墨");
    }

    #[test]
    fn all_pools_nonempty() {
        assert!(!QUIET_WORDS
            .split_whitespace()
            .collect::<Vec<_>>()
            .is_empty());
        assert!(!MOTION_WORDS
            .split_whitespace()
            .collect::<Vec<_>>()
            .is_empty());
        assert!(!COOL_WORDS.split_whitespace().collect::<Vec<_>>().is_empty());
        assert!(!WARM_WORDS.split_whitespace().collect::<Vec<_>>().is_empty());
    }

    #[test]
    fn pick_is_deterministic() {
        for tick in [0u64, 1, 42, 12345, 987_654_321] {
            assert_eq!(
                pick_word(QUIET_WORDS, tick),
                pick_word(QUIET_WORDS, tick),
                "tick={tick}"
            );
        }
    }

    #[test]
    fn picks_only_words_in_pool() {
        let pool: std::collections::HashSet<&str> = QUIET_WORDS.split_whitespace().collect();
        for tick in 0..200u64 {
            let w = pick_word(QUIET_WORDS, tick);
            assert!(pool.contains(w), "tick {tick} -> {w:?} not in QUIET_WORDS");
        }
    }

    #[test]
    fn pool_for_is_deterministic() {
        assert_eq!(pool_for(0.5, 0.5, 0), pool_for(0.5, 0.5, 0));
        // tick % 3 == 0 picks motion/quiet branch
        assert_eq!(pool_for(0.5, 0.9, 3), MOTION_WORDS);
        assert_eq!(pool_for(0.5, 0.1, 3), QUIET_WORDS);
        // otherwise temperature branch
        assert_eq!(pool_for(0.9, 0.5, 1), WARM_WORDS);
        assert_eq!(pool_for(0.1, 0.5, 1), COOL_WORDS);
    }

    #[test]
    fn pools_have_at_least_30_words() {
        // Each pool must hold enough fragments to make 60 fps visually
        // distinct. Below 30 the LCG scatter visibly cycles.
        for (name, pool) in [
            ("QUIET", QUIET_WORDS),
            ("MOTION", MOTION_WORDS),
            ("COOL", COOL_WORDS),
            ("WARM", WARM_WORDS),
        ] {
            let n = pool.split_whitespace().count();
            assert!(n >= 30, "{name} has only {n} words (need >= 30)");
        }
    }
}
