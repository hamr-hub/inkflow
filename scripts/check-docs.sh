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

exit $status
