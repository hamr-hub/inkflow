#!/usr/bin/env bash
# inkflow · scripts/check-docs.sh
#
# Fail if a doc points at a source file that is not there.
#
# The generated docs under docs/ spent months describing the pre-rewrite
# architecture — mood.rs, llm_loop.rs, net_ollama.rs, drm.rs, evdev.rs and a
# dozen more that now live in legacy/, plus LOC counts off by a factor of five.
# Nothing flagged it, because a wrong doc looks exactly like a right one. A
# zero-dependency Rust project cannot run a linter over its prose, so this
# checks the one thing that actually rots: file references.
set -uo pipefail
cd "$(dirname "$0")/.."

status=0
note() { echo "  $1"; }

echo "docs referencing source files that do not exist:"
# Gather first, then loop: a `while read` fed by a pipe runs in a subshell, so
# setting a flag inside it would never reach this shell.
broken=""
for doc in docs/*.md README.md; do
    [ -f "$doc" ] || continue
    for m in $(grep -oE 'src/[A-Za-z_0-9]+\.rs' "$doc" 2>/dev/null \
               | sed 's|src/||; s|\.rs$||' | sort -u); do
        [ -z "$m" ] && continue
        if [ ! -f "src/$m.rs" ]; then
            broken="$broken$doc:$m "
        fi
    done
done
if [ -z "$broken" ]; then
    note "none"
else
    for entry in $broken; do
        doc="${entry%%:*}"
        mod="${entry##*:}"
        echo "::error::$doc names src/$mod.rs, which is not in src/ (archived modules live in legacy/src/)"
    done
    status=1
fi

# The sub-doc index must not promise files that were deleted.
for promised in ARCHITECTURE.md; do
    if grep -q "$promised" AGENTS.md 2>/dev/null && [ ! -f "docs/$promised" ]; then
        echo "::error::AGENTS.md points at docs/$promised, which does not exist"
        status=1
    fi
done

# Numbers rot as surely as file paths: a doc quoting a line count is a claim
# that goes stale the moment anyone edits the file. REFACTOR_PLAN.md is the one
# doc that quotes them, and it must agree with the tree.
while IFS= read -r row; do
    file="$(echo "$row" | sed -n 's/.*`\([a-z_0-9]*\.rs\)`.*/\1/p')"
    claimed="$(echo "$row" | awk -F'|' '{gsub(/ /,"",$3); print $3}')"
    [ -z "$file" ] || [ -z "$claimed" ] && continue
    mod="${file%.rs}"
    actual=$(wc -l < "src/$file" 2>/dev/null | tr -d ' ')
    [ -z "$actual" ] && continue
    if [ "$actual" != "$claimed" ]; then
        echo "::error::docs quote $file as $claimed lines; it is $actual"
        status=1
    fi
    # The test column counts #[test] in the module *and* its tests/ submodule.
    # grep -c already prints 0 when there is no match; adding `|| echo 0`
    # would append a second 0 and break the arithmetic.
    claimed_tests="$(echo "$row" | awk -F'|' '{gsub(/ /,"",$4); print $4}')"
    [ -z "$claimed_tests" ] && continue
    real_tests=$(grep -c '#\[test\]' "src/$file" 2>/dev/null)
    extra="src/$mod/tests.rs"
    if [ -f "$extra" ]; then
        real_tests=$((real_tests + $(grep -c '#\[test\]' "$extra" 2>/dev/null)))
    fi
    # A dash in the table means "no tests" — it is an em dash, which bash's
    # `case` cannot pattern-match, so treat any non-numeric claim as zero.
    if ! printf '%s' "$claimed_tests" | grep -qE '^[0-9]+$'; then
        if [ "$real_tests" -ne 0 ]; then
            echo "::error::docs claim $file has no tests; it has $real_tests"
            status=1
        fi
    elif [ "$real_tests" != "$claimed_tests" ]; then
        echo "::error::docs quote $file as having $claimed_tests tests; it has $real_tests"
        status=1
    fi
done < <(grep -E '^\| `[a-z_0-9]+\.rs` \| [0-9]+ \|' docs/REFACTOR_PLAN.md 2>/dev/null)

exit $status
