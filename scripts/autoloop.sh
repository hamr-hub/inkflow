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

# Remember what was already dirty before the turn starts. The loop stages only
# what a turn actually wrote; anything already modified belongs to a human and
# is left alone. A path-wide `git add` once swallowed two unrelated human
# commits into a single "art:" message, and both had to be recovered after they
# had already been pushed.
# `core.fileMode=false` is load-bearing here, not housekeeping. This volume
# reports every file as 755, so without it git calls all 80-odd files
# modified; the snapshot would then treat every path as pre-existing and the
# turn's own change would be indistinguishable from churn. That is not
# hypothetical — it silently discarded a good turn whole, logging only
# "turn produced no changes of its own".
git -c core.fileMode=false status --porcelain | sed 's/^...//' | grep -v '^$' | sort > state/preexisting.txt
# Clear last turn's rationale so a turn that forgets to write one falls back
# to the default instead of silently reusing a stale commit message.
rm -f state/art_message.txt

cat > state/claude_prompt.txt <<'EOF'
You are the unattended curator of "inkflow" — a zero-dependency Rust binary
that is also a generative art piece. Its architecture is a small library
(src/lib.rs: background / color / compose / glyph / phrase / rhythm / scene /
surface / png) plus a harness binary (src/main.rs) that renders compositions.
Every theme is pinned to one complete five-char Tang quatrain (five works
total in phrase::POEM_GROUPS); never break a work into fragment pools.

Read in order each turn:
  1. ART_DIRECTION.md      ← binding visual intent; READ FIRST.
  2. ARTIFACT.md            ← the work's artistic statement.
  3. state/iterations.md    ← what previous turns already tried. READ IT.
  4. state/compose-0.png    ← the piece as it currently looks (just rendered).

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

Do NOT run `git add` or `git commit`. Leave your edits in the working tree.
The outer loop owns staging: it snapshots the tree before your turn, then
stages only the files you actually changed. That is deliberate — an agent told
to `git add src/ scripts/` cannot tell its own work from a human's, and once
did exactly that, publishing two unrelated human commits under one "art:"
message. Anything already modified when your turn began is recorded in
state/preexisting.txt and will not be staged; leave those files alone.

Instead of committing, write your one-line rationale — how this turn makes the
piece more like itself — to:

  state/art_message.txt

That line becomes the commit message, so keep it to a single line.

README sample frames are diffed byte-for-byte by CI, so any turn that moves
ink makes docs/samples/ stale. The outer loop refreshes them after your gate
passes; do not regenerate them yourself.

If the gate fails, revert your own edits and append what failed to
state/autoloop.log. Stay minimal.

Discipline — violations must be reverted:
  - one small change per turn; no sweeping rewrites;
  - DO NOT RE-TREAD. state/iterations.md is the log of what past turns did.
    Two turns an hour apart both lengthened the pause between phrases without
    either knowing of the other, and a third of the piece's real changes went
    to that one idea. If a dimension is already spent, move to another, or go
    genuinely deeper on it and say in your message how. Repeating a settled
    change is not an iteration; it is a turn burnt.
  - the change must be visible. After you commit, the loop runs
    `python3 scripts/art-turn-check.py HEAD~1..HEAD`; if every line you touched
    is a number nudged by less than 10%, the turn is reverted, not published.
    Twenty-odd past turns were exactly that — one float multiplied by 1.025
    each time, with an ever-longer essay about restraint attached. Do not spend
    a turn there. Change the composition, the legibility, the depth, or the
    rhythm, or change nothing;
  - comments are short and factual. NEVER write multi-paragraph or
    history-recapping comment blocks; never justify a change at length;
  - commit message is one line only.
EOF

# How long one turn may take. The scheduler fires about every 21 minutes, and a
# turn that finishes well inside that window leaves the next tick free to run;
# `flock` skips an overlapping tick rather than corrupting the tree. 780 was
# leaving 8 minutes of every tick unused, and rc=124 (killed mid-turn) was as
# common as rc=0. 1080 keeps a 3-minute margin under the cadence.
TURN_TIMEOUT="${TURN_TIMEOUT:-1080}"

# NOTE: the value below is NOT recognised by the claude-code CLI. Every tick
# logs `[claude-code:unrecognized_model]` and the loop silently falls back to
# whatever the CLI defaults to — so this flag has never actually selected a
# model, and neither the turn-to-turn timing spread nor the `Token Plan` 429s
# should be attributed to the model named here. Set TURN_MODEL to a name this
# CLI accepts, and confirm the warning is gone, before trusting the model's
# cost or latency.
TURN_MODEL="${TURN_MODEL:-MiniMax-M3[1m]}"

timeout "$TURN_TIMEOUT" claude --dangerously-skip-permissions --print \
  --model "$TURN_MODEL" \
  < state/claude_prompt.txt >> "$LOG" 2>&1
RC=$?
echo "claude_rc=$RC $(date -Is)" >> "$LOG"

# A turn that died — killed by the timeout, or a model error — leaves whatever
# it had written so far in the tree. Those edits look "pre-existing" to the next
# turn, which then refuses to stage that file, so one bad turn can block that
# path from being committed ever again. Put the tree back the way the tick
# found it. Only what this turn touched: anything already dirty when the tick
# started is somebody else's and stays.
if [ "$RC" != "0" ]; then
    git -c core.fileMode=false status --porcelain | sed 's/^...//' | grep -v '^$' | grep -v '^state/' \
        | sort > state/dirty_after_fail.txt 2>/dev/null || true
    grep -vxFf state/preexisting.txt state/dirty_after_fail.txt > state/dirty_orphan.txt 2>/dev/null || true
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        if git ls-files --error-unmatch -- "$f" >/dev/null 2>&1; then
            git checkout -- "$f" >> "$LOG" 2>&1
        else
            rm -f "$f" >> "$LOG" 2>&1
        fi
    done < state/dirty_orphan.txt
    if [ -s state/dirty_orphan.txt ]; then
        echo "turn failed (rc=$RC); reverted its edits: $(tr '\n' ' ' < state/dirty_orphan.txt)" >> "$LOG"
    fi
    exit 0
fi

# Auto-push the green commit — this is what closes the human out of the loop.
if [ "$RC" = "0" ]; then
    # What did this turn actually write? Everything already dirty when the turn
    # began is somebody else's and stays out of the commit.
    # state/ is the loop's own bookkeeping (log, snapshots, render evidence),
    # not the art — it is created after the snapshot and would otherwise look
    # like this turn's work and get committed.
    git -c core.fileMode=false status --porcelain | sed 's/^...//' | grep -v '^$' | grep -v '^state/' \
        | sort > state/dirty_now.txt
    grep -vxFf state/preexisting.txt state/dirty_now.txt > state/dirty_turn.txt 2>/dev/null || true
    LEFT_BEHIND="$(comm -23 state/dirty_now.txt state/dirty_turn.txt 2>/dev/null | tr '\n' ' ')"
    if [ -n "$LEFT_BEHIND" ]; then
        echo "left unstaged (pre-existing, not ours): $LEFT_BEHIND" >> "$LOG"
    fi

    if [ ! -s state/dirty_turn.txt ]; then
        echo "turn produced no changes of its own — nothing to commit" >> "$LOG"
        exit 0
    fi

    # Judge the turn's own diff before it is allowed to become a commit. The
    # loop once spent twenty-odd consecutive turns multiplying a single float
    # by 1.025 and calling each one an aesthetic decision, with commit messages
    # long enough to hide how little had moved. The prompt already asked for one
    # visible refinement and one-line messages; the loop ignored both, so the
    # refusal is mechanical now.
    if [ -f scripts/art-turn-check.py ]; then
        if ! python3 scripts/art-turn-check.py >> "$LOG" 2>&1; then
            echo "refused: turn only nudged numbers — discarding this turn's edits" >> "$LOG"
            while IFS= read -r f; do
                [ -n "$f" ] || continue
                if git ls-files --error-unmatch -- "$f" >/dev/null 2>&1; then
                    git checkout -- "$f" >> "$LOG" 2>&1
                else
                    rm -f "$f" >> "$LOG" 2>&1
                fi
            done < state/dirty_turn.txt
            exit 0
        fi
    fi

    # The gate, re-run here rather than trusted: the loop is what publishes, so
    # the loop is what verifies.
    if ! cargo fmt --check >> "$LOG" 2>&1 \
       || ! cargo clippy --release --all-targets -- -D warnings >> "$LOG" 2>&1 \
       || ! cargo build --release >> "$LOG" 2>&1; then
        echo "gate failed in the outer loop — not committing" >> "$LOG"
        exit 0
    fi

    # The agent's one-line rationale becomes the commit message (ARTIFACT.md:
    # a decision that has to be readable as a judgement about the work).
    # The loop owns the prefix, so strip any the agent wrote before adding one
    # back — otherwise the subject line reads "art: art: ...".
    ART_MSG="art: one refinement to the piece"
    if [ -s state/art_message.txt ]; then
        LINE="$(head -1 state/art_message.txt | sed 's/^[[:space:]]*//; s/^art:[[:space:]]*//')"
        [ -n "$LINE" ] && ART_MSG="art: $LINE"
    fi

    STAGED=""
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        if git add -- "$f" >> "$LOG" 2>&1; then
            STAGED="$STAGED $f"
        fi
    done < state/dirty_turn.txt
    if [ -z "$STAGED" ]; then
        echo "nothing staged — git add failed for: $(cat state/dirty_turn.txt | tr '\n' ' ')" >> "$LOG"
        exit 0
    fi
    if ! git diff --cached --quiet; then
        git commit -q -m "$ART_MSG" >> "$LOG" 2>&1 || {
            echo "commit failed — not pushing" >> "$LOG"; exit 0; }
        echo "committed: $ART_MSG" >> "$LOG"
        # Remember what this turn did, so the next one can tell a fresh idea
        # from a re-tread. The agent has no memory between ticks otherwise.
        {
            printf '%s  %s\n' "$(date +%m-%d' '%H:%M)" "$ART_MSG"
            printf '    touched: %s\n' "$(git show --name-only --format='' HEAD \
                | grep '^src/' | tr '\n' ' ')"
        } >> state/iterations.md
    fi

    # Any turn that moved ink left the committed sample frames describing the
    # previous render, and CI diffs them byte-for-byte. Refresh before pushing
    # so the piece and its picture of itself stay the same generation.
    if [ -f scripts/refresh-samples.sh ]; then
        if ! scripts/refresh-samples.sh >> "$LOG" 2>&1; then
            echo "sample refresh failed — not pushing" >> "$LOG"
            exit 0
        fi
        git add docs/samples/ >> "$LOG" 2>&1
        git commit -q -m "art: refresh the README sample frames to match this turn" >> "$LOG" 2>&1 || true
    fi

    # Never push a turn whose committed frames disagree with the renderer. This
    # is the check CI already runs, run here so a red main cannot leave the box:
    # the old best-effort `|| true` commit is how art turns shipped against
    # stale frames in the first place.
    if [ -f scripts/verify-samples.sh ]; then
        if ! scripts/verify-samples.sh >> "$LOG" 2>&1; then
            echo "verify-samples failed — refusing to push a red main" >> "$LOG"
            exit 0
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
