#!/usr/bin/env bash
# inkflow autonomous iteration: one bounded Claude turn every timer tick.
set -uo pipefail
cd "$HOME/codespace/inkflow" || exit 0
export PATH="$HOME/.nvm/versions/node/v22.22.1/bin:$PATH"
export CARGO_TARGET_DIR=/mnt/ssd/codespace/.cargo-target/inkflow
LOG=state/autoloop.log
mkdir -p state
echo "===== $(date -Is) tick =====" >> "$LOG"

# skip on memory pressure — never push the Jetson into OOM
AVAIL_KB=$(awk '/MemAvailable/{print $2}' /proc/meminfo)
if [ "${AVAIL_KB:-0}" -lt 700000 ]; then
  echo "skip: MemAvailable=${AVAIL_KB}KB" >> "$LOG"
  exit 0
fi

# flock: never overlap ourselves
exec 9>state/autoloop.lock
if ! flock -n 9; then echo "skip: locked" >> "$LOG"; exit 0; fi

# ensure the piece itself is running
systemctl --user is-active --quiet inkflow.service || systemctl --user start inkflow.service

# screenshot for visual feedback (root window, X:0)
SHOT=state/screen.png
if command -v convert >/dev/null; then
  xwd -display :0 -root -silent | convert xwd:- -resize 960x "$SHOT" 2>>"$LOG"
fi

# tail telemetry for context
tail -n 12 state/telemetry.jsonl > state/tel_tail.txt 2>/dev/null || true

cat > state/claude_prompt.txt <<'EOF'
You are the unattended maintainer of "inkflow" (Rust + macroquad + evdev + ollama),
a self-iterating generative art object: fullscreen ambient Chinese text stream +
particles on a Jetson Orin Nano, shaped by touch input. It must NEVER go dark.

Do ONE bounded improvement/maintenance turn, autonomously:

1. Read state/tel_tail.txt (fps, warmth/energy, touch device, llm_ok, tok/s,
   glyph/particle counts) and state/screen.png (current screen).
2. Pick exactly ONE small, high-value change, e.g.:
   - visual/typography polish (layout, fade, hue, motion, glyph sizing)
   - touch→mood mapping (position/energy/contacts → prompt, colour, speed)
   - LLM prompt engineering; resilience when ollama is slow/down
   - performance/memory (Jetson 8GB, keep app RSS under ~1GB, avoid OOM)
   - fix anything visibly broken in the screenshot
   Constraints: keep the calm ambience aesthetic; no menus, no HUD, no debug text;
   output is Chinese-first; local fallback generator must always keep streaming.
3. Edit src/ and/or scripts/. Then run, ALL must pass:
     cargo fmt
     cargo clippy --release -- -D warnings
     cargo build --release
4. If build passes: systemctl --user restart inkflow.service
   then commit: git add -A && git commit -m "auto: <one-line change>"
   (pre-commit hook enforces fmt+build). Do NOT push.
5. If anything fails: revert your edits (`git checkout -- .`), leave the running
   service untouched, and append what failed to state/autoloop.log.
Stay minimal. Do not edit systemd units, CI, or this governance loop.
EOF

timeout 780 claude --dangerously-skip-permissions --print \
  < state/claude_prompt.txt >> "$LOG" 2>&1
RC=$?
echo "claude_rc=$RC $(date -Is)" >> "$LOG"

# rotate logs (keep last 200 lines)
for f in "$LOG" state/telemetry.jsonl; do
  [ -f "$f" ] && tail -n 200 "$f" > "$f.tmp" && mv "$f.tmp" "$f"
done
exit 0
