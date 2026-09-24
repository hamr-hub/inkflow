#!/usr/bin/env bash
# dedicated zero-dep rewrite driver (single long Claude session, may iterate many turns)
set -uo pipefail
cd "$HOME/codespace/inkflow"
export PATH="$HOME/.nvm/versions/node/v22.22.1/bin:$PATH"
export CARGO_TARGET_DIR=/mnt/ssd/codespace/.cargo-target/inkflow-zero
mkdir -p state
exec > >(tee -a state/zero-rewrite.log) 2>&1
echo "===== zero-dep rewrite start $(date -Is) ====="

# bail if memory is too tight to run rustc
AVAIL_KB=$(awk '/MemAvailable/{print $2}' /proc/meminfo)
echo "MemAvailable=${AVAIL_KB}KB"

cat > state/zero_prompt.txt <<'EOF'
You are doing a full ZERO-DEPENDENCY rewrite of "inkflow". Read these in order:
ZERO_DEP.md (the binding spec), PRODUCTION.md, then current src/ for product semantics.

Requirements, non-negotiable:
- std-only Rust, zero third-party crates (justify in commit if you believe one crate
  is unavoidable — prefer writing raw ioctl/extern "C" yourself).
- No X11/Wayland/desktop: render directly via DRM/KMS dumb-buffer on /dev/dri/card*,
  software 32bpp BGRA into mmap.
- evdev touch via raw open + input_event + EVIOC* ioctl + MT slots + hotplug.
- ollama over hand-written std::net TCP + minimal HTTP/1.1 + minimal JSON parse;
  silent local-pool fallback on any failure/timeout.
- Embed a subset CJK bitmap font generated at build time (scripts) from Noto Sans CJK
  for the ~ambient glyph set + ASCII; software rasterize.
- Fixed reused arrays, no per-frame allocation; keep calm ambience; never black.

Work autonomously, as many turns as needed. After each meaningful step:
  cargo fmt && cargo clippy --release -- -D warnings && cargo build --release
Commit each green step: git commit -m "zero: <step>". Do NOT push. Do not touch
systemd units except providing scripts/install-systemd.sh.

Final definition of done (prove with real evidence, write into PRODUCTION.md):
binary runs on real DRM, state/screen.png shows CJK glyphs + particles, measured
binary size / RSS / fps; works with no X; install-systemd.sh ready.
If memory on this Jetson is too tight to run rustc at some point, pause and note it;
retry rather than forcing an OOM. Report a concise final summary.
EOF

timeout 3600 claude --dangerously-skip-permissions --print < state/zero_prompt.txt
echo "===== zero-dep rewrite end rc=$? $(date -Is) ====="
