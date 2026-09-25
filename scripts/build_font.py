#!/usr/bin/env python3
"""
build_font.py — generate the embedded grayscale-coverage font for inkflow.

Reads the curated phrase bank from src/phrase.rs, collects every unique
codepoint plus a small set of ASCII / CJK punctuation, and renders each
codepoint at high resolution with real anti-aliased coverage from the system
Noto Sans CJK JP face (which covers all the CJK Unified Ideographs we use).

Two pre-rendered buckets are emitted:

    hero   = 128 px em   (used at native render size — never upscale)
    body   =  72 px em   (used for smaller text or downsampling)

Both buckets ship as raw 8-bit coverage bytes (one byte per pixel, value 0..=255),
so `draw_glyph` composites with smooth, font-quality edges — no upscaling of a
tiny 1-bit mask.

The whole atlas is committed into `src/glyph_table.rs`; the generator itself
is *not* a runtime dependency.

Memory budget per glyph (tight-bbox, 8-bit):
    hero worst-case  128 * 128 =  16 KiB
    hero typical    ~ 95 *  95 =  ~9 KiB
    body worst-case   72 *  72 =  ~5 KiB
    body typical    ~ 56 *  56 =  ~3 KiB
For ~170 glyphs the whole table is ~1.6 MiB raw, ~7 MiB as committed Rust —
manageable for a zero-dep inkflow atlas.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
SRC_DIR = ROOT / "src"
OUT = SRC_DIR / "glyph_table.rs"

CJK_PATH = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"

# Size buckets: hero is the native em we render at, body is for downsamples.
HERO_PX = 128
BODY_PX = 72

# ------------------------------------------------------------------
# Collect every codepoint we need to ship.
# ------------------------------------------------------------------
def collect_codepoints() -> list[int]:
    phrase_src = (SRC_DIR / "phrase.rs").read_text(encoding="utf-8")
    cps: set[int] = set()
    # Phrase text (the lines shown in the composition).
    for m in re.finditer(r'text:\s*"([^"]+)"', phrase_src):
        for ch in m.group(1):
            cps.add(ord(ch))
    # Poem-group titles (rendered as a faint baseline inscription below the
    # composition; e.g. 寻隐者不遇, the title of 《寻隐者不遇》 by 贾岛).
    for m in re.finditer(r'title:\s*"([^"]+)"', phrase_src):
        for ch in m.group(1):
            cps.add(ord(ch))
    # ASCII space + light punctuation + common CJK punctuation.
    for ch in " \n\t!?,.;:'\"“”‘’—…·《》、。":
        cps.add(ord(ch))
    return sorted(cps)


# ------------------------------------------------------------------
# FontTools metrics for advance width; PIL for rasterisation.
# ------------------------------------------------------------------
_cjk: TTFont | None = None
def get_cjk() -> TTFont:
    global _cjk
    if _cjk is None:
        _cjk = TTFont(CJK_PATH, fontNumber=0)
    return _cjk


def render_glyph(cp: int, px: int) -> tuple[bytes, int, int, int, int, int]:
    """Render a single glyph.

    Returns: (coverage_bytes, w, h, bearing_x, bearing_y, advance_px).
    coverage_bytes is the tight-bbox grayscale alpha (8-bit, 0..=255), row-major.
    bearing_x = px from pen position to left edge of bbox (can be negative).
    bearing_y = px from baseline UP to top of bbox.
    advance_px = px to next pen.
    """
    cjk = get_cjk()
    cmap = cjk.getBestCmap()
    glyph_name = cmap.get(cp, ".notdef")
    units_per_em = cjk["head"].unitsPerEm
    advance_units, _lsb_units = cjk["hmtx"][glyph_name]
    advance_px = max(1, int(round(advance_units * (px / units_per_em))))

    pil_font = ImageFont.truetype(CJK_PATH, px)

    # Use font.getbbox() to find the rendered bbox of the glyph in pen
    # coordinates (y grows UP from baseline).
    try:
        bbox_pen = pil_font.getbbox(chr(cp))
    except Exception:
        bbox_pen = None

    if bbox_pen is None:
        # Empty glyph (e.g., space).
        return (b"\x00", 1, 1, 0, 0, advance_px)

    bx0, by0, bx1, by1 = bbox_pen
    gw = bx1 - bx0
    gh = by1 - by0
    if gw <= 0 or gh <= 0:
        return (b"\x00", 1, 1, 0, 0, advance_px)

    # Render the glyph into a small image.  We use ImageDraw.text with a
    # negative offset so the glyph's bbox origin sits at (0, 0).
    glyph_img = Image.new("L", (gw, gh), 0)
    draw = ImageDraw.Draw(glyph_img)
    draw.text((-int(bx0), -int(by0)), chr(cp), fill=255, font=pil_font)

    # Trim to the actual non-zero bbox (in case of any ghosting).
    bbox = glyph_img.getbbox()
    if bbox is None or bbox[2] - bbox[0] <= 0 or bbox[3] - bbox[1] <= 0:
        return (b"\x00", 1, 1, 0, 0, advance_px)
    cropped = glyph_img.crop(bbox)
    w, h = cropped.size
    data = cropped.tobytes()  # raw 8-bit grayscale, length w*h

    # bearing_x = bbox[0] - bx0  (negative if pen is inside the bbox)
    # bearing_y = -by0  (distance from baseline UP to top of bbox)
    bearing_x = bbox[0] - bx0
    bearing_y = -by0

    return (data, w, h, bearing_x, bearing_y, advance_px)


# ------------------------------------------------------------------
# Emit the Rust source.
# ------------------------------------------------------------------
def _repr_ch(cp: int) -> str:
    """Render a codepoint as a Rust char literal, escaping control chars."""
    ch = chr(cp)
    if ch == "\\":
        return r"'\\'"
    if ch == "'":
        return r"'\''"
    if ch == "\n":
        return r"'\n'"
    if ch == "\t":
        return r"'\t'"
    if ch == "\r":
        return r"'\r'"
    if ord(ch) < 0x20 or ord(ch) == 0x7F:
        return f"'\\u{{{ord(ch):04X}}}'"
    if ord(ch) > 0x7E:
        # Non-ASCII printable — Rust accepts the literal directly.
        return f"'{ch}'"
    return f"'{ch}'"


def _emit_bucket(
    name: str,
    px: int,
    data_parts: list[bytes],
    offsets: list[int],
    glyph_infos: list[dict],
) -> list[str]:
    total = sum(len(p) for p in data_parts)
    lines: list[str] = []
    lines.append(f"/// {name.capitalize()} bucket: 8-bit coverage at {px} px em.")
    lines.append(f"pub static {name}_DATA: [u8; {total}] = [")
    cur = 0
    for part, info in zip(data_parts, glyph_infos):
        ch = _repr_ch(info["cp"])
        lines.append(f"    // offset {cur} ({len(part)} bytes) U+{info['cp']:04X} {ch}")
        # 32 bytes per line keeps the source from being ridiculously tall.
        for i in range(0, len(part), 32):
            row = part[i:i + 32]
            lines.append("    " + ",".join(f"0x{b:02X}" for b in row) + ",")
        cur += len(part)
    lines.append("];")
    return lines


def emit_rust(codepoints: list[int]) -> str:
    glyph_infos: list[dict] = []
    hero_data_parts: list[bytes] = []
    body_data_parts: list[bytes] = []
    hero_offsets: list[int] = []
    body_offsets: list[int] = []

    for cp in codepoints:
        try:
            hb, hw, hh, hbx, hby, hadv = render_glyph(cp, HERO_PX)
            hero_part = hb
        except Exception as e:
            print(f"warn: hero render failed for U+{cp:04X}: {e}", file=sys.stderr)
            hero_part = b"\x00"; hw = hh = 1; hbx = hby = 0; hadv = 1
        try:
            bb, bw, bh, bbx, bby, badv = render_glyph(cp, BODY_PX)
            body_part = bb
        except Exception as e:
            print(f"warn: body render failed for U+{cp:04X}: {e}", file=sys.stderr)
            body_part = b"\x00"; bw = bh = 1; bbx = bby = 0; badv = 1

        hero_offsets.append(sum(len(p) for p in hero_data_parts))
        hero_data_parts.append(hero_part)
        body_offsets.append(sum(len(p) for p in body_data_parts))
        body_data_parts.append(body_part)

        glyph_infos.append({
            "cp": cp,
            "hero_w": hw, "hero_h": hh, "hero_bx": hbx, "hero_by": hby, "hero_adv": hadv,
            "body_w": bw, "body_h": bh, "body_bx": bbx, "body_by": bby, "body_adv": badv,
        })

    index_rows: list[tuple[int, int]] = [(info["cp"], i) for i, info in enumerate(glyph_infos, start=1)]

    lines: list[str] = []
    lines.append("// AUTO-GENERATED by scripts/build_font.py — do not edit by hand.")
    lines.append(f"// Source: NotoSansCJK-Regular (JP face covers all CJK we use).")
    lines.append(f"// Format: 8-bit coverage per glyph, two buckets (hero {HERO_PX}px / body {BODY_PX}px).")
    lines.append("")
    lines.append("/// A single pre-rendered glyph (one bucket).")
    lines.append("#[derive(Clone, Copy)]")
    lines.append("#[repr(C)]")
    lines.append("pub struct Glyph {")
    lines.append("    /// Offset into the bucket byte stream.")
    lines.append("    pub offset: u32,")
    lines.append("    /// Tight bbox width in source pixels.")
    lines.append("    pub w: u16,")
    lines.append("    /// Tight bbox height in source pixels.")
    lines.append("    pub h: u16,")
    lines.append("    /// Horizontal bearing — distance from pen to left edge of bbox.")
    lines.append("    pub bearing_x: i16,")
    lines.append("    /// Vertical bearing — distance from baseline UP to top of bbox.")
    lines.append("    pub bearing_y: i16,")
    lines.append("    /// Pen advance to next character (pixels).")
    lines.append("    pub advance: u16,")
    lines.append("}")
    lines.append("")
    lines.append("/// Codepoint → glyph slot (binary-search friendly).")
    lines.append("#[derive(Clone, Copy)]")
    lines.append("#[repr(C)]")
    lines.append("pub struct IndexEntry {")
    lines.append("    pub cp: u32,")
    lines.append("    pub slot: u16,")
    lines.append("}")
    lines.append("")
    lines.extend(_emit_bucket("HERO", HERO_PX, hero_data_parts, hero_offsets, glyph_infos))
    lines.append("")
    lines.extend(_emit_bucket("BODY", BODY_PX, body_data_parts, body_offsets, glyph_infos))
    lines.append("")
    lines.append("/// Slot 0 is the notdef (1x1 zero-coverage placeholder).")
    lines.append(f"pub const HERO_NOTDEF: Glyph = Glyph {{ offset: 0, w: 1, h: 1, bearing_x: 0, bearing_y: 0, advance: 1 }};")
    lines.append(f"pub const BODY_NOTDEF: Glyph = Glyph {{ offset: 0, w: 1, h: 1, bearing_x: 0, bearing_y: 0, advance: 1 }};")
    lines.append("")
    lines.append(f"/// Hero-bucket glyph table ({len(glyph_infos)} glyphs).")
    lines.append(f"pub const HERO_TABLE: [Glyph; {len(glyph_infos) + 1}] = [")
    lines.append("    HERO_NOTDEF,  // 0 notdef")
    for info, hero_off in zip(glyph_infos, hero_offsets):
        ch = _repr_ch(info["cp"])
        lines.append(
            f"    Glyph {{ offset: {hero_off}, w: {info['hero_w']}, h: {info['hero_h']}, "
            f"bearing_x: {info['hero_bx']}, bearing_y: {info['hero_by']}, advance: {info['hero_adv']} }}, "
            f"// U+{info['cp']:04X} {ch}"
        )
    lines.append("];")
    lines.append("")
    lines.append(f"/// Body-bucket glyph table ({len(glyph_infos)} glyphs).")
    lines.append(f"pub const BODY_TABLE: [Glyph; {len(glyph_infos) + 1}] = [")
    lines.append("    BODY_NOTDEF,  // 0 notdef")
    for info, body_off in zip(glyph_infos, body_offsets):
        ch = _repr_ch(info["cp"])
        lines.append(
            f"    Glyph {{ offset: {body_off}, w: {info['body_w']}, h: {info['body_h']}, "
            f"bearing_x: {info['body_bx']}, bearing_y: {info['body_by']}, advance: {info['body_adv']} }}, "
            f"// U+{info['cp']:04X} {ch}"
        )
    lines.append("];")
    lines.append("")
    lines.append(f"/// Codepoint → glyph slot ({len(index_rows)} entries, binary-search).")
    lines.append(f"pub const FONT_INDEX: [IndexEntry; {len(index_rows)}] = [")
    for cp, slot in index_rows:
        lines.append(f"    IndexEntry {{ cp: 0x{cp:04X}, slot: {slot} }}, // {_repr_ch(cp)}")
    lines.append("];")
    lines.append("")
    lines.append(f"pub const HERO_EM_PX: u16 = {HERO_PX};")
    lines.append(f"pub const BODY_EM_PX: u16 = {BODY_PX};")
    lines.append(f"pub const GLYPH_COUNT: usize = {len(glyph_infos)};")
    lines.append("")
    return "\n".join(lines)


def main() -> None:
    cps = collect_codepoints()
    print(f"[build_font] {len(cps)} unique codepoints", file=sys.stderr)
    src = emit_rust(cps)
    OUT.write_text(src, encoding="utf-8")
    size_kb = OUT.stat().st_size / 1024
    print(f"[build_font] wrote {OUT} ({size_kb:.1f} KiB)", file=sys.stderr)


if __name__ == "__main__":
    main()