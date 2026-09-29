#!/usr/bin/env bash
# inkflow · scripts/refresh-samples.sh
#
# Regenerate the README's sample frames from the current renderer. CI diffs
# these against a fresh render, so run this after any change that moves ink —
# otherwise the README quietly shows a picture the code no longer produces.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# One hero frame per pinned work, in POEM_BY_THEME order.
./target/release/inkflow --works-gallery --out="$TMP" >/dev/null 2>&1

# A touch accent: the compose run taps the upper-right echo near the end.
./target/release/inkflow --compose-test=12 --out="$TMP" >/dev/null 2>&1

mkdir -p docs/samples
cp "$TMP"/works/0-0.png docs/samples/xun-yinzhe.png
cp "$TMP"/works/1-0.png docs/samples/jiangxue.png
cp "$TMP"/works/2-0.png docs/samples/jingyesi.png
cp "$TMP"/works/3-0.png docs/samples/dengguanquelou.png
cp "$TMP"/works/4-0.png docs/samples/chunxiao.png
cp "$TMP"/compose-11.png docs/samples/touch-accent.png

echo "refreshed $(ls docs/samples | wc -l) sample frames"
