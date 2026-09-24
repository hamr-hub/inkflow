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

# screenshot is produced in-app every 60s (state/screen.png) — no xwd needed
SHOT=state/screen.png

# tail telemetry for context
tail -n 12 state/telemetry.jsonl > state/tel_tail.txt 2>/dev/null || true

cat > state/claude_prompt.txt <<'EOF'
You are the unattended maintainer of "inkflow" (now a ZERO-DEP, std-only Rust binary).
Read these in order each turn: ZERO_DEP.md, PRODUCTION.md, then state/tel_tail.txt and
state/screen.png. Rendering is DRM/KMS dumb-buffer + software 32bpp; evdev touch via
raw ioctl; ollama over hand-written TCP; embedded CJK bitmap font. It must never go dark.

Do ONE bounded improvement/maintenance turn, autonomously.

PRIMARY MISSION NOW: make this PRODUCTION-GRADE. Read PRODUCTION.md and work down
its checklist, highest-impact unverified item first. Priority order:
  1. stability/memory (RSS limits, object pools vs per-frame alloc, no leaks)
  2. real frame-rate/frame-time correctness and CPU ceiling
  3. true fullscreen + resolution adapt + unattended boot-to-piece
  4. LLM resilience + real measured tok/s + graceful fallback/backoff
  5. touch hotplug, log rotation, disk caps
After each change, capture REAL EVIDENCE in your commit message and (if useful)
append to PRODUCTION.md checkboxes using measured numbers from telemetry/journal.

Default selection rules when reading state/tel_tail.txt and state/screen.png:
pick exactly ONE small, high-value step per turn. Keep the calm ambience; no menus,
no HUD, no debug text; Chinese-first; the local fallback generator must always stream.

Hard rules for every turn:
- Edit src/ and/or scripts/, then ALL must pass:
     cargo fmt
     cargo clippy --release -- -D warnings
     cargo build --release
- If build passes: systemctl --user restart inkflow.service
  then commit: git add -A && git commit -m "auto: <one-line change w/ measured evidence>"
  (pre-commit hook enforces fmt+build). Do NOT push.
- If anything fails: revert your edits (`git checkout -- .`), leave the running
  service untouched, and append what failed to state/autoloop.log.
Stay minimal. Do not edit systemd units, CI, PRODUCTION.md's mission, or this loop.
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
