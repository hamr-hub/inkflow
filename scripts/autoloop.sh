#!/usr/bin/env bash
# inkflow autonomous art iteration — one bounded agent turn per timer tick.
# Runs inside the GitHub-backed tree (personal/inkflow) so every green turn
# is committed AND pushed to origin/main without a human relaying it.
set -uo pipefail

REPO="$HOME/codespace/personal/inkflow"
cd "$REPO" || exit 0
export PATH="$HOME/.cargo/bin:$HOME/.nvm/versions/node/v22.22.1/bin:$PATH"
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

# Fresh render evidence. Never render with a stale binary: a failed build
# skips the turn so old compositions can't overwrite current evidence.
if ! cargo build --release >> "$LOG" 2>&1; then
  echo "skip: build failed — no render, no agent turn" >> "$LOG"
  exit 0
fi
"$CARGO_TARGET_DIR/release/inkflow" --compose-test 12 >> "$LOG" 2>&1 || true

cat > state/claude_prompt.txt <<'EOF'
You are the unattended curator of "inkflow" — a zero-dependency Rust binary
that is also a generative art piece. Its architecture is a small library
(src/lib.rs: background / color / compose / glyph / phrase / rhythm / scene /
surface / png) plus a harness binary (src/main.rs) that renders compositions.
Every theme is pinned to one complete five-char Tang quatrain (five works
total in phrase::POEM_GROUPS); never break a work into fragment pools.

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

Your turn MUST move the art, so never stage work you did not write. The
outer loop stages only src/ scripts/ docs/samples/ for exactly this reason:
`git add -A` once swallowed two unrelated human commits into a single "art:"
message describing neither, and both had to be recovered after they had
already been pushed. If you find unrelated edits in the tree, leave them
alone — they are somebody else's and the loop will not touch them.

README sample frames are diffed byte-for-byte by CI, so any turn that moves
ink makes docs/samples/ stale. The outer loop refreshes them after your
gate passes; do not regenerate them yourself.

If the gate passes:
  git add src/ scripts/ docs/samples/
  git commit -m "art: <one line — how this makes the piece more like itself>"
The outer loop refreshes the samples and pushes. If the gate fails, revert
your edits (git checkout -- src/) and append what failed to
state/autoloop.log. Stay minimal.

Discipline — violations must be reverted:
  - one small change per turn; no sweeping rewrites;
  - comments are short and factual. NEVER write multi-paragraph or
    history-recapping comment blocks; never justify a change at length;
  - commit message is one line only.
EOF

timeout 780 claude --dangerously-skip-permissions --print \
  --model "MiniMax-M3[1m]" \
  < state/claude_prompt.txt >> "$LOG" 2>&1
RC=$?
echo "claude_rc=$RC $(date -Is)" >> "$LOG"

# Auto-push the green commit — this is what closes the human out of the loop.
if [ "$RC" = "0" ]; then
    # Any turn that moved ink left the committed sample frames describing the
    # previous render, and CI diffs them byte-for-byte. Refresh before pushing
    # so the piece and its picture of itself stay the same generation.
    if [ -x scripts/refresh-samples.sh ] || [ -f scripts/refresh-samples.sh ]; then
        if ! scripts/refresh-samples.sh >> "$LOG" 2>&1; then
            echo "sample refresh failed — not pushing" >> "$LOG"
            exit 0
        fi
        git add docs/samples/ >> "$LOG" 2>&1 || true
        if ! git diff --cached --quiet; then
            git commit -q -m "art: refresh the README sample frames to match this turn" >> "$LOG" 2>&1 || true
        fi
    fi
    if git push --no-verify origin main >> "$LOG" 2>&1; then
        echo "pushed origin/main at $(date -Is)" >> "$LOG"
    else
        echo "push_failed at $(date -Is) (commit kept local)" >> "$LOG"
    fi
fi

# rotate logs (keep last 200 lines)
[ -f "$LOG" ] && tail -n 200 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"
exit 0
