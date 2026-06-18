#!/bin/zsh
# Rebuild Foreman only if its source changed, then launch it.
# Invoked by the Desktop "Foreman" launcher. GUI launches get a minimal
# environment, so PATH is set explicitly (node lives under nvm here).
set -uo pipefail

PROJECT="/Users/kairussenberger/Developer/foreman"
APP="/Applications/Foreman.app"
BUILT="$PROJECT/src-tauri/target/release/bundle/macos/Foreman.app"

NVM_BIN=$(ls -d "$HOME"/.nvm/versions/node/*/bin 2>/dev/null | sort -V | tail -1)
export PATH="${NVM_BIN}:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

cd "$PROJECT" || exit 1

# Installed binary (named after the cargo package — lowercase).
installed_bin() { ls "$APP"/Contents/MacOS/* 2>/dev/null | head -1; }

needs_build() {
  local b; b="$(installed_bin)"
  [ -z "$b" ] && return 0
  local newer
  newer=$(find src src-tauri/src src-tauri/templates src-tauri/icons index.html \
               package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json \
               src-tauri/capabilities -type f -newer "$b" 2>/dev/null | head -1)
  [ -n "$newer" ]
}

if needs_build; then
  echo "Foreman: source changed — rebuilding…"
  npm run tauri build -- --bundles app || { echo "build failed" >&2; exit 1; }
  rm -rf "$APP"
  cp -R "$BUILT" "$APP"
  xattr -dr com.apple.quarantine "$APP" 2>/dev/null || true
else
  echo "Foreman: up to date."
fi

open "$APP"
