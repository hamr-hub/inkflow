#!/usr/bin/env bash
# inkflow · scripts/extract-font.sh
#
# Render each char in GLYPH_SET from Noto Sans CJK into a 4bpp packed
# bitmap and emit src/fontdata.rs. Pure Python + Pillow — no Rust deps.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=src/fontdata.rs
FONT="${1:-/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc}"
test -f "$FONT" || { echo "NotoSansCJK not found at $FONT"; exit 1; }

python3 scripts/extract_font.py "$FONT" > "$OUT"
echo "wrote $OUT ($(wc -c < "$OUT") bytes)"
grep -c '^    (' "$OUT" | xargs -I{} echo "  glyphs: {}"