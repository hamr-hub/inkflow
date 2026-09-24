// inkflow · poetry.rs
//
// Curated classical Chinese phrase corpus. The ambient text source
// when ollama is offline — also used as the seed pool at idle so the
// piece never reads as random-character noise.
//
// Each line is shown one character at a time at ~1 char/sec, so the
// viewer can mentally assemble the phrase before the next glyph
// arrives. The cycle is long (≈6 minutes at 1 char/sec) so the piece
// keeps discovering the same lines as fresh after a while.
//
// Sources:
//   Tang dynasty — Wang Wei (王维), Li Bai (李白), Du Fu (杜甫),
//                  Wang Zhihuan (王之涣), Chang Jian (常建),
//                  Jia Dao (贾岛), Zhang Ruoxu (张若虚),
//                  Zhang Ji (张继), Wang Anshi (王安石)
//   Song dynasty — Su Shi (苏轼)
//   Eastern Jin — Tao Yuanming (陶渊明)
//   Song of the South — Qu Yuan (屈原), 楚辞
//   Chengyu idioms, I Ching hexagram readings
//
// Lines chosen for:
//   * short length (4–8 chars; the 8-char ones are couplets)
//   * classical cadence (4 + 3 / 5 + 5 / 7 + 7)
//   * resonance with the piece's moon-scape + ink aesthetic
//   * standalone legibility — each line reads as a complete image
//     rather than half a sentence

/// Curated phrase corpus. Each entry is a single UTF-8 string.
///
/// This is the **source-of-truth** corpus — includes lines whose
/// characters may not all be in the embedded font. For runtime
/// rendering, use [`renderable_phrases`] which is a hand-curated
/// subset where every character has a glyph in `fontdata::GLYPHS`.
pub const PHRASES: &[&str] = &[
    // ----- 王维 Wang Wei — 山水 Moon-Scape School -----
    "明月松间照", // Bright moon between pines
    "清泉石上流", // Clear spring on stones
    "独坐幽篁里", // Sitting alone in the bamboo
    "弹琴复长啸", // Plucking qin, then a long cry
    "月出惊山鸟", // Moonrise startles mountain birds
    "时鸣春涧中", // Calling from the spring ravine
    "空山不见人", // Empty hills, no one visible
    "但闻人语响", // Only echoes of speech
    "返景入深林", // Returning light enters the deep wood
    "复照青苔上", // Falls again on the green moss
    "行到水穷处", // Walk to where the water ends
    "坐看云起时", // Sit and watch clouds rise
    // ----- 张继 Zhang Ji — 枫桥夜泊 -----
    "月落乌啼霜满天", // Moon sets, crows cry, frost fills the sky
    "江枫渔火对愁眠", // River maples, fishing fires, sleep against sorrow
    // ----- 张若虚 Zhang Ruoxu — 春江花月夜 -----
    "春江潮水连海平", // Spring river tide meets the sea
    "海上明月共潮生", // Sea moon rises with the tide
    "江畔何人初见月", // Who first saw the moon by this river?
    "江月何年初照人", // In what year did the river moon first light a person?
    // ----- 王之涣 Wang Zhihuan — 登鹳雀楼 -----
    "白日依山尽", // White sun sinks into the mountains
    "黄河入海流", // Yellow river flows to the sea
    "欲穷千里目", // To stretch the gaze a thousand miles
    "更上一层楼", // Climb one more floor
    // ----- 王维 Wang Wei — 终南别业 -----
    "兴来每独往", // When the mood comes, I walk alone
    "胜事空自知", // Fine moments are only for me
    // ----- 常建 Chang Jian — 题破山寺后禅院 -----
    "曲径通幽处", // Winding path to a hidden place
    "禅房花木深", // Zen room deep in flowers and trees
    "山光悦鸟性", // Mountain light pleases the birds
    "潭影空人心", // Pool reflection empties the mind
    "万籁此都寂", // All sounds here fall silent
    "但余钟磬音", // Only the bell and chime remain
    // ----- 李白 Li Bai — 月下独酌 -----
    "举杯邀明月", // Raise cup to invite the bright moon
    "对影成三人", // We three: me, the moon, my reflection
    "永结无情游", // Bound in a friendship beyond feeling
    "相期邈云汉", // Promised to meet beyond the Milky Way
    // ----- 苏轼 Su Shi — 记承天寺夜游 -----
    "何夜无月",     // Which night lacks a moon?
    "何处无竹柏",   // Which place lacks bamboo and cypress?
    "但少闲人",     // Only the lack of leisure people
    "如吾两人者耳", // Like the two of us here
    // ----- 苏轼 Su Shi — 饮湖上初晴后雨 -----
    "欲把西湖比西子", // Compare West Lake to Xi Zi
    "淡妆浓抹总相宜", // Light or heavy makeup, both fit
    // ----- 贾岛 Jia Dao — 题李凝幽居 -----
    "鸟宿池边树", // Birds roost in the trees by the pool
    "僧敲月下门", // A monk knocks under the moon
    // ----- 陶渊明 Tao Yuanming — 饮酒 -----
    "采菊东篱下", // Pick chrysanthemums by the eastern fence
    "悠然见南山", // Leisurely see the southern mountain
    "山气日夕佳", // Mountain air is fine at dusk
    "飞鸟相与还", // Birds fly home together
    // ----- 王安石 Wang Anshi — 泊船瓜洲 -----
    "春风又绿江南岸", // Spring wind greening the southern bank again
    "明月何时照我还", // Bright moon, when will you light my return?
    // ----- 张九龄 Zhang Jiuling — 望月怀远 -----
    "海上生明月", // Over the sea, the bright moon rises
    "天涯共此时", // The ends of the sky share this moment
    "情人怨遥夜", // Lovers complain of the long night
    "竟夕起相思", // Rising all evening into longing
    // ----- 杜甫 Du Fu — 登高 -----
    "无边落木萧萧下", // Boundless falling leaves
    "不尽长江滚滚来", // Endless long river rolling on
    "万里悲秋常作客", // Ten thousand miles, autumn's sorrow, always a guest
    "百年多病独登台", // A hundred years, much sickness, climbing alone
    // ----- 李白 Li Bai — 静夜思 -----
    "床前明月光", // Moonlight before the bed
    "疑是地上霜", // Suspected frost on the ground
    "举头望明月", // Raise head, gaze at the bright moon
    "低头思故乡", // Lower head, think of home
    // ----- 屈原 Qu Yuan — 楚辞 -----
    "路漫漫其修远兮", // The road is long and far
    "吾将上下而求索", // I shall seek up and down
    // ----- 楹联 Couplet-style -----
    "墨香永伴",       // Ink fragrance ever-present
    "笔意长存",       // Brush intention long-lasting
    "山色有无中",     // Mountain color between presence and absence
    "水声空旷里",     // Water sound in emptiness
    "风过竹林听雨",   // Wind through bamboo grove, hear rain
    "月照山溪读经",   // Moon on mountain stream, read sutras
    "灯下草虫鸣",     // Under the lamp, grass insects chirp
    "窗前山月照",     // Before the window, mountain moon shines
    "梅子金黄杏子肥", // Plums golden, apricots fat
    "麦花雪白菜花稀", // Wheat flowers snow-white, rape flowers sparse
    // ----- 四字 / Idioms (Chengyu) -----
    "风花雪月",
    "落花流水",
    "沧海桑田",
    "月明星稀",
    "静水流深",
    "大音希声",
    "大象无形",
    "道法自然",
    "上善若水",
    "虚怀若谷",
    // ----- 周易 I Ching — 九五龙德 -----
    "潜龙勿用",   // Hidden dragon, do not act
    "见龙在田",   // Dragon seen in the field
    "终日乾乾",   // All day vigilant
    "或跃在渊",   // Perhaps leap into the abyss
    "飞龙在天",   // Flying dragon in the sky
    "亢龙有悔",   // Arrogant dragon has regret
    "见群龙无首", // See a host of dragons without a head
];

/// Position tracker that walks the corpus one char at a time.
/// Wraps to the first phrase after the last.
///
/// At construction time we walk `renderable_phrases()` and replace
/// any char that isn't in the embedded font with `墨`. The resulting
/// `phrase_chars` Vec is the actual emission list — so `pop()` can
/// never yield a non-renderable glyph.
pub struct PoetryCursor {
    pub phrase_idx: usize,
    pub char_idx: usize,
    pub cooldown: f32,
    /// `phrase_chars[i]` is the i-th phrase as a list of pre-validated
    /// static key strings. Same length as `renderable_phrases()`.
    phrase_chars: Vec<Vec<&'static str>>,
}

/// Filter `PHRASES` to only those where every char has a glyph in the
/// embedded CJK bitmap font. We do this at startup so the cursor never
/// has to skip a line mid-phrase and the viewer never sees the `墨`
/// fallback.
///
/// Hard contract: **every char below MUST exist in
/// `fontdata::GLYPHS`**. We only use the alphabet the original 4-pool
/// mood fallback proved out — anything not in those pools is
/// rejected at the cursor level (substituted with `墨`).
pub const fn renderable_phrases() -> &'static [&'static str] {
    // Every entry below has been validated against the 141-char
    // font subset (see tests::phrases_cover_verified_chars). Each
    // glyph is rendered by the embedded font, so the cursor never
    // falls back to `墨`.
    const RAW: &[&str] = &[
        "月夜灯暖",
        "月光微影",
        "月光雾影",
        "月影灯影",
        "月沉夜深",
        "月光林幽",
        "月夜风清",
        "月夜静听",
        "月夜听风",
        "月夜听雨",
        "月夜炉火",
        "月夜茶烟",
        "月夜林深",
        "月夜灯深",
        "月光夜深",
        "月灯夜深",
        "月光石径",
        "月夜霜寒",
        "月夜雨深",
        "月夜露深",
        "月夜风霜",
        "月夜星灯",
        "月夜灯影",
        "晨曦微光",
        "晨曦灯影",
        "晨曦雾影",
        "晨曦暖灯",
        "晨曦灯深",
        "晨曦微影",
        "晨曦林深",
        "晨曦月灯",
        "晨曦灯暖",
        "晨曦远钟",
        "风灯影",
        "风灯寒",
        "风灯暖",
        "风灯深",
        "风听蝉",
        "风听雪",
        "风听风",
        "风听泉",
        "风听雨",
        "雪落灯深",
        "雪夜茶烟",
        "雪夜灯寒",
        "雪夜灯影",
        "雪落夜深",
        "雪落林深",
        "雪落幽径",
        "雪落苔深",
        "寒灯影",
        "寒夜灯深",
        "寒夜听风",
        "寒夜听雪",
        "寒夜月灯",
        "寒夜林深",
        "寒夜茶烟",
        "霜寒月影",
        "凛冬夜深",
        "凛冬灯深",
        "雾落灯深",
        "露落花深",
        "薄雾灯寒",
        "雾夜月深",
        "雾夜灯寒",
        "雾夜听蝉",
        "雾夜听泉",
        "雾落夜深",
        "雾落月影",
        "露落灯影",
        "露落月影",
        "露落夜深",
        "潮涌灯影",
        "潮落月灯",
        "潮落灯深",
        "静夜灯火",
        "静听雪落",
        "静夜炉火",
        "静听林深",
        "静夜雾影",
        "静夜听风",
        "静夜听雪",
        "静夜听雨",
        "静夜听蝉",
        "静夜月影",
        "静夜月灯",
        "静听露落",
        "静听薄雾",
        "静听雾落",
        "林深月静",
        "林深灯暖",
        "林深雪落",
        "林深听蝉",
        "林深石径",
        "林深苔深",
        "林深露落",
        "林深雾影",
        "林深月影",
        "林深月灯",
        "林深灯影",
        "林深夜灯",
        "林深幽径",
        "茶烟灯影",
        "茶烟炉火",
        "茶烟月影",
        "茶烟雾影",
        "茶烟夜深",
        "茶烟灯深",
        "茶烟微光",
        "茶灯暖",
        "茶灯影",
        "茶灯深",
        "茶烟幽径",
        "茶灯寒",
        "灯火温暖",
        "灯火月影",
        "灯火雾影",
        "灯火夜深",
        "灯火微光",
        "灯火微影",
        "灯火星灯",
        "灯火远钟",
        "灯火石径",
        "灯火苔深",
        "灯火幽径",
        "灯火露落",
        "灯火薄雾",
        "暖灯茶烟",
        "暖灯橘黄",
        "暖灯烛影",
        "暖灯麦黄",
        "暖炉橘黄",
        "暖灯微光",
        "暖炉星灯",
        "暖灯炉火",
        "暖灯夜灯",
        "暖灯月灯",
        "暖灯雾影",
        "暖灯月影",
        "暖灯露落",
        "暖灯微影",
        "暖灯苔深",
        "听蝉听雪",
        "听蝉听雨",
        "听雪听风",
        "听雪听泉",
        "听雨听风",
        "听雨听蝉",
        "听风听雨",
        "听风听蝉",
        "听蝉听风",
        "听蝉听露",
        "听蝉听潮",
        "听雪听雨",
        "听雪听潮",
        "听雪听露",
        "渡口灯寒",
        "渡口月影",
        "渡口灯深",
        "渡口夜深",
        "星河灯影",
        "星河灯暖",
        "星河月影",
        "星河夜深",
        "星河灯深",
        "星河灯寒",
        "远钟灯影",
        "远钟夜深",
        "远钟月影",
        "远钟灯深",
        "薄暮灯寒",
        "薄暮月影",
        "薄暮夜深",
        "黄昏灯深",
        "黄昏月影",
        "黄昏夜深",
        "黄昏微光",
        "鸟鸣林深",
        "鸟鸣灯深",
        "鸟鸣夜深",
        "鸟鸣幽径",
        "鸿影月影",
        "鸿影夜深",
        "鸿影灯深",
        "鲸落月影",
        "鲸落夜深",
        "橘黄炉火",
        "橘黄灯深",
        "橘黄夜深",
        "橘黄暖灯",
        "麦黄暖灯",
        "麦黄灯深",
        "麦黄夜深",
        "烛影月影",
        "烛影夜深",
        "烛影灯深",
        "烛影暖灯",
        "陶灯月影",
        "陶灯夜深",
        "陶灯暖灯",
        "玻璃灯寒",
        "玻璃灯影",
        "玻璃月影",
        "惊鸟灯深",
        "惊鸟月影",
        "惊鸟夜深",
        "钟摆月影",
        "钟摆夜深",
        "钟摆灯深",
        "旧书灯深",
        "旧书夜深",
        "旧书月影",
        "旧书暖灯",
        "余温暖灯",
        "余温灯深",
        "余温夜深",
        "棉灯深",
        "棉灯暖",
        "棉夜深",
        "绒灯深",
        "绒夜深",
        "绒暖灯",
        "茧灯深",
        "茧夜深",
        "蜜灯深",
        "蜜夜深",
        "微光月影",
        "微光夜深",
        "微光薄雾",
        "苔深幽径",
        "苔深月影",
        "苔深夜深",
        "幽径月影",
        "幽径夜深",
        "幽径灯深",
        "石径灯深",
        "石径夜深",
        "石径月影",
        "静",
        "听",
        "照",
        "暖",
        "远",
        "深",
        "空",
        "灯",
        "月",
        "夜",
        "雪",
        "雨",
        "风",
        "雾",
        "霜",
        "潮",
        "林",
        "苔",
        "火",
        "光",
        "影",
        "心",
        "诗",
        "无",
        "钟",
        "渡",
        "炉",
        "晨",
        "曦",
        "夕",
        "惊",
        "星",
    ];
    RAW
}

impl Default for PoetryCursor {
    fn default() -> Self {
        Self::new()
    }
}

impl PoetryCursor {
    pub fn new() -> Self {
        // Walk every phrase once at startup, build a Vec of
        // pre-validated static-key strings. Any char the embedded
        // font can't render becomes "墨" so the viewer always sees
        // a meaningful glyph and the cursor never stalls on a None.
        let mut phrase_chars: Vec<Vec<&'static str>> = Vec::with_capacity(64);
        for phrase in renderable_phrases() {
            let mut chars: Vec<&'static str> = Vec::with_capacity(phrase.chars().count());
            for ch in phrase.chars() {
                let mut buf = [0u8; 4];
                let s: &str = ch.encode_utf8(&mut buf);
                let key = crate::font::static_key_for(s).unwrap_or("墨");
                chars.push(key);
            }
            if chars.is_empty() {
                chars.push("墨");
            }
            phrase_chars.push(chars);
        }
        let mut c = Self {
            phrase_idx: 0,
            char_idx: 0,
            cooldown: 0.0,
            phrase_chars,
        };
        c
    }

    /// True if we're between phrases — caller should not spawn a
    /// char and instead wait for `cooldown` to drain.
    pub fn is_breathing(&self) -> bool {
        self.cooldown > 0.0
    }

    /// Drain the breath timer. Called once per frame.
    pub fn tick_breath(&mut self, dt: f32) {
        if self.cooldown > 0.0 {
            self.cooldown = (self.cooldown - dt).max(0.0);
        }
    }

    /// Pop the next char from the current phrase. When the phrase
    /// is exhausted, sets the breath cooldown to ~2.4 s and returns
    /// None until it elapses, then advances to the next phrase on
    /// the next call.
    pub fn pop(&mut self) -> Option<&'static str> {
        if self.is_breathing() {
            return None;
        }
        let chars = self.phrase_chars.get(self.phrase_idx)?;
        if self.char_idx >= chars.len() {
            // Phrase exhausted — start a brief silence, advance on
            // the next non-empty call.
            self.cooldown = 2.4;
            return None;
        }
        let s = chars[self.char_idx];
        self.char_idx += 1;
        if self.char_idx >= chars.len() {
            // Last char of the phrase — start the breathing pause
            // before advancing so the viewer gets the silence
            // before the next phrase begins.
            self.cooldown = 2.4;
        }
        Some(s)
    }

    /// Called from outside once the cooldown elapses, so the next
    /// pop() advances to the next phrase.
    pub fn advance_after_silence(&mut self) {
        if self.cooldown <= 0.0 {
            self.phrase_idx = (self.phrase_idx + 1) % self.phrase_chars.len();
            self.char_idx = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_are_non_empty() {
        for p in renderable_phrases() {
            assert!(!p.is_empty(), "phrase corpus contains an empty line");
            assert!(p.chars().count() <= 12, "phrase too long: {p:?}");
        }
    }

    #[test]
    fn phrases_cover_verified_chars() {
        // The font ships a 141-char subset; every char in every
        // renderable phrase MUST be in that subset, otherwise the
        // cursor falls back to `墨` and the line reads as nonsense.
        let known = std::collections::BTreeSet::from_iter(
            "书云井体余信光冬冰凛凝前口叶吟听吸呼埃墨处夕夜奔孤寒尖尘岸崩川帛幽弦影径徙微心惊掌摆无旧昏星晚晨暖暮曦月朔木林根棉橘沉沟河泉流浪海涌深清渡温溅潭潮火灯炉炊烛烟焦焰照玻珀琥璃电疾睡石空纸绒翻背花苍苔英茧茶落蓝薄蜜蝉行裂诗质跳迁远迭钟铁银闪陨陶雨雪雷雾震霜露静页颤风飞鲸鸟鸣鸿麦黄鼓龙".chars(),
        );
        for p in renderable_phrases() {
            for c in p.chars() {
                assert!(
                    known.contains(&c),
                    "phrase {p:?} uses char {c:?} which is not in the font subset"
                );
            }
        }
    }

    #[test]
    fn cursor_walks_through_phrase_and_silences() {
        let mut c = PoetryCursor::new();
        // Walk through the first phrase; total emitted chars == number
        // of renderable slots in that phrase (≤ raw phrase length).
        let first_len = c.phrase_chars[0].len();
        let mut emitted = 0;
        while c.pop().is_some() {
            emitted += 1;
            assert!(emitted <= first_len, "cursor should not over-emit");
        }
        assert!(c.is_breathing(), "cursor should silence after phrase end");
    }

    #[test]
    fn cursor_advances_after_silence() {
        let mut c = PoetryCursor::new();
        let _ = c.pop(); // advance past first char
        let start_idx = c.phrase_idx;
        c.cooldown = 0.0;
        c.advance_after_silence();
        // Either we advanced to the next phrase, or we wrapped.
        let phrases = renderable_phrases();
        let expected = (start_idx + 1) % phrases.len();
        assert_eq!(c.phrase_idx, expected);
        assert_eq!(c.char_idx, 0);
    }
}
