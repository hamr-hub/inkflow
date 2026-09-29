#!/usr/bin/env bash
# inkflow · scripts/verify-samples.sh
#
# Fail unless the sample frames COMMITTED to git match what the current
# renderer produces. This is the comparison CI runs on every push; running it
# here means a turn can never leave main with a README that quietly lies.
#
# The loop used to refresh the frames on a best-effort basis and push anyway
# (`git commit ... || true`), so two art turns shipped against stale frames and
# the samples gate went red without anyone noticing. Refuse to push instead.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release

BIN="${CARGO_TARGET_DIR:-$PWD/target}/release/inkflow"
if [ ! -x "$BIN" ]; then
    echo "verify-samples: no executable at $BIN (CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>})" >&2
    exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

"$BIN" --works-gallery --out="$TMP" >/dev/null

fail=0
for pair in "0-0 xun-yinzhe" "1-0 jiangxue" "2-0 jingyesi" "3-0 dengguanquelou" "4-0 chunxiao"; do
    set -- $pair
    if ! git show "HEAD:docs/samples/$2.png" > "$TMP/$2.committed" 2>/dev/null; then
        echo "::error::docs/samples/$2.png is not committed at all" >&2
        fail=1
        continue
    fi
    if ! cmp -s "$TMP/works/$1.png" "$TMP/$2.committed"; then
        echo "::error::docs/samples/$2.png does not match the renderer — refresh and commit it" >&2
        fail=1
    fi
done
exit $fail
