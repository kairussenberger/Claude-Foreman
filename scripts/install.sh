#!/bin/zsh
# Build Foreman and install it to /Applications. Run from anywhere:
#   ./scripts/install.sh
set -euo pipefail

PROJECT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT"

command -v npm >/dev/null   || { echo "✗ Node.js / npm not found — install Node 18+ (nodejs.org)"; exit 1; }
command -v cargo >/dev/null || { echo "✗ Rust not found — install via https://rustup.rs"; exit 1; }
command -v claude >/dev/null || echo "⚠ 'claude' CLI not on PATH — install & log in to Claude Code before running Foreman."

echo "▸ Installing dependencies…"
npm install

echo "▸ Building Foreman (first build compiles Rust — a few minutes)…"
npm run tauri build -- --bundles app

APP="$PROJECT/src-tauri/target/release/bundle/macos/Foreman.app"
[ -d "$APP" ] || { echo "✗ build did not produce $APP" >&2; exit 1; }

echo "▸ Installing to /Applications…"
rm -rf "/Applications/Foreman.app"
cp -R "$APP" "/Applications/Foreman.app"
xattr -dr com.apple.quarantine "/Applications/Foreman.app" 2>/dev/null || true

echo ""
echo "✓ Installed: /Applications/Foreman.app"
echo "  Open it from Spotlight / Launchpad, or:  open /Applications/Foreman.app"
echo "  Optional self-updating Desktop launcher:  ./scripts/install-launcher.sh"
