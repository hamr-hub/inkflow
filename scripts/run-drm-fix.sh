#!/usr/bin/env bash
# fix DRM on the REAL device: iterate until modeset works on /dev/dri/card0
set -uo pipefail
cd "$HOME/codespace/inkflow"
export PATH="$HOME/.nvm/versions/node/v22.22.1/bin:$PATH"
export CARGO_TARGET_DIR=/mnt/ssd/codespace/.cargo-target/inkflow-zero
mkdir -p state
exec > >(tee -a state/drm-fix.log) 2>&1
echo "===== drm real-device fix start $(date -Is) ====="

cat > state/drm_prompt.txt <<'EOF'
The zero-dep rewrite (commits 5fc30e3..abc6c6d) compiles but DRM was never tested on
real hardware — it only ran headless. THIS machine HAS real DRM. Prove and fix it.

Observed real failure right now:
  inkflow --diag opens /dev/dri/card0 (fd=3) but prints
  drm=err:DRM_IOCTL_VERSION failed: 22   (EINVAL)
Root-cause hint to verify: src/drm.rs hardcodes DRM_IOCTL_VERSION=0xc010_6464 with a
16-byte size, but the real UAPI `struct drm_version` is 64-byte on aarch64
(3x c_int, then 3x { size_t _len; char* ptr }). Compute ioctl numbers correctly for
aarch64 from the actual struct sizes (use _IOWR('d', nr, T) with mem::size_of::<T>()),
don't trust the hand-typed constants. Audit EVERY drm ioctl constant and struct layout
(getresources, getconnector, getencoder, getcrtc, setcrtc, addfb, create/map/destroy
dumb, mode_setcrtc struct drm_mode_crtc, drm_mode_modeinfo, connector/encoder structs).
Cross-check against kernel headers on this box (find them; or infer carefully).

Iterate this loop until it genuinely works, verifying on the real card each time:
  edit -> cargo build --release -> ./target/release/inkflow --diag
First make DRM_IOCTL_VERSION return real driver name/version. Then implement/verify a
full modeset path: get resources -> pick connected connector + encoder + mode ->
create dumb buffer -> mmap -> addfb (DRM_FORMAT_XRGB8888) -> setcrtc, render one frame,
and keep a runnable mode (a flag like --drm-test that modesets, draws an obvious test
pattern + a few CJK glyphs, holds 5s, then restores crtc and exits). Capture what is on
the dumb buffer into state/screen.png as evidence.

Constraints remain: std-only, zero crates; do not disturb the running user inkflow.service
or systemd; beware tight RAM (MemAvailable ~0.6GB — if rustc risks OOM, wait/retry).
Commit green steps "zerofix: <step>". Final report: exact ioctl numbers, real connector/
mode, and confirm modeset succeeded. Do not run the persistent DRM loop (would steal the
screen); just prove the path with --drm-test.
EOF

timeout 3000 claude --dangerously-skip-permissions --print < state/drm_prompt.txt
echo "===== drm fix end rc=$? $(date -Is) ====="
