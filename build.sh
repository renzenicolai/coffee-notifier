#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

WINDOWS_TARGET="x86_64-pc-windows-gnu"

echo "==> Building for Linux (release)"
cargo build --release

echo "==> Building for Windows (release, target: $WINDOWS_TARGET)"
rustup target add "$WINDOWS_TARGET" >/dev/null 2>&1 || true
cargo build --release --target "$WINDOWS_TARGET"

echo
echo "Build complete:"
echo "  Linux:   target/release/dcn-notify"
echo "  Windows: target/$WINDOWS_TARGET/release/dcn-notify.exe"
