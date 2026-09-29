#!/usr/bin/env bash
# inkflow · scripts/autoloop-selftest.sh
#
# Exercise scripts/autoloop.sh's post-turn control flow without a model, a
# network, or the real renderer.
#
# Why this exists: the loop publishes to a real remote, unattended, every twenty
# minutes. Its staging / judging / push decisions were rewritten (the agent no
# longer runs `git add`, the loop owns it) and on a busy day the model is quota
# limited, so the code that decides what gets published can go a long time
# without running once. This drives it through a scratch repo with a local bare
# origin and stubbed tools, and asserts the outcomes that matter:
#
#   1. a real turn is committed with the agent's own one-line rationale
#   2. a turn that only nudged a number is refused AND its edits discarded
#   3. edits already in the tree before the turn are never staged
#   4. a turn that changed nothing commits nothing
#   5. the push is refused when the sample frames disagree with the renderer
#
# Harness-only substitutions: flock/timeout/cargo/claude are stubbed, and the
# Linux-only /proc memory guard plus the hardcoded CARGO_TARGET_DIR are patched
# out. Everything else is the real script, unmodified.
set -uo pipefail
cd "$(dirname "$0")/.."
REAL="$(pwd)"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT

PASS=0
FAIL=0
check() {
    if [ "$2" = "0" ]; then
        echo "  ok   $1"
        PASS=$((PASS + 1))
    else
        echo "  FAIL $1"
        FAIL=$((FAIL + 1))
    fi
}

mkdir -p "$ROOT/bin"
printf '#!/usr/bin/env bash\nexit 0\n' > "$ROOT/bin/flock"
# A faithful-enough `timeout`: macOS has no timeout(1), and a stub that merely
# execs would make the killed-turn scenario pass against a loop that never
# times out at all. GNU timeout reports 124 when it kills, so do the same.
cat > "$ROOT/bin/timeout" <<'STUB'
#!/usr/bin/env bash
secs="$1"; shift
"$@" &
pid=$!
( sleep "$secs"; kill -TERM "$pid" 2>/dev/null ) >/dev/null 2>&1 &
killer=$!
wait "$pid" 2>/dev/null
rc=$?
kill -9 "$killer" 2>/dev/null
wait "$killer" 2>/dev/null
# GNU timeout reports 124 when it had to kill the child.
[ "$rc" -eq 143 ] && rc=124
exit "$rc"
STUB
printf '#!/usr/bin/env bash\nexit 0\n' > "$ROOT/bin/cargo"
cat > "$ROOT/bin/claude" <<'STUB'
#!/usr/bin/env bash
[ -n "${SCENARIO:-}" ] && eval "$SCENARIO"
exit "${AGENT_RC:-0}"
STUB
chmod +x "$ROOT/bin"/*

export HOME="$ROOT/home"
REPO="$HOME/codespace/personal/inkflow"
mkdir -p "$REPO/scripts" "$REPO/src" "$REPO/docs/samples" "$REPO/state"
git init -q --bare "$ROOT/origin.git"
cd "$REPO"
git init -q -b main .
git config user.email selftest@inkflow
git config user.name selftest
printf 'fn main() { let alpha = 0.50; }\n' > src/alpha.rs
printf 'fn helper() {}\n' > src/beta.rs
printf 'x\n' > scripts/thing.sh
git add -A >/dev/null && git commit -qm "base"
git remote add origin "$ROOT/origin.git"
git push -q -u origin main

# The real script, with only the Linux-only bits swapped for the harness.
sed -e 's#^AVAIL_KB=.*#AVAIL_KB=9999999#' \
    -e "s#^export CARGO_TARGET_DIR=.*#export CARGO_TARGET_DIR=$ROOT/ctd#" \
    "$REAL/scripts/autoloop.sh" > "$REPO/scripts/autoloop.sh"
chmod +x "$REPO/scripts/autoloop.sh"
cp "$REAL/scripts/art-turn-check.py" "$REPO/scripts/"

# A harness that failed to install the script under test would make every
# assertion below pass (or fail) for the wrong reason.
[ -s "$REPO/scripts/autoloop.sh" ] && [ -s "$REPO/scripts/art-turn-check.py" ]
check "0. the script under test was installed into the scratch repo" $?

# Stand-ins for the sample-frame scripts: the real ones need a real renderer.
# These just let the control flow run, and let scenario 5 fail verification.
# Idempotent on purpose: a refresh that rewrites the frame would add a second
# commit on top of the turn's and hide it from the assertions below. The
# refresh-commit path is exercised by scenario 5 instead.
printf '#!/usr/bin/env bash\necho refreshed\n' \
    > "$REPO/scripts/refresh-samples.sh"
printf '#!/usr/bin/env bash\n[ -f state/force-fail-verify ] && exit 1\nexit 0\n' \
    > "$REPO/scripts/verify-samples.sh"
chmod +x "$REPO/scripts/refresh-samples.sh" "$REPO/scripts/verify-samples.sh"
git add -A >/dev/null && git commit -qm "harness" && git push -q origin main

export PATH="$ROOT/bin:$PATH"
run_loop() {
    "$REPO/scripts/autoloop.sh" >/dev/null 2>&1
    LAST_RC=$?
}
head_msg() { git log -1 --format=%s; }

# --- 1. a real turn, committed with the agent's own rationale ---------------
export SCENARIO='
  echo "fn main() { let alpha = 0.62; let added = 1; }" > src/alpha.rs
  echo "art: the focal line earns its weight" > state/art_message.txt
'
run_loop
grep -q 'claude_rc=0' state/autoloop.log
check "0b. the loop really ran the agent and the post-turn path" $?
[ "$(head_msg)" = "art: the focal line earns its weight" ]
check "1. turn committed with the agent's rationale" $?
case "$(head_msg)" in *"art: art:"*) BAD=1 ;; *) BAD=0 ;; esac
check "1d. the subject line has exactly one 'art:' prefix" $BAD
git show --name-only --format= HEAD | grep -q 'src/alpha.rs'
check "1b. the turn's file is in the commit" $?

# --- 2. a turn that only nudged a number is refused and discarded ----------
export SCENARIO='
  sed -i.bak "s/let alpha = 0.62;/let alpha = 0.6355;/" src/alpha.rs
  echo "art: nudge" > state/art_message.txt
'
run_loop
grep -q 'let alpha = 0.62;' src/alpha.rs
check "2. numeric nudge refused and its edit discarded" $?
[ "$(head_msg)" != "art: nudge" ]
check "2b. nudge never became a commit" $?

# --- 3. pre-existing human work is never staged ----------------------------
echo "// human work in progress" >> src/beta.rs
export SCENARIO='
  echo "fn main() { let alpha = 0.62; let added = 2; }" > src/alpha.rs
  echo "art: a real change" > state/art_message.txt
'
run_loop
! git show --name-only --format= HEAD | grep -q 'src/beta.rs'
check "3. the human's uncommitted file stayed out of the commit" $?
grep -q 'human work in progress' src/beta.rs
check "3b. and it is still in the working tree" $?

# --- 4. mode-only churn must not make a file look "pre-existing" ----------
# This volume reports every file as 755. Without core.fileMode=false the
# snapshot sees all 80-odd paths as modified, so the turn's own edit is
# indistinguishable from churn and the turn is silently thrown away — which is
# exactly what happened to a good turn: it made a real change, passed the gate,
# and the loop logged "turn produced no changes of its own".
# Churn the file the turn will edit, not just its neighbours: on the real
# volume *every* path looks modified, so the turn's own target is the one that
# matters.
chmod 755 src/alpha.rs src/beta.rs scripts/thing.sh
export SCENARIO='
  echo "fn main() { let alpha = 0.62; let touched_churned = 1; }" > src/alpha.rs
  echo "art: a real change beside mode churn" > state/art_message.txt
'
run_loop
[ "$(head_msg)" = "art: a real change beside mode churn" ]
check "4. a turn commits even when the tree is full of mode churn" $?

# --- 5. a turn that changed nothing commits nothing ------------------------
BEFORE="$(git rev-parse HEAD)"
export SCENARIO='true'
run_loop
[ "$(git rev-parse HEAD)" = "$BEFORE" ]
check "5. an empty turn produced no commit" $?

# --- 5. a turn killed mid-way commits nothing half-finished ---------------
# The real loop runs every ~21 minutes; a slow turn used to be cut off at 780s
# and the tick wasted. The budget is now TURN_TIMEOUT, and the property that
# matters is not the number but what a kill leaves behind: a half-written file
# must not become a commit.
BEFORE="$(git rev-parse HEAD)"
export SCENARIO='
  echo "fn main() { let alpha = 0.62; let half_done = 1; }" > src/alpha.rs
  sleep 30
  echo "art: never reached" > state/art_message.txt
'
export TURN_TIMEOUT=2   # a deliberately tiny budget: the stub sleeps 30s
run_loop
[ "$(git rev-parse HEAD)" = "$BEFORE" ]
check "6. a turn killed by the timeout committed nothing" $?
if grep -q 'claude_rc=124' state/autoloop.log; then
    check "6b. and the loop reported it as a timeout, not a crash" 0
else
    check "6b. and the loop reported it as a timeout, not a crash" 1
fi
unset TURN_TIMEOUT
# The half-written file must be gone, not merely uncommitted. A killed turn that
# leaves the path dirty would make the next tick treat it as pre-existing, and
# that path could then never be committed again.
! grep -q 'half_done' src/alpha.rs
check "6c. and the killed turn's half-written edit was reverted" $?

# --- 7. the push is refused when the frames disagree -----------------------
export SCENARIO='
  echo "fn main() { let alpha = 0.62; let added = 3; }" > src/alpha.rs
  echo "art: another real change" > state/art_message.txt
  touch state/force-fail-verify
'
run_loop
LOCAL="$(git rev-parse HEAD)"
REMOTE="$(git rev-parse origin/main)"
[ "$LOCAL" != "$REMOTE" ]
check "7. a turn whose samples failed verification was not pushed" $?
rm -f state/force-fail-verify

echo
echo "autoloop-selftest: $PASS passed, $FAIL failed"
[ "$FAIL" = "0" ]
