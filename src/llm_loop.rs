// inkflow · llm_loop.rs
//
// The LLM worker thread. Owns one `OllamaClient` and pushes harvested
// characters into the shared `Shared` queue so the render thread can
// pop one per spawn without ever blocking on the network.
//
// Loop shape, per call to `client.generate(warmth, energy)`:
//   1. read mood (warmth, energy) under the mood mutex
//   2. send the streaming /api/generate request (ollama or local pool)
//   3. on success: mark `llm_ok=true`, blend tok/s with EMA, push
//      characters into `llm_chars` (cap 2048 with FIFO eviction)
//   4. on zero-token reply with non-empty last_text: sleep 3 s
//      (ollama is running but slow)
//   5. on empty reply: mark `llm_ok=false`, sleep 4 s (ollama is down)
//   6. unconditional 120 ms tail sleep so we never hammer the server
//
// The four constants (3s, 4s, 120ms) match the original `main.rs` body
// so this is a pure code-motion refactor.

use crate::mood::FrameMood;
use crate::net_ollama::OllamaClient;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Shared state between the LLM worker and the render thread.
///
/// Lives behind a Mutex because reads happen every frame; writes
/// happen at most a few times per second. The queue itself is a
/// `VecDeque<char>` with cap 2048 so memory is bounded.
pub struct Shared {
    /// `true` once the LLM has produced at least one successful
    /// streamed response since boot. Stays `true` even across slow
    /// responses (those just throttle the loop instead).
    pub llm_ok: bool,
    /// EMA of tokens-per-second from the most recent ollama reply.
    pub llm_tps: f32,
    /// Last streamed text fragment, truncated to 80 chars. Goes into
    /// telemetry so the autoloop can verify the model is generating
    /// what we asked for.
    pub llm_last: String,
    /// Ring buffer of characters waiting to be painted. The render
    /// thread pops one per glyph spawn.
    pub llm_chars: VecDeque<char>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            llm_ok: false,
            llm_tps: 0.0,
            llm_last: String::new(),
            llm_chars: VecDeque::with_capacity(2048),
        }
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

const LLM_CHAR_CAP: usize = 2048;
const LLM_OK_REPLY_SLEEP: Duration = Duration::from_secs(3);
const LLM_DOWN_SLEEP: Duration = Duration::from_secs(4);
const LLM_TAIL_SLEEP: Duration = Duration::from_millis(120);

/// Run the LLM worker forever. Spawned as its own OS thread from
/// `main`. Takes ownership of one `OllamaClient` (so reconnect logic
/// could be added here later) plus `Arc`s to the shared character
/// queue and the mood state.
pub fn run_worker(client: OllamaClient, shared: Arc<Mutex<Shared>>, mood: Arc<Mutex<(f32, f32)>>) {
    loop {
        let (warmth, energy) = {
            let g = mood.lock().unwrap();
            *g
        };
        let res = client.generate(warmth, energy);
        {
            let mut sh = shared.lock().unwrap();
            if res.toks > 0 {
                sh.llm_ok = true;
                let tps = res.toks as f32 / res.elapsed.as_secs_f32().max(0.001);
                // EMA so a single fast reply doesn't snap the displayed
                // tok/s to a misleading peak.
                sh.llm_tps = sh.llm_tps * 0.7 + tps * 0.3;
                sh.llm_last = res.last_text.clone();
                for c in res.chars {
                    sh.llm_chars.push_back(c);
                    if sh.llm_chars.len() > LLM_CHAR_CAP {
                        sh.llm_chars.pop_front();
                    }
                }
            } else if !res.last_text.is_empty() {
                // Slow but connected — back off so we don't busy-poll.
                std::thread::sleep(LLM_OK_REPLY_SLEEP);
            } else {
                // Disconnected / refused / timed out.
                sh.llm_ok = false;
                std::thread::sleep(LLM_DOWN_SLEEP);
            }
        }
        std::thread::sleep(LLM_TAIL_SLEEP);
    }
}

/// Convenience: pop one character off the shared queue under the
/// lock. Used by the render thread every glyph spawn.
pub fn pop_char(shared: &Arc<Mutex<Shared>>) -> Option<char> {
    shared.lock().unwrap().llm_chars.pop_front()
}

/// Convenience: read `llm_ok` under the lock. Used by the render thread
/// to choose the LLM-vs-fallback size bump for newly spawned glyphs.
pub fn llm_ok(shared: &Arc<Mutex<Shared>>) -> bool {
    shared.lock().unwrap().llm_ok
}

/// Build a snapshot of the shared state for telemetry. Acquires the
/// lock once and copies out the few primitives the JSONL writer needs.
pub fn snapshot(shared: &Arc<Mutex<Shared>>) -> (bool, f32, String) {
    let sh = shared.lock().unwrap();
    (sh.llm_ok, sh.llm_tps, sh.llm_last.clone())
}

/// Convenience: publish the current mood into the LLM worker's mood
/// state. Called once per frame so the next /api/generate prompt gets
/// the freshest (warmth, energy).
pub fn publish_mood(mood: &Arc<Mutex<(f32, f32)>>, frame: &FrameMood) {
    *mood.lock().unwrap() = (frame.warmth, frame.energy);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_char_cap_holds() {
        let s = Shared::new();
        assert_eq!(s.llm_chars.capacity(), LLM_CHAR_CAP);
    }

    #[test]
    fn snapshot_copies_primitives() {
        let sh = Arc::new(Mutex::new(Shared::new()));
        {
            let mut g = sh.lock().unwrap();
            g.llm_ok = true;
            g.llm_tps = 12.5;
            g.llm_last = "墨".into();
        }
        let (ok, tps, last) = snapshot(&sh);
        assert!(ok);
        assert!((tps - 12.5).abs() < 1e-6);
        assert_eq!(last, "墨");
    }

    #[test]
    fn pop_char_returns_none_when_empty() {
        let sh = Arc::new(Mutex::new(Shared::new()));
        assert!(pop_char(&sh).is_none());
    }

    #[test]
    fn pop_char_returns_in_fifo_order() {
        let sh = Arc::new(Mutex::new(Shared::new()));
        {
            let mut g = sh.lock().unwrap();
            g.llm_chars.push_back('墨');
            g.llm_chars.push_back('夜');
        }
        assert_eq!(pop_char(&sh), Some('墨'));
        assert_eq!(pop_char(&sh), Some('夜'));
        assert_eq!(pop_char(&sh), None);
    }
}
