#!/bin/zsh
# (Optional) Create a Desktop "Foreman" launcher that rebuilds-if-changed, then
# launches Foreman.app. Portable — all paths derive from this repo's location.
set -uo pipefail

PROJECT="$(cd "$(dirname "$0")/.." && pwd)"
LAUNCHER="/Applications/Foreman Launcher.app"
ICON="$PROJECT/src-tauri/icons/icon.icns"
DESKTOP="$HOME/Desktop"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"

# Generate the AppleScript launcher with this repo's launch.sh path baked in.
TMP="$(mktemp -t foreman-launcher).applescript"
cat > "$TMP" <<EOF
with timeout of 1800 seconds
    try
        display notification "Checking for updates…" with title "Foreman"
        do shell script "/bin/zsh " & quoted form of "$PROJECT/scripts/launch.sh"
    on error errMsg number errNum
        display dialog "Foreman couldn't build:" & return & return & errMsg buttons {"OK"} default button "OK" with icon stop with title "Foreman"
    end try
end timeout
EOF

rm -rf "$LAUNCHER"
/usr/bin/osacompile -o "$LAUNCHER" "$TMP"
rm -f "$TMP"

# Give it the Foreman icon.
cp "$ICON" "$LAUNCHER/Contents/Resources/applet.icns"
touch "$LAUNCHER"
[ -x "$LSREGISTER" ] && "$LSREGISTER" -f "$LAUNCHER" >/dev/null 2>&1 || true

# Desktop alias named "Foreman" pointing at the launcher (shows its icon).
rm -f "$DESKTOP/Foreman" "$DESKTOP/Foreman.app" 2>/dev/null
osascript -e "tell application \"Finder\" to make alias file to (POSIX file \"$LAUNCHER\") at (POSIX file \"$DESKTOP\") with properties {name:\"Foreman\"}" >/dev/null 2>&1 \
  || ln -s "$LAUNCHER" "$DESKTOP/Foreman"

echo "Launcher: $LAUNCHER"
echo "Desktop:  $DESKTOP/Foreman"
