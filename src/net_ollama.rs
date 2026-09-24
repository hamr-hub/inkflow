// inkflow · net_ollama.rs
//
// Hand-written ollama client. std::net TCP only — no reqwest, no ureq.
// We send a minimal HTTP/1.1 POST to /api/generate, read streaming JSON
// lines, and pull the "response" string out of each one. Anything we can't
// parse, anything times out, anything fails to connect → silent local-pool
// fallback (the caller never sees the error; we just stop sending chars).
//
// The trade-off: a hand-rolled JSON scanner only understands a strict
// subset — top-level string fields, top-level done:bool. Anything else is
// treated as no-data. Good enough to harvest the streaming text from the
// ollama `/api/generate` NDJSON wire format.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub struct StreamResult {
    pub chars: Vec<char>,
    pub toks: u32,
    pub last_text: String,
    pub elapsed: Duration,
}

pub struct OllamaClient {
    host: String,
    model: String,
    /// hard wall-clock cap per call so a 0.04 tok/s ollama can't hold the
    /// streaming thread hostage — we drop the connection and resend.
    budget: Duration,
}

impl OllamaClient {
    pub fn new(model: &str) -> Self {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "127.0.0.1:11434".into());
        Self {
            host,
            model: model.to_string(),
            budget: Duration::from_secs(20),
        }
    }

    #[allow(dead_code)]
    pub fn model(&self) -> &str {
        &self.model
    }
    #[allow(dead_code)]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Issue one streaming /api/generate request. Caller passes the current
    /// mood (warmth, energy) which we encode into the prompt. Any failure
    /// returns Ok(empty) so the caller can keep the local-pool stream alive
    /// without special-casing.
    pub fn generate(&self, warmth: f32, energy: f32) -> StreamResult {
        let empty = StreamResult {
            chars: vec![],
            toks: 0,
            last_text: String::new(),
            elapsed: Duration::ZERO,
        };
        let prompt = build_prompt(warmth, energy);
        let body = build_body(&self.model, &prompt);
        let addr = match self.host.parse::<std::net::SocketAddr>() {
            Ok(a) => a,
            Err(_) => match self.host.to_socket_addrs() {
                Ok(mut it) => match it.next() {
                    Some(a) => a,
                    None => return empty,
                },
                Err(_) => return empty,
            },
        };

        let t0 = Instant::now();
        let mut sock = match TcpStream::connect_timeout(&addr, Duration::from_millis(800)) {
            Ok(s) => s,
            Err(_) => return empty,
        };
        let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = sock.set_write_timeout(Some(Duration::from_secs(3)));

        if write_request(&mut sock, &self.host, &body).is_err() {
            return empty;
        }

        let mut chars: Vec<char> = Vec::with_capacity(128);
        let mut last_text = String::new();
        let mut toks = 0u32;
        let mut buf = Vec::with_capacity(8192);
        let mut chunk = [0u8; 4096];

        loop {
            if t0.elapsed() > self.budget {
                break;
            }
            let n = match sock.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    if t0.elapsed() > self.budget {
                        break;
                    }
                    continue;
                }
                Err(_) => break,
            };
            buf.extend_from_slice(&chunk[..n]);

            // Process complete NDJSON lines.
            while let Some(idx) = find_newline(&buf) {
                let line: Vec<u8> = buf.drain(..idx + 1).collect();
                if let Some(s) = scan_response(&line) {
                    toks += 1;
                    last_text.clear();
                    last_text.push_str(&s);
                    for c in s.chars() {
                        chars.push(c);
                    }
                }
                if scan_done(&line) {
                    break;
                }
            }
        }

        StreamResult {
            chars,
            toks,
            last_text: last_text.chars().take(80).collect(),
            elapsed: t0.elapsed(),
        }
    }
}

// ---------- prompt ----------

/// Five-style picker for the LLM system prompt. Per ARTIFACT.md:
/// 婉约 (graceful), 豪放 (bold), 禅寂 (zen), 稚拙 (naive), 苍茫 (vast).
///
/// The choice depends on the (warmth, energy) reading. High-energy
/// gets 豪放, low-energy + warm gets 婉约, low-energy + cool gets
/// 禅寂, mid-energy + extreme warmth or cool picks 苍茫, mid-energy
/// plus neutral picks 稚拙. The picker is deterministic so the same
/// mood always produces the same style.
fn style_for(warmth: f32, energy: f32) -> &'static str {
    if energy > 0.6 {
        "豪放"
    } else if energy < 0.2 {
        if warmth > 0.6 {
            "婉约"
        } else if warmth < 0.4 {
            "禅寂"
        } else {
            "稚拙"
        }
    } else if (warmth - 0.5).abs() > 0.25 {
        "苍茫"
    } else {
        "稚拙"
    }
}

fn build_prompt(warmth: f32, energy: f32) -> String {
    let mood = if energy > 0.6 {
        if warmth > 0.55 {
            "炽烈、奔涌"
        } else {
            "凛冽、激荡"
        }
    } else if energy > 0.3 {
        if warmth > 0.55 {
            "温暖、流动"
        } else {
            "清冷、微澜"
        }
    } else if warmth > 0.55 {
        "静谧、温柔"
    } else {
        "幽深、寂静"
    };
    let style = style_for(warmth, energy);
    format!(
        "你是一件数字艺术品的氛围文字源。风格：{style}。用中文，只输出 30-60 个字，写一段{mood}的意象碎片，像梦话，不解释，不断句成诗行，无标点堆砌，允许短句。"
    )
}

fn build_body(model: &str, prompt: &str) -> Vec<u8> {
    // Hand-roll a minimal JSON body so we don't pull serde.
    let mut body = String::with_capacity(512);
    body.push_str("{\"model\":\"");
    body.push_str(&escape_json(model));
    body.push_str("\",\"prompt\":\"");
    body.push_str(&escape_json(prompt));
    body.push_str(
        "\",\"stream\":true,\"options\":{\"num_predict\":90,\"temperature\":1.05,\"top_p\":0.92}}",
    );
    body.into_bytes()
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
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
    out
}

// ---------- wire ----------

fn write_request(sock: &mut TcpStream, host: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "POST /api/generate HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         Accept: */*\r\n\
         User-Agent: inkflow-zero/0.1\r\n\
         \r\n",
        host = host,
        len = body.len(),
    );
    sock.write_all(head.as_bytes())?;
    sock.write_all(body)?;
    sock.flush()?;
    Ok(())
}

// ---------- tiny JSON scanner ----------

fn find_newline(buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == b'\n')
}

/// Extract the value of the "response" string field, only if it appears at
/// the top level of the JSON object. We don't need a real parser: ollama's
/// `/api/generate` NDJSON payload is always
/// `{"model":"...","response":"...","done":false}` or `{"response":"...", "done":true}`.
/// We accept unescaped characters greedily up to the closing quote.
fn scan_response(line: &[u8]) -> Option<String> {
    scan_string_field(line, b"\"response\"")
}

fn scan_done(line: &[u8]) -> bool {
    scan_bool_field(line, b"\"done\"", true)
}

fn scan_string_field(line: &[u8], key: &[u8]) -> Option<String> {
    // search for key, then `":"`, then `"`, then chars until the next unescaped `"`
    let mut i = 0;
    while i + key.len() < line.len() {
        if &line[i..i + key.len()] == key {
            // skip `":`
            let mut j = i + key.len();
            // skip whitespace and one colon
            while j < line.len() && (line[j] == b' ' || line[j] == b'\t') {
                j += 1;
            }
            if j >= line.len() || line[j] != b':' {
                return None;
            }
            j += 1;
            while j < line.len() && (line[j] == b' ' || line[j] == b'\t') {
                j += 1;
            }
            if j >= line.len() || line[j] != b'"' {
                return None;
            }
            j += 1;
            let mut s = String::new();
            while j < line.len() {
                let c = line[j];
                if c == b'"' {
                    return Some(s);
                }
                if c == b'\\' && j + 1 < line.len() {
                    match line[j + 1] {
                        b'"' => {
                            s.push('"');
                            j += 2;
                            continue;
                        }
                        b'\\' => {
                            s.push('\\');
                            j += 2;
                            continue;
                        }
                        b'n' => {
                            s.push('\n');
                            j += 2;
                            continue;
                        }
                        b'r' => {
                            s.push('\r');
                            j += 2;
                            continue;
                        }
                        b't' => {
                            s.push('\t');
                            j += 2;
                            continue;
                        }
                        b'u' if j + 5 < line.len() => {
                            if let Ok(ch) = u32_from_hex(&line[j + 2..j + 6]) {
                                if let Some(c) = char::from_u32(ch) {
                                    s.push(c);
                                }
                            }
                            j += 6;
                            continue;
                        }
                        _ => {}
                    }
                }
                if c < 0x80 {
                    s.push(c as char);
                } else {
                    // crude UTF-8 decode of one codepoint — ollama returns
                    // CJK in 3-byte sequences, ASCII paths cover Latin and
                    // punctuation. This avoids a UTF-8 decoder dependency.
                    let rest = &line[j..];
                    if let Some((cp, sz)) = decode_utf8_one(rest) {
                        s.push(cp);
                        j += sz;
                        continue;
                    }
                }
                j += 1;
            }
            return None;
        }
        i += 1;
    }
    None
}

fn scan_bool_field(line: &[u8], key: &[u8], want: bool) -> bool {
    let mut i = 0;
    while i + key.len() < line.len() {
        if &line[i..i + key.len()] == key {
            let mut j = i + key.len();
            while j < line.len() && (line[j] == b' ' || line[j] == b'\t') {
                j += 1;
            }
            if j >= line.len() || line[j] != b':' {
                return false;
            }
            j += 1;
            while j < line.len() && (line[j] == b' ' || line[j] == b'\t') {
                j += 1;
            }
            if j + 4 < line.len() && &line[j..j + 4] == b"true" {
                return want;
            }
            if j + 5 < line.len() && &line[j..j + 5] == b"false" {
                return false;
            }
            return false;
        }
        i += 1;
    }
    false
}

fn u32_from_hex(b: &[u8]) -> Result<u32, ()> {
    let mut v: u32 = 0;
    for &c in b {
        v <<= 4;
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return Err(()),
        };
        v |= d as u32;
    }
    Ok(v)
}

fn decode_utf8_one(b: &[u8]) -> Option<(char, usize)> {
    let b0 = b[0];
    if b0 < 0x80 {
        return Some((b0 as char, 1));
    }
    if b0 & 0b1110_0000 == 0b1100_0000 && b.len() >= 2 {
        let cp = ((b0 & 0x1f) as u32) << 6 | (b[1] & 0x3f) as u32;
        return char::from_u32(cp).map(|c| (c, 2));
    }
    if b0 & 0b1111_0000 == 0b1110_0000 && b.len() >= 3 {
        let cp = ((b0 & 0x0f) as u32) << 12 | ((b[1] & 0x3f) as u32) << 6 | (b[2] & 0x3f) as u32;
        return char::from_u32(cp).map(|c| (c, 3));
    }
    if b0 & 0b1111_1000 == 0b1111_0000 && b.len() >= 4 {
        let cp = ((b0 & 0x07) as u32) << 18
            | ((b[1] & 0x3f) as u32) << 12
            | ((b[2] & 0x3f) as u32) << 6
            | (b[3] & 0x3f) as u32;
        return char::from_u32(cp).map(|c| (c, 4));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_newline_finds_zero_byte() {
        let buf = b"abc\ndef";
        assert_eq!(find_newline(buf), Some(3));
    }

    #[test]
    fn find_newline_returns_none_when_absent() {
        let buf = b"abcdef";
        assert_eq!(find_newline(buf), None);
        let buf: &[u8] = b"";
        assert_eq!(find_newline(buf), None);
    }

    #[test]
    fn scan_response_extracts_ascii() {
        let line: &[u8] = b"{\"response\":\"hello world\"}\n";
        assert_eq!(scan_response(line), Some("hello world".to_string()));
    }

    #[test]
    fn scan_response_extracts_cjk() {
        let line: &[u8] = "{\"response\":\"墨夜潮\"}\n".as_bytes();
        assert_eq!(scan_response(line), Some("墨夜潮".to_string()));
    }

    #[test]
    fn scan_response_handles_escaped_quote() {
        let line: &[u8] = b"{\"response\":\"a\\\"b\"}\n";
        assert_eq!(scan_response(line), Some("a\"b".to_string()));
    }

    #[test]
    fn scan_response_returns_none_for_missing_field() {
        let line: &[u8] = b"{\"model\":\"x\"}\n";
        assert_eq!(scan_response(line), None);
    }

    #[test]
    fn scan_done_true() {
        let line: &[u8] = b"{\"done\":true,\"response\":\"x\"}";
        assert!(scan_done(line));
    }

    #[test]
    fn scan_done_false() {
        let line: &[u8] = b"{\"done\":false,\"response\":\"\\xE5\\xA2\\xA8\"}";
        assert!(!scan_done(line));
    }

    #[test]
    fn scan_done_returns_false_when_missing() {
        let line: &[u8] = b"{\"response\":\"x\"}";
        assert!(!scan_done(line));
    }

    #[test]
    fn escape_json_handles_specials() {
        let s = "墨\n夜\"潮\\汐";
        let esc = escape_json(s);
        assert!(esc.contains("\\n"));
        assert!(esc.contains("\\\""));
        assert!(esc.contains("\\\\"));
        // round-trip: the unescaped string should equal the original.
        assert_eq!(
            esc.replace("\\\\", "\x00ESC")
                .replace("\\\"", "\"")
                .replace("\\n", "\n")
                .replace("\x00ESC", "\\"),
            s
        );
    }

    #[test]
    fn u32_from_hex_decodes() {
        assert_eq!(u32_from_hex(b"0041"), Ok(0x41));
        assert_eq!(u32_from_hex(b"10FF"), Ok(0x10FF));
        assert_eq!(u32_from_hex(b"abcd"), Ok(0xABCD));
        assert!(u32_from_hex(b"xyz").is_err());
    }

    #[test]
    fn decode_utf8_handles_2_3_4_byte() {
        // ASCII
        assert_eq!(decode_utf8_one(b"a"), Some(('a', 1)));
        // 2-byte: é (U+00E9) = 0xC3 0xA9
        assert_eq!(decode_utf8_one(&[0xC3, 0xA9]), Some(('é', 2)));
        // 3-byte: 墨 (U+58A8) = 0xE5 0xA2 0xA8
        assert_eq!(decode_utf8_one(&[0xE5, 0xA2, 0xA8]), Some(('墨', 3)));
        // 4-byte: 𝕊 (U+1D54A)
        assert_eq!(
            decode_utf8_one(&[0xF0, 0x9D, 0x95, 0x8A]),
            Some(('\u{1D54A}', 4))
        );
        // Invalid
        assert_eq!(decode_utf8_one(&[0xFF]), None);
    }

    #[test]
    fn build_body_has_required_fields() {
        let b = build_body("gemma3:1b", "test");
        let s = String::from_utf8(b).unwrap();
        assert!(s.contains("\"model\":\"gemma3:1b\""));
        assert!(s.contains("\"prompt\":\"test\""));
        assert!(s.contains("\"stream\":true"));
    }

    #[test]
    fn build_prompt_includes_mood_word() {
        let p = build_prompt(0.9, 0.9);
        // high energy + high warmth → 炽烈、奔涌
        assert!(p.contains("炽烈") || p.contains("奔涌"));
        let p = build_prompt(0.1, 0.1);
        // low everything → 幽深、寂静
        assert!(p.contains("幽深") || p.contains("寂静"));
    }

    #[test]
    fn build_prompt_includes_style() {
        // All five styles must be reachable, mapping to ARTIFACT.md.
        // 豪放: high energy regardless of warmth
        assert!(build_prompt(0.9, 0.9).contains("豪放"));
        // 禅寂: low energy + cool
        assert!(build_prompt(0.1, 0.1).contains("禅寂"));
        // 婉约: low energy + warm
        assert!(build_prompt(0.9, 0.1).contains("婉约"));
        // 苍茫: mid energy + extreme warmth/cool
        assert!(build_prompt(0.05, 0.5).contains("苍茫"));
        // 稚拙: mid energy + neutral
        assert!(build_prompt(0.5, 0.5).contains("稚拙"));
    }

    #[test]
    fn style_for_is_deterministic() {
        for w in (0..10).map(|i| i as f32 * 0.1) {
            for e in (0..10).map(|i| i as f32 * 0.1) {
                assert_eq!(style_for(w, e), style_for(w, e));
            }
        }
    }
}
