#!/usr/bin/env bash
# inkflow · scripts/install-systemd.sh
#
# One-shot install for the zero-dep DRM/KMS build.
#
# Steps:
#   1. Build the release binary into the same CARGO_TARGET_DIR the dev
#      loop writes to, OR reuse an already-built binary if it exists.
#   2. Copy the binary to /usr/local/bin/inkflow (root required).
#   3. Copy scripts/inkflow-zero.service into /etc/systemd/system/.
#   4. `systemctl daemon-reload && enable --now inkflow-zero.service`.
#   5. Print a brief status so the operator sees the binary is up.
#
# Idempotent: re-running after a rebuild simply reinstalls the binary and
# triggers a restart. We do NOT touch any existing inkflow.service or
# inkflow-autoloop.{service,timer} (those are the user-mode autoloop pair
# the dev loop maintains; this script only adds the system-mode service
# for the boot-to-piece story).

set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

if [[ $EUID -ne 0 ]]; then
    echo "This installer must run as root (it copies files into /usr/local and /etc/systemd)."
    echo "Re-run with: sudo $0"
    exit 1
fi

# ---------- 1. ensure a built binary ----------
BIN_SRC="${CARGO_TARGET_DIR:-/mnt/ssd/codespace/.cargo-target/inkflow-zero}/release/inkflow"
if [[ ! -x "$BIN_SRC" ]]; then
    echo "Building release binary into $BIN_SRC ..."
    (cd "$ROOT" && CARGO_TARGET_DIR="$(dirname "$BIN_SRC")" cargo build --release)
fi
test -x "$BIN_SRC" || { echo "no binary at $BIN_SRC after build"; exit 1; }

# ---------- 2. install binary ----------
install -m 0755 "$BIN_SRC" /usr/local/bin/inkflow
echo "installed binary -> /usr/local/bin/inkflow ($(stat -c%s /usr/local/bin/inkflow) bytes)"

# ---------- 3. install unit ----------
install -m 0644 "$HERE/inkflow-zero.service" /etc/systemd/system/inkflow-zero.service
echo "installed unit   -> /etc/systemd/system/inkflow-zero.service"

# ---------- 4. enable + start ----------
systemctl daemon-reload
systemctl enable inkflow-zero.service
systemctl restart inkflow-zero.service

# ---------- 5. status ----------
echo
echo "----- systemctl status inkflow-zero.service -----"
systemctl --no-pager --full status inkflow-zero.service | head -20 || true
echo
echo "Done. The piece will be on screen after every boot."
echo "Logs:   journalctl -u inkflow-zero.service -f"
echo "         /var/log/inkflow.log (stdout/stderr tee)"
echo "Stop:   sudo systemctl stop inkflow-zero.service"