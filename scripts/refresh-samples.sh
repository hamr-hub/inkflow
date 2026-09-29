#!/usr/bin/env bash
# inkflow · scripts/refresh-samples.sh
#
# Regenerate the README's sample frames from the current renderer. CI diffs
# these against a fresh render, so run this after any change that moves ink —
# otherwise the README quietly shows a picture the code no longer produces.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release

# Render with the binary this script just built. Honour CARGO_TARGET_DIR: the
# autoloop exports it, so a hardcoded ./target/release/inkflow is a *different*
# binary — stale, or missing entirely. That bug made every art turn ship a
# renderer change against unchanged sample frames, and CI's byte-for-byte
# samples gate went red on main without anyone noticing.
BIN="${CARGO_TARGET_DIR:-$PWD/target}/release/inkflow"
if [ ! -x "$BIN" ]; then
    echo "refresh-samples: no executable at $BIN (CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>})" >&2
    exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# One hero frame per pinned works, in POEM_BY_THEME order.
"$BIN" --works-gallery --out="$TMP" >/dev/null

# A touch accent: the compose run taps the upper-right echo near the end.
"$BIN" --compose-test=12 --out="$TMP" >/dev/null

# Never publish an empty or half-written frame set. A silently-missing render
# used to look exactly like "nothing changed", which is how the gate went red.
for f in works/0-0.png works/1-0.png works/2-0.png works/3-0.png works/4-0.png compose-11.png; do
    if [ ! -s "$TMP/$f" ]; then
        echo "refresh-samples: render produced no $f — refusing to touch docs/samples" >&2
        exit 1
    fi
done

mkdir -p docs/samples
cp "$TMP"/works/0-0.png docs/samples/xun-yinzhe.png
cp "$TMP"/works/1-0.png docs/samples/jiangxue.png
cp "$TMP"/works/2-0.png docs/samples/jingyesi.png
cp "$TMP"/works/3-0.png docs/samples/dengguanquelou.png
cp "$TMP"/works/4-0.png docs/samples/chunxiao.png
cp "$TMP"/compose-11.png docs/samples/touch-accent.png

echo "refreshed $(ls docs/samples | wc -l) sample frames"
