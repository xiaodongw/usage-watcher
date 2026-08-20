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
# Usage: scripts/package.sh [--container] [output-dir]
#
#   --container   build the Linux zip inside Ubuntu 22.04 rather than natively.
#                 Only the build host decides which machines can run the result
#                 — see scripts/Dockerfile.linux — so a release build wants this
#                 and a local one does not.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

use_container=""
if [ "${1:-}" = "--container" ]; then
  use_container=1
  shift
fi
out="${1:-$root/dist}"
version="$(grep -m1 '^version' "$root/Cargo.toml" | cut -d'"' -f2)"

case "$(uname -s)" in
  Darwin) platform="macos-$(uname -m)" ; exe="" ;;
  Linux)  platform="linux-$(uname -m)" ; exe="" ;;
  *)      platform="windows-x86_64"    ; exe=".exe" ;;
esac

# Everything the Linux app links against, and the package that supplies it.
#
# Checked up front, all of it, before a single crate is compiled. The build
# itself reports these one at a time, ten minutes apart, and always names the
# pkg-config module rather than the package — so the natural way to satisfy it
# is to install the leaf that was named and start again. That loop does not
# converge quickly: `atk`, `pango`, `libsoup-3.0` and `javascriptcoregtk-4.1`
# are all dependencies of libgtk-3-dev and libwebkit2gtk-4.1-dev, so installing
# them one at a time is walking up the tree from the leaves while the two
# packages that would have brought the whole set stay missing.
linux_deps() {
  cat <<'DEPS'
webkit2gtk-4.1            libwebkit2gtk-4.1-dev
javascriptcoregtk-4.1     libwebkit2gtk-4.1-dev
libsoup-3.0               libwebkit2gtk-4.1-dev
gtk+-3.0                  libgtk-3-dev
gdk-3.0                   libgtk-3-dev
gdk-x11-3.0               libgtk-3-dev
gdk-wayland-3.0           libgtk-3-dev
gdk-pixbuf-2.0            libgtk-3-dev
cairo                     libgtk-3-dev
pango                     libgtk-3-dev
atk                       libgtk-3-dev
glib-2.0                  libglib2.0-dev
gobject-2.0               libglib2.0-dev
gio-2.0                   libglib2.0-dev
ayatana-appindicator3-0.1 libayatana-appindicator3-dev
dbus-1                    libdbus-1-dev
DEPS
}

preflight() {
  local missing=() modules=() pkg module tool
  for tool in cargo npm pkg-config zip; do
    command -v "$tool" >/dev/null || missing+=("$tool")
  done
  if [ ${#missing[@]} -gt 0 ]; then
    echo "not on PATH: ${missing[*]}" >&2
    exit 1
  fi

  [ "$(uname -s)" = "Linux" ] || return 0

  while read -r module pkg; do
    [ -n "$module" ] || continue
    pkg-config --exists "$module" 2>/dev/null && continue
    modules+=("$module")
    case " ${missing[*]} " in *" $pkg "*) ;; *) missing+=("$pkg") ;; esac
  done < <(linux_deps)

  [ ${#missing[@]} -eq 0 ] && return 0

  {
    echo "the Tauri app cannot be built here yet."
    echo
    echo "missing pkg-config modules: ${modules[*]}"
    echo
    echo "on Debian or Ubuntu, all of it at once:"
    echo
    echo "  sudo apt install ${missing[*]}"
    echo
    echo "Fedora, Arch and the reasoning are in docs/BUILDING.md."
    echo
    echo "The CLI and the daemon need none of this:"
    echo
    echo "  cargo build --release -p uw-cli -p uwd"
  } >&2
  exit 1
}

# Build in a container instead, and stop: everything below runs inside it.
#
# The caches are bind-mounted to their own directories rather than shared with
# the native build. Two toolchains against two different glibcs would otherwise
# invalidate each other's `target/` on every alternation, and each rebuild from
# cold costs minutes.
if [ -n "$use_container" ]; then
  [ "$(uname -s)" = "Linux" ] || {
    echo "--container builds the Linux zip; run it on Linux (or in WSL)." >&2
    exit 1
  }
  command -v docker >/dev/null || { echo "docker is not on PATH" >&2; exit 1; }

  image="usage-watcher-build:jammy"
  cache="$root/.container-cache"
  mkdir -p "$cache"/{cargo,npm,home,target,src-tauri-target,node_modules} "$out"

  echo "==> building the image"
  docker build -q -f "$root/scripts/Dockerfile.linux" -t "$image" "$root/scripts" >/dev/null

  echo "==> building in $image"
  # As the invoking user, so that everything written into the checkout — the
  # zip, and any file a build step touches — belongs to them and not to root.
  exec docker run --rm \
    --user "$(id -u):$(id -g)" \
    -e HOME=/cache/home \
    -e CARGO_HOME=/cache/cargo \
    -e npm_config_cache=/cache/npm \
    -v "$root:/work" \
    -v "$cache:/cache" \
    -v "$cache/target:/work/target" \
    -v "$cache/src-tauri-target:/work/widget/src-tauri/target" \
    -v "$cache/node_modules:/work/widget/node_modules" \
    -w /work \
    "$image" \
    scripts/package.sh "${out#$root/}"
fi

preflight

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
