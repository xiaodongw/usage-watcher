#!/usr/bin/env bash
# Build a portable release: unzip, double-click, done.
#
# Deliberately not an installer. An installer is the right answer eventually,
# but it is also a code-signing certificate, a Gatekeeper story and a SmartScreen
# reputation problem — none of which make the app work better on the machine of
# the person who wrote it. A zip works today, on every platform, with no
# elevation prompt and nothing left behind when it is deleted.
#
# The zip holds three executables and needs none of them installed:
#
#   usage-watcher   the app: tray icon, panel, and the collector inside it
#   uw              one-shot read from a terminal
#   uwd             the collector on its own, for a headless box or WSL
#
# Usage: scripts/package.sh [output-dir]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${1:-$root/dist}"
version="$(grep -m1 '^version' "$root/Cargo.toml" | cut -d'"' -f2)"

case "$(uname -s)" in
  Darwin) platform="macos-$(uname -m)" ; exe="" ;;
  Linux)  platform="linux-$(uname -m)" ; exe="" ;;
  *)      platform="windows-x86_64"    ; exe=".exe" ;;
esac

stage="$out/usage-watcher-$version-$platform"
rm -rf "$stage"
mkdir -p "$stage"

echo "==> building the panel"
npm --prefix "$root/widget" ci
npm --prefix "$root/widget" run build

echo "==> building the app"
# --no-bundle: we want the bare executable, not an installer around it.
npm --prefix "$root/widget" exec -- tauri build --no-bundle

echo "==> building the command line"
cargo build --release --manifest-path "$root/Cargo.toml" -p uw-cli -p uwd

cp "$root/widget/src-tauri/target/release/usage-watcher$exe" "$stage/"
cp "$root/target/release/uw$exe" "$stage/"
cp "$root/target/release/uwd$exe" "$stage/"
cp "$root/LICENSE" "$stage/" 2>/dev/null || true

cat > "$stage/README.txt" <<'TXT'
Usage Watcher
=============

Double-click `usage-watcher`. It puts an icon in the tray (Windows) or the
menu bar (macOS); click that to open the panel.

The first screen is empty, with an "Add provider" button. Adding a provider
walks you through signing in to it — for most that means a browser window, and
the app waits for you to come back.

Nothing else needs installing and nothing needs to be running first: the
collector runs inside the app.

Also in this folder, for anyone who wants them:

  uw     read your usage in a terminal:  uw
  uwd    run the collector on its own, e.g. on a headless box or inside WSL,
         with the panel on another machine pointed at it

Your settings live in:
  Windows  %APPDATA%\usage-watcher\config.toml
  macOS    ~/Library/Application Support/usage-watcher/config.toml
  Linux    ~/.config/usage-watcher/config.toml

Credentials do not. They go to the Windows Credential Manager, the macOS
Keychain, or an owner-only file on Linux — never into the config file.

To remove it: quit from the tray menu and delete this folder. `uw provider
remove <name>` first if you also want the stored credentials gone.
TXT

echo "==> zipping"
( cd "$out" && rm -f "$(basename "$stage").zip" && zip -qr "$(basename "$stage").zip" "$(basename "$stage")" )
echo "$out/$(basename "$stage").zip"
