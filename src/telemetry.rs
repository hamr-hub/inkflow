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
