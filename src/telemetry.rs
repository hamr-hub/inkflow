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
