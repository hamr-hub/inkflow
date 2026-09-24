// inkflow · telemetry.rs
//
// JSONL telemetry writer + downsampled PPM frame grab. Hand-rolled JSON so
// we don't pull serde back in; every field is a number or a fixed string
// so the encoder stays tiny.

use crate::sys;
use std::fs::OpenOptions;
use std::io::Write;

pub struct Telemetry<'a> {
    pub ts: u64,
    pub fps: f32,
    pub warmth: f32,
    pub energy: f32,
    pub contacts: usize,
    pub touch_device: &'a str,
    pub llm_ok: bool,
    pub llm_toks_per_s: f32,
    pub llm_model: &'a str,
    pub llm_last: &'a str,
    pub glyphs: usize,
    pub particles: usize,
    // Aesthetic state — what the piece is "saying" right now. Per
    // ARTIFACT.md 'telemetry.jsonl is the work's visible breath':
    // voice is the curatorial voice (婉约 / 豪放 / 禅寂 / 稚拙 /
    // 苍茫), ink_x is the current x-bias that glyphs cluster around
    // (留白 anchor), hue is the warm/cool palette position. Reading
    // these in telemetry lets the autoloop verify the art-direction
    // contract is being held — not just that fps is on target.
    pub voice: &'a str,
    pub ink_x: f32,
    pub hue: f32,
    // Per-window (10 s) frame cadence envelope in microseconds:
    // frame_min_us — fastest interval between consecutive frame starts
    //                 (smallest wall-clock gap the loop achieved)
    // frame_max_us — slowest interval between consecutive frame starts
    //                 (largest wall-clock gap; large = a frame missed
    //                 its budget and the loop had to skip-ahead).
    // frame_min_us = u32::MAX means "no frames in window" (u32 sentinels
    // never appear because the loop body resets to 0 every tick).
    pub frame_min_us: u32,
    pub frame_max_us: u32,
}

// Telemetry JSONL is appended every 10 s (≈ 8.6 K lines/day, ≈ 1.8 MB/day on
// this build's ~210-byte rows). On 24×7 production a single file would grow
// without bound, eventually filling whatever partition holds INKFLOW_STATE_DIR
// (usually /var/lib on the target). We bound the live file at 2 MiB and rotate
// to `<path>.1` (overwriting the previous `.1`) on every append that finds it
// over the cap. Two files × 2 MiB = 4 MiB total ceiling per state dir.
//
// Stat is one syscall per 10 s telemetry tick — negligible — and we only run
// it through the slow path (rotate) when actually over budget.
const TELEMETRY_MAX_BYTES: u64 = 2 * 1024 * 1024;

fn rotate_if_needed(path: &str) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= TELEMETRY_MAX_BYTES {
        return;
    }
    let backup = format!("{path}.1");
    // Move current to .1 (overwriting old). On any failure just leave the
    // oversize file in place — next tick will retry, telemetry never blocks
    // the render loop.
    let _ = std::fs::rename(path, &backup);
}

pub fn append(path: &str, t: &Telemetry<'_>) {
    rotate_if_needed(path);
    let line = encode(t);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", line);
    }
}

fn encode(t: &Telemetry<'_>) -> String {
    // Each push_kv_* helper appends a trailing comma when `last=false`,
    // so the field separator is owned by the helper — no manual commas
    // between calls. The earlier code did both, which produced `,,`
    // between every field and made the JSONL unparseable by any strict
    // JSON consumer (jq, Python json.loads, …).
    let mut s = String::with_capacity(512);
    s.push('{');
    push_kv_num(&mut s, "ts", t.ts as f64, false);
    push_kv_num(&mut s, "fps", t.fps as f64, false);
    push_kv_num(&mut s, "warmth", t.warmth as f64, false);
    push_kv_num(&mut s, "energy", t.energy as f64, false);
    push_kv_num(&mut s, "contacts", t.contacts as f64, false);
    push_kv_str(&mut s, "touch_device", t.touch_device, false);
    push_kv_bool(&mut s, "llm_ok", t.llm_ok, false);
    push_kv_num(&mut s, "llm_toks_per_s", t.llm_toks_per_s as f64, false);
    push_kv_str(&mut s, "llm_model", t.llm_model, false);
    push_kv_str(&mut s, "llm_last", t.llm_last, false);
    push_kv_num(&mut s, "glyphs", t.glyphs as f64, false);
    push_kv_num(&mut s, "particles", t.particles as f64, false);
    push_kv_str(&mut s, "voice", t.voice, false);
    push_kv_num(&mut s, "ink_x", t.ink_x as f64, false);
    push_kv_num(&mut s, "hue", t.hue as f64, false);
    // u32::MAX sentinel means "no frames completed in window" — emit as
    // null so consumers don't accidentally average an impossible value.
    // The null literal carries its own trailing comma so the next field
    // (frame_max_us) is separated consistently with the helper path.
    if t.frame_min_us == u32::MAX {
        s.push_str("\"frame_min_us\":null,");
    } else {
        push_kv_num(&mut s, "frame_min_us", t.frame_min_us as f64, false);
    }
    push_kv_num(&mut s, "frame_max_us", t.frame_max_us as f64, true);
    s.push('}');
    s
}

fn push_kv_num(out: &mut String, key: &str, val: f64, last: bool) {
    out.push('"');
    out.push_str(key);
    out.push('"');
    out.push(':');
    out.push_str(&fmt_f64(val));
    if !last {
        out.push(',');
    }
}

fn push_kv_bool(out: &mut String, key: &str, val: bool, last: bool) {
    out.push('"');
    out.push_str(key);
    out.push('"');
    out.push(':');
    out.push_str(if val { "true" } else { "false" });
    if !last {
        out.push(',');
    }
}

fn push_kv_str(out: &mut String, key: &str, val: &str, last: bool) {
    out.push('"');
    out.push_str(key);
    out.push('"');
    out.push(':');
    out.push('"');
    for c in val.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    if !last {
        out.push(',');
    }
}

fn fmt_f64(v: f64) -> String {
    if v.is_nan() {
        return "null".into();
    }
    if v.is_infinite() {
        return "null".into();
    }
    // 6 sig figs is plenty for our telemetry fields
    let s = format!("{:.6}", v);
    // trim trailing zeros after the dot but keep at least one digit
    let s = if let Some(dot) = s.find('.') {
        let mut end = s.len();
        while end > dot + 2 && s.as_bytes()[end - 1] == b'0' {
            end -= 1;
        }
        s[..end].to_string()
    } else {
        s
    };
    s
}

// ---------- PPM frame grab ----------

/// Write a down-sampled PPM (P6) of the current framebuffer. Caller is the
/// main thread (we read the BGRA buffer here in a spawned task). The
/// rasterizer is single-threaded so we copy the down-sampled bytes into
/// a fresh Vec and hand that to a worker to write to disk.
/// Self-monitoring: read the most recent entries from a
/// telemetry.jsonl file and report whether the curatorial voice
/// distribution is healthy. The autoloop Claude maintainer reads
/// this signal before deciding a change — if voice is stuck on one
/// for too long, the change this tick should push toward variety.
///
/// Returns:
///   - Ok(VoiceDriftReport) with a per-voice histogram and the
///     most-recent dominant voice, on success.
///   - Err(String) if the file is missing or empty.
///
/// The function does NOT enforce a hard limit — it just reports.
/// The art-direction judgement lives in the autoloop Claude, not
/// in the telemetry module. Currently #[cfg(test)] — no
/// production caller yet. The autoloop Claude learns the drift
/// state by inspecting the result of cargo test --release
/// ::voice_drift_check. When we add a `--voice-drift` CLI
/// subcommand, this gate drops.
#[cfg(test)]
pub fn voice_drift_check(path: &str) -> Result<VoiceDriftReport, String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Err(format!("telemetry file not readable: {path}"));
    };
    let mut counts: [u32; 5] = [0; 5]; // 婉约, 豪放, 禅寂, 稚拙, 苍茫
    let mut total = 0u32;
    let mut last_voice: Option<&'static str> = None;
    // Walk lines backwards (most recent first) until we've seen
    // `recent_window_count` entries OR the file is exhausted.
    let recent_window_count = 60usize; // 60 lines × 10 s/tick = 10 min
    let mut lines_seen = 0usize;
    for line in content.lines().rev() {
        if lines_seen >= recent_window_count {
            break;
        }
        lines_seen += 1;
        // Parse the voice field. The line is JSON; a tiny scanner
        // would do — for now extract "voice":"X" with a fixed string
        // search. (We don't need to be perfect; we just need to
        // count which voices have been in play recently.)
        if let Some(idx) = line.find("\"voice\":\"") {
            let start = idx + "\"voice\":\"".len();
            // End of the value is the next unescaped quote. For our
            // 5 voice names (all simple Chinese chars), a closing
            // quote is enough.
            if let Some(end) = line[start..].find('"') {
                let voice = &line[start..start + end];
                match voice {
                    "婉约" => counts[0] += 1,
                    "豪放" => counts[1] += 1,
                    "禅寂" => counts[2] += 1,
                    "稚拙" => counts[3] += 1,
                    "苍茫" => counts[4] += 1,
                    _ => {}
                }
                total += 1;
                if last_voice.is_none() {
                    last_voice = Some(match voice {
                        "婉约" => "婉约",
                        "豪放" => "豪放",
                        "禅寂" => "禅寂",
                        "稚拙" => "稚拙",
                        "苍茫" => "苍茫",
                        _ => "",
                    });
                }
            }
        }
    }
    let names = ["婉约", "豪放", "禅寂", "稚拙", "苍茫"];
    let per_voice: Vec<(&'static str, u32, u32)> = names
        .iter()
        .copied()
        .zip(counts.iter().copied())
        .map(|(n, c)| {
            let pct = if total > 0 { (c * 100) / total } else { 0 };
            (n, c, pct)
        })
        .collect();
    Ok(VoiceDriftReport {
        total,
        last_voice: last_voice.unwrap_or(""),
        per_voice,
    })
}

/// Voice-distribution summary produced by [`voice_drift_check`].
/// `per_voice` is (name, count, percent) tuples ordered to match the
/// canonical voice list (婉约 / 豪放 / 禅寂 / 稚拙 / 苍茫).
#[cfg(test)]
pub struct VoiceDriftReport {
    pub total: u32,
    pub last_voice: &'static str,
    /// (voice_name, count_in_window, percent_in_window)
    pub per_voice: Vec<(&'static str, u32, u32)>,
}

#[cfg(test)]
impl VoiceDriftReport {
    /// Returns true if one voice accounts for ≥ 70% of the recent
    /// window — a strong signal the piece is stuck on one voice
    /// and the autoloop Claude should push toward variety this tick.
    pub fn is_stuck(&self) -> bool {
        if self.total == 0 {
            return false;
        }
        self.per_voice.iter().any(|(_, _, pct)| *pct >= 70)
    }
}

#[allow(dead_code)]
pub fn grab_ppm_async(bgra: &[u32], w: u32, h: u32, pitch_px: usize, step: u32, path: String) {
    let nw = (w / step) as usize;
    let nh = (h / step) as usize;
    let mut buf: Vec<u8> = Vec::with_capacity(nw * nh * 3);
    for y in 0..nh {
        let sy = (y as u32 * step) as usize;
        for x in 0..nw {
            let sx = (x as u32 * step) as usize;
            let px = bgra[sy * pitch_px + sx];
            // BGRA in little-endian: byte0=B, byte1=G, byte2=R
            buf.push((px & 0xFF) as u8);
            buf.push(((px >> 8) & 0xFF) as u8);
            buf.push(((px >> 16) & 0xFF) as u8);
        }
    }
    std::thread::spawn(move || {
        if let Ok(mut f) = std::fs::File::create(&path) {
            let _ = writeln!(f, "P6\n{nw} {nh}\n255");
            let _ = f.write_all(&buf);
        }
    });
}

// ---------- soft rate limiter via nanosleep, in case we ever need it -----

#[allow(dead_code)]
pub fn nanosleep_ms(ms: u64) {
    sys::sleep_ms(ms);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_produces_valid_json() {
        let t = Telemetry {
            ts: 1_700_000_000,
            fps: 60.0,
            warmth: 0.5,
            energy: 0.3,
            contacts: 2,
            touch_device: "abc",
            llm_ok: true,
            llm_toks_per_s: 12.34,
            llm_model: "gemma3:1b",
            llm_last: "墨",
            glyphs: 100,
            particles: 50,
            voice: "禅寂",
            ink_x: 0.5,
            hue: 0.55,
            frame_min_us: 16_000,
            frame_max_us: 17_500,
        };
        let s = encode(&t);
        // The encoder must NOT leave double-commas or trailing commas
        // — verify by trying to round-trip through a basic
        // brace/quote-balanced parser.
        assert!(s.starts_with('{'));
        assert!(s.ends_with('}'));
        let mut depth: i32 = 0;
        let mut in_str = false;
        let mut prev = '\0';
        for c in s.chars() {
            if c == '"' && prev != '\\' {
                in_str = !in_str;
            }
            if !in_str {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    ',' if prev == ',' => panic!("double-comma at {:?}", s),
                    _ => {}
                }
            }
            assert!(depth >= 0, "unbalanced }} at: {s}");
            prev = c;
        }
        assert_eq!(depth, 0, "unclosed braces in: {s}");
        // No unescaped quotes inside string fields (we don't allow
        // quote characters in field values).
        assert!(s.contains("\"fps\":60"));
        assert!(s.contains("\"llm_ok\":true"));
        assert!(s.contains("\"frame_max_us\":17500"));
    }

    #[test]
    fn encode_handles_frame_min_max_sentinel() {
        let t = Telemetry {
            ts: 0,
            fps: 0.0,
            warmth: 0.0,
            energy: 0.0,
            contacts: 0,
            touch_device: "",
            llm_ok: false,
            llm_toks_per_s: 0.0,
            llm_model: "",
            llm_last: "",
            glyphs: 0,
            particles: 0,
            voice: "稚拙",
            ink_x: 0.5,
            hue: 0.5,
            frame_min_us: u32::MAX, // sentinel
            frame_max_us: 0,
        };
        let s = encode(&t);
        // sentinel must be emitted as null
        assert!(s.contains("\"frame_min_us\":null"));
        // And not as a literal u32::MAX
        assert!(!s.contains("4294967295"));
    }

    #[test]
    fn push_kv_str_escapes_specials() {
        let mut s = String::new();
        push_kv_str(&mut s, "k", "墨\n夜\"潮", false);
        // Should not contain raw newline
        assert!(!s.contains('\n'));
        // Should escape the quote
        assert!(s.contains("\\\""));
        // The key + value should round-trip through a simple scan.
        assert_eq!(s, "\"k\":\"墨\\n夜\\\"潮\",");
    }

    #[test]
    fn push_kv_str_emits_no_trailing_comma_when_last() {
        let mut s = String::new();
        push_kv_str(&mut s, "k", "x", true);
        assert_eq!(s, "\"k\":\"x\"");
    }

    #[test]
    fn push_kv_str_emits_trailing_comma_when_not_last() {
        let mut s = String::new();
        push_kv_str(&mut s, "k", "x", false);
        assert_eq!(s, "\"k\":\"x\",");
    }

    #[test]
    fn fmt_f64_handles_specials() {
        // NaN and Infinity must serialize as null so the JSONL parses.
        assert_eq!(fmt_f64(f64::NAN), "null");
        assert_eq!(fmt_f64(f64::INFINITY), "null");
        assert_eq!(fmt_f64(f64::NEG_INFINITY), "null");
        // Whole numbers keep a single zero after the dot — keeps the
        // field uniformly numeric for downstream readers (some
        // parsers reject "0" without a decimal part).
        assert_eq!(fmt_f64(0.0), "0.0");
        assert_eq!(fmt_f64(1.0), "1.0");
        // 6-significant-figure rounding
        assert_eq!(fmt_f64(0.1 + 0.2), "0.3");
        assert_eq!(fmt_f64(1.5), "1.5");
        assert_eq!(fmt_f64(0.5), "0.5");
    }

    #[test]
    fn fmt_f64_trims_trailing_zeros() {
        // 0.5000001 rounds to "0.500000", trailing zeros stripped → "0.5"
        assert_eq!(fmt_f64(0.5000001), "0.5");
        // 12.345678 rounds to "12.345678", no trailing zeros.
        assert_eq!(fmt_f64(12.345678), "12.345678");
    }

    #[test]
    fn append_writes_one_line_per_call() {
        let dir = std::env::temp_dir().join(format!("inkflow-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tel.jsonl");
        let path_str = path.to_string_lossy().to_string();

        // write 3 lines
        for i in 0..3 {
            let t = Telemetry {
                ts: i,
                fps: i as f32,
                warmth: 0.0,
                energy: 0.0,
                contacts: 0,
                touch_device: "",
                llm_ok: false,
                llm_toks_per_s: 0.0,
                llm_model: "",
                llm_last: "",
                glyphs: 0,
                particles: 0,
                voice: "苍茫",
                ink_x: 0.5,
                hue: 0.5,
                frame_min_us: 0,
                frame_max_us: 0,
            };
            append(&path_str, &t);
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "expected 3 lines, got {:?}", lines);
        for (i, l) in lines.iter().enumerate() {
            assert!(l.contains(&format!("\"ts\":{}", i)), "line {i}: {l}");
        }

        // cleanup
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod drift_tests {
    use super::*;

    fn write_fake_telemetry(path: &str, voices: &[&str]) {
        let mut s = String::new();
        for v in voices {
            // Minimal valid JSONL entry — must round-trip through
            // a json parser for the autoloop to trust the count.
            // We use placeholder values for everything except voice.
            s.push_str(&format!(
                "{{\"voice\":\"{v}\",\"ts\":1,\"warmth\":0.5,\"energy\":0.1}}\n"
            ));
        }
        std::fs::write(path, s).expect("write fake telemetry");
    }

    #[test]
    fn voice_drift_check_missing_file_returns_err() {
        let result = voice_drift_check("/tmp/inkflow_no_such_file.jsonl");
        assert!(result.is_err(), "missing file must surface as Err");
    }

    #[test]
    fn voice_drift_check_empty_file_reports_zero() {
        let path = "/tmp/inkflow_empty_telemetry.jsonl";
        std::fs::write(path, "").expect("write empty");
        let r = voice_drift_check(path).unwrap();
        assert_eq!(r.total, 0);
        assert_eq!(r.last_voice, "");
        assert!(!r.is_stuck());
    }

    #[test]
    fn voice_drift_check_balanced_distribution_is_not_stuck() {
        // 12 entries spread evenly across 3 voices: 4 / 4 / 4.
        // No voice has ≥ 70 %; not stuck.
        let path = "/tmp/inkflow_balanced_telemetry.jsonl";
        let voices = [
            "婉约", "婉约", "婉约", "婉约", "豪放", "豪放", "豪放", "豪放", "禅寂", "禅寂", "禅寂",
            "禅寂",
        ];
        write_fake_telemetry(path, &voices);
        let r = voice_drift_check(path).unwrap();
        assert_eq!(r.total, 12);
        assert!(!r.is_stuck(), "balanced should not be stuck");
    }

    #[test]
    fn voice_drift_check_dominated_distribution_is_stuck() {
        // 10 禅寂 + 3 婉约. 禅寂 dominates (10/13 = 77 %).
        // Most recent entry is 婉约, so last_voice == 婉约.
        let path = "/tmp/inkflow_dominated_telemetry.jsonl";
        let mut voices: Vec<&str> = (0..10).map(|_| "禅寂").collect();
        voices.extend(&["婉约", "婉约", "婉约"]);
        write_fake_telemetry(path, &voices);
        let r = voice_drift_check(path).unwrap();
        assert_eq!(r.total, 13);
        assert!(r.is_stuck(), "禅寂 10/13 = 77%% should be stuck");
        assert_eq!(r.last_voice, "婉约");
        // Confirm the per-voice breakdown.
        let 禅寂_count = r
            .per_voice
            .iter()
            .find(|(n, _, _)| *n == "禅寂")
            .map(|(_, c, _)| *c)
            .unwrap_or(0);
        assert_eq!(禅寂_count, 10);
    }

    #[test]
    fn voice_drift_check_only_recent_window_counts() {
        // Older entries outside the recent window shouldn't count.
        // The function reads the last `recent_window_count` (60)
        // lines, so we write 120 of the same voice. With a window
        // of 60, only 60 lines are counted — the report says the
        // piece is stuck on that voice, regardless of the earlier
        // 60 also being that voice (the report just sees 60/60
        // in the window).
        let path = "/tmp/inkflow_long_stuck_telemetry.jsonl";
        let voices: Vec<&str> = (0..120).map(|_| "豪放").collect();
        write_fake_telemetry(path, &voices);
        let r = voice_drift_check(path).unwrap();
        assert_eq!(r.total, 60, "only last 60 entries count");
        assert!(r.is_stuck());
    }
}
