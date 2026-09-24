#!/usr/bin/env bash
# dedicated visual polish session: beautify, verify with real dumb-buffer captures
set -uo pipefail
cd "$HOME/codespace/inkflow"
export PATH="$HOME/.nvm/versions/node/v22.22.1/bin:$PATH"
export CARGO_TARGET_DIR=/mnt/ssd/codespace/.cargo-target/inkflow-zero
mkdir -p state
exec > >(tee -a state/beautify.log) 2>&1
echo "===== beautify start $(date -Is) ====="

cat > state/beautify_prompt.txt <<'EOF'
You are the visual designer for "inkflow", a zero-dep Rust ambient art object (DRM/KMS
software rendering, embedded CJK bitmap font, 1280x720+ output). No display is attached,
but you can render to a dumb-buffer surface and capture PNGs to judge your work.

MISSION: make it genuinely beautiful — a refined gallery-grade object, not a tech demo.
Improve the visual language in concrete ways, e.g.:
- composition: glyph scale hierarchy & depth (parallax layers), breathing margins,
  intentional focal drift, golden-ratio placements instead of uniform scatter
- color: a coherent, restrained palette; soft warm/cool gradients, glows, additive
  bokeh; smooth vignette and depth fog; tasteful hue tied to touch warmth
- typography: smoother bitmap glyph rasterization/anti-aliasing, subtle per-glyph
  brightness, elegant fade in/out curves, line-breath spacing, occasional word pairing
- motion: buttery easing, gentle currents/vortices, particles that respond like ink in
  water; no linear/mechanical motion
- atmosphere: fine grain, distant stars/dust layered by depth, light bloom

PROCESS (iterate visibly):
1. Add/ensure a headless render harness that renders the dumb-buffer surface to
   state/beauty-N.png so you can SEE results; use the SAME production render path.
   Capture several frames over time (motion) not just one.
2. Make ONE coherent set of visual changes, then capture and self-critique against the
   PNGs; repeat 3-5 rounds until visibly refined. Look at the screenshots each time.
3. Each green step: cargo fmt && cargo clippy --release -- -D warnings && cargo build
   --release, then git commit -m "beauty: <change>". Do NOT push.

Constraints: std-only / zero crates; calm ambience, no HUD/menus/debug; must still work
with no ollama (local glyph pool) and fixed-size no-per-frame-alloc discipline; tight RAM
(~0.5-0.9GB) — wait/retry rather than force OOM; do not disturb the running user
inkflow.service or systemd. Keep --drm-test working.

End with a short summary of the visual changes and point to the final state/beauty-*.png.
EOF

timeout 3000 claude --dangerously-skip-permissions --print < state/beautify_prompt.txt
echo "===== beautify end rc=$? $(date -Is) ====="
