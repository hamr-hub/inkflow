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
    format!(
        "你是一件数字艺术品的氛围文字源。用中文，只输出 30-60 个字，\
         写一段{mood}的意象碎片，像梦话，不解释，不断句成诗行，无标点堆砌，允许短句。"
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
            let _truth = want as usize;
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
