#!/usr/bin/env bash
# inkflow autonomous art iteration — one bounded agent turn per timer tick.
# Runs inside the GitHub-backed tree (personal/inkflow) so every green turn
# is committed AND pushed to origin/main without a human relaying it.
set -uo pipefail

REPO="$HOME/codespace/personal/inkflow"
cd "$REPO" || exit 0
export PATH="$HOME/.nvm/versions/node/v22.22.1/bin:$PATH"
export CARGO_TARGET_DIR=/mnt/ssd/codespace/.cargo-target/inkflow-merge
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

# make sure the local checkout tracks origin, then pull the latest so the
# agent never iterates on a stale base (the root service / other pushes
# may have advanced main).
git fetch origin main >> "$LOG" 2>&1 || true
git merge --ff-only origin/main >> "$LOG" 2>&1 || echo "note: not ff-only, agent resolves" >> "$LOG"

# fresh render evidence for the agent to look at (build first so the
# harness exists in the custom target dir)
cargo build --release >> "$LOG" 2>&1 || true
"$CARGO_TARGET_DIR/release/inkflow" --compose-test 12 >> "$LOG" 2>&1 || true

cat > state/claude_prompt.txt <<'EOF'
You are the unattended curator of "inkflow" — a zero-dependency Rust binary
that is also a generative art piece. Its current architecture is a small
library (src/lib.rs: color / glyph / phrase / rhythm / scene / surface / png)
plus a harness binary (src/main.rs) that renders compositions.

Read in order each turn:
  1. ART_DIRECTION.md   ← binding visual intent; READ FIRST.
  2. ARTIFACT.md         ← the work's artistic statement.
  3. state/compose-0.png ← the piece as it currently looks (just rendered).

MISSION: make the piece more like itself as a gallery-grade Chinese poetic
artifact. Pick exactly ONE small, high-value aesthetic refinement per turn:
  - coherent themed content (complete Tang quatrain / one same-moment group;
    never UI or status words), ordered reading;
  - real grid / rule-of-thirds composition with deliberate balance;
  - depth & material: brush-weight variation, bloom only on the focal line,
    near-crisp / far-faint layering; restrained palette, no stray artifacts;
  - motion that lets a coherent group enter / hold / read / exit in order.

Hard gate — every turn ALL must pass:
     cargo fmt
     cargo clippy --release -- -D warnings
     cargo build --release
Do not restart display services (the live panel is owned by a root system
service; you cannot modeset). Work only in src/ and scripts/.

If the gate passes:
  git add -A
  git commit -m "art: <one line — how this makes the piece more like itself>"
The outer loop pushes. If the gate fails, revert your edits
(git checkout -- .) and append what failed to state/autoloop.log. Stay minimal.
EOF

timeout 780 claude --dangerously-skip-permissions --print \
  --model "MiniMax-M3[1m]" \
  < state/claude_prompt.txt >> "$LOG" 2>&1
RC=$?
echo "claude_rc=$RC $(date -Is)" >> "$LOG"

# Auto-push the green commit — this is what closes the human out of the loop.
if [ "$RC" = "0" ]; then
    if git push --no-verify origin main >> "$LOG" 2>&1; then
        echo "pushed origin/main at $(date -Is)" >> "$LOG"
    else
        echo "push_failed at $(date -Is) (commit kept local)" >> "$LOG"
    fi
fi

# rotate logs (keep last 200 lines)
[ -f "$LOG" ] && tail -n 200 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"
exit 0
