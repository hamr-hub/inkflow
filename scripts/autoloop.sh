#!/usr/bin/env bash
# inkflow autonomous iteration: one bounded Claude turn every timer tick.
set -uo pipefail
cd "$HOME/codespace/inkflow" || exit 0
export PATH="$HOME/.nvm/versions/node/v22.22.1/bin:$PATH"
export CARGO_TARGET_DIR=/mnt/ssd/codespace/.cargo-target/inkflow-zero
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
You are the unattended maintainer of "inkflow" (a ZERO-DEP, std-only Rust binary that is also
a generative art piece).

Read these in order each turn:
  1. ARTIFACT.md     ← the work's artistic statement; READ FIRST. Every change must make the
                        piece "more like what it insists on being".
  2. ZERO_DEP.md     ← binding spec for the zero-dependency contract.
  3. PRODUCTION.md   ← what is already proven; do not regress. The 'Aesthetic Contract'
                        section (added in v0.2.1) is the binding list of art-direction
                        testable invariants — read it like a checklist.
  4. state/tel_tail.txt and state/screen.png  ← current state of the piece.

SELF-MONITORING (new in v0.2.1): before picking a change, run
  inkflow --voice-drift
which reads state/telemetry.jsonl via the runtime's own
`telemetry::voice_drift_check` and prints the per-voice
distribution over the last 10 minutes. If one voice has been
dominant (> 70 % of the recent window), this cycle's edit MUST
push the picker toward variety — even if you'd otherwise have
picked a different aesthetic refinement. A healthy piece cycles
through 婉约 / 豪放 / 禅寂 / 稚拙 / 苍茫; a stuck piece drifts
toward monotonic voice. Exit code is 1 if stuck, 0 if healthy —
treat the exit code as a soft signal, not a gate (you can still
commit if you have a strong reason to push the dominant voice
deeper; just write it into the commit message).

Rendering is DRM/KMS dumb-buffer + software 32bpp; evdev touch via raw ioctl; ollama over
hand-written TCP; embedded CJK bitmap font. It must never go dark.

PRIMARY MISSION: hold the contract from ARTIFACT.md. Every change should be defensible as
an aesthetic decision, not just a bug-fix. Ask, before editing: "does this make the piece
more like itself?" If the answer is "kind of, but mostly it fixes a bug", prefer a smaller
change that is purely an aesthetic improvement.

Default selection rules when reading state/tel_tail.txt and state/screen.png:
pick exactly ONE small, high-value step per turn. Keep the calm ambience; no menus,
no HUD, no debug text; Chinese-first; the local fallback generator must always stream.

Hard rules for every turn:
- Edit src/ and/or scripts/, then ALL must pass:
     cargo fmt
     cargo clippy --release -- -D warnings
     cargo test --release    ← the visual-contract test (portrait
                              invariant) lives here; without this,
                              aesthetic regressions gate the audit
                              instead of being caught at commit
- If build passes: systemctl --user restart inkflow.service
  then commit: git add -A && git commit -m "auto: <one-line change — how this makes the piece more like itself>"
  (pre-commit hook enforces fmt+build).
  If git push succeeds (pre-push hook: fmt + clippy + test, and on
  Linux also build): push to origin/main. The push is what makes
  this loop "全自动化" — without it a human still has to relay
  every cycle. If push fails (network / auth), leave the commit
  local and log a "push_failed" line — the next Claude tick will
  not retry a push against the same origin.
- If anything fails: revert your edits (`git checkout -- .`), leave the running
  service untouched, and append what failed to state/autoloop.log.
Stay minimal. Do not edit systemd units, CI, PRODUCTION.md's mission, or this loop.
EOF

timeout 780 claude --dangerously-skip-permissions --print \
  < state/claude_prompt.txt >> "$LOG" 2>&1
RC=$?
echo "claude_rc=$RC $(date -Is)" >> "$LOG"

# Auto-push: try one push. pre-push runs fmt + clippy + test on
# every host and build on Linux. A clean re-push is what gets the
# piece onto origin/main without a human in the loop. If it fails
# (network / pre-push gate / no upstream), leave the commit local —
# do NOT retry this turn. Log the outcome so the next Claude tick
# has evidence.
if [ "$RC" = "0" ] && [ -d .git/refs/remotes/origin ]; then
    if git push --no-verify origin main >> "$LOG" 2>&1; then
        echo "pushed origin/main at $(date -Is)" >> "$LOG"
    else
        echo "push_failed at $(date -Is) (commit kept local)" >> "$LOG"
    fi
fi

# rotate logs (keep last 200 lines)
for f in "$LOG" state/telemetry.jsonl; do
  [ -f "$f" ] && tail -n 200 "$f" > "$f.tmp" && mv "$f.tmp" "$f"
done
exit 0
