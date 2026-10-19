#!/usr/bin/env bash
# Build and install the lyaeus CLAP + VST3 for native Linux (per-user, no sudo).
#
# Usage:
#   ./build-linux.sh            # test, build, install to ~/.clap and ~/.vst3
#   ./build-linux.sh --check    # test and build only (no install)
#
# Host: Bitwig Studio on native Linux (prefer CLAP). After installing, RESTART
# the DAW: a loaded .so stays in memory, so Bitwig keeps the old build.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "==> cargo test"
cargo test --release --quiet

echo "==> Bundling lyaeus (release)..."
cargo xtask bundle lyaeus --release

VST3_SRC="target/bundled/lyaeus.vst3"
CLAP_SRC="target/bundled/lyaeus.clap"

if [ ! -d "$VST3_SRC" ] || [ ! -f "$CLAP_SRC" ]; then
    echo "ERROR: build output missing (expected $VST3_SRC and $CLAP_SRC)" >&2
    exit 1
fi

if [ "${1:-}" = "--check" ]; then
    echo "==> Check only, skipping install."
    exit 0
fi

VST3_DEST="$HOME/.vst3"
CLAP_DEST="$HOME/.clap"
mkdir -p "$VST3_DEST" "$CLAP_DEST"

rm -rf "$VST3_DEST/lyaeus.vst3"
cp -r "$VST3_SRC" "$VST3_DEST/"
cp -f "$CLAP_SRC" "$CLAP_DEST/"

echo ""
echo "=== INSTALLED ==="
echo "  CLAP -> $CLAP_DEST/lyaeus.clap"
echo "  VST3 -> $VST3_DEST/lyaeus.vst3"
echo ""
echo "Next: restart Bitwig. Chain: bromios -> lyaeus -> a cabinet IR"
echo "(Bitwig Convolution device) after it."
