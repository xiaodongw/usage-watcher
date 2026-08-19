#!/usr/bin/env bash
# Install the extension for the current user and compile its settings schema.
#
# No packaging step: a GNOME extension is a directory, and this copies it into
# the place the shell looks. Run it again to update.
set -euo pipefail

UUID="usage-watcher@usagewatcher.dev"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"

command -v glib-compile-schemas >/dev/null || {
  echo "glib-compile-schemas not found — install libglib2.0-dev-bin (Debian/Ubuntu)" >&2
  echo "or glib2-devel (Fedora), then run this again." >&2
  exit 1
}

echo "Installing to $DEST"
mkdir -p "$DEST"
# Explicit list rather than `cp -r .`: this directory also holds install.sh and
# a README, and shipping those into the shell's extension dir is just litter.
for f in metadata.json extension.js prefs.js daemon.js format.js stylesheet.css; do
  cp "$SRC/$f" "$DEST/$f"
done
mkdir -p "$DEST/schemas"
cp "$SRC/schemas/"*.gschema.xml "$DEST/schemas/"
glib-compile-schemas "$DEST/schemas"

echo
echo "Installed. Now:"
echo
if [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
  # Wayland cannot restart the shell in place, and telling someone to press
  # Alt+F2 r on Wayland — where it silently does nothing — wastes an hour.
  echo "  1. Log out and back in (Wayland cannot reload the shell in place)."
else
  echo "  1. Press Alt+F2, type 'r', press Enter to restart the shell."
fi
echo "  2. gnome-extensions enable $UUID"
echo "  3. gnome-extensions prefs $UUID   # if uwd is not on 127.0.0.1:7878"
echo
echo "Logs:  journalctl -f -o cat /usr/bin/gnome-shell"
