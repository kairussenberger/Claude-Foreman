#!/bin/zsh
# (Re)create the Desktop "Foreman" launcher — an app that rebuilds-if-needed
# then launches Foreman.app. Idempotent; safe to re-run after icon changes.
set -uo pipefail

PROJECT="/Users/kairussenberger/Developer/foreman"
LAUNCHER="/Applications/Foreman Launcher.app"
ICON="$PROJECT/src-tauri/icons/icon.icns"
DESKTOP="$HOME/Desktop"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"

# Compile the AppleScript launcher into an app bundle.
rm -rf "$LAUNCHER"
/usr/bin/osacompile -o "$LAUNCHER" "$PROJECT/scripts/Foreman.applescript"

# Give it the Foreman icon (osacompile applets read Contents/Resources/applet.icns).
cp "$ICON" "$LAUNCHER/Contents/Resources/applet.icns"
touch "$LAUNCHER"
[ -x "$LSREGISTER" ] && "$LSREGISTER" -f "$LAUNCHER" >/dev/null 2>&1 || true

# Desktop alias named "Foreman" pointing at the launcher (alias shows its icon).
rm -f "$DESKTOP/Foreman" "$DESKTOP/Foreman.app" 2>/dev/null
osascript -e "tell application \"Finder\" to make alias file to (POSIX file \"$LAUNCHER\") at (POSIX file \"$DESKTOP\") with properties {name:\"Foreman\"}" >/dev/null 2>&1 \
  || ln -s "$LAUNCHER" "$DESKTOP/Foreman"

echo "Launcher: $LAUNCHER"
echo "Desktop:  $DESKTOP/Foreman"
