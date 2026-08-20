#!/usr/bin/env bash
# Scaffold and run the Android / iOS targets.
#
# `tauri <platform> init` generates a Gradle or Xcode project under
# src-tauri/gen/, which is why that step is a script rather than something
# committed: the generated project pins toolchain paths and SDK versions from
# the machine that ran it, and checking one in guarantees it is wrong on the
# next machine.
#
#   ./mobile.sh android init | dev | build
#   ./mobile.sh ios     init | dev | build
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

PLATFORM="${1:-}"
ACTION="${2:-dev}"

usage() { echo "usage: ./mobile.sh {android|ios} {init|dev|build}" >&2; exit 2; }
[ -n "$PLATFORM" ] || usage

fail() { echo "error: $*" >&2; exit 1; }

case "$PLATFORM" in
  android)
    # Checked up front and all together: finding out about the NDK ten minutes
    # into a Gradle sync is a bad way to spend an evening.
    [ -n "${JAVA_HOME:-}" ]    || fail "JAVA_HOME is not set (JDK 17+ needed)"
    [ -n "${ANDROID_HOME:-}" ] || fail "ANDROID_HOME is not set — install the Android SDK"
    [ -n "${NDK_HOME:-}" ]     || fail "NDK_HOME is not set — install the NDK via the SDK manager"

    # Tauri cross-compiles to four ABIs; without the targets, cargo fails per
    # architecture with an error that does not mention rustup.
    for t in aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android; do
      rustup target list --installed | grep -qx "$t" || {
        echo "adding rust target $t"
        rustup target add "$t"
      }
    done
    ;;
  ios)
    [ "$(uname -s)" = "Darwin" ] || fail "iOS can only be built on macOS — Xcode is required and does not exist elsewhere"
    # The Command Line Tools alone are enough for the desktop build but not for
    # this one: `tauri ios` drives a real Xcode project.
    command -v xcodebuild >/dev/null || fail "xcodebuild not found — install the full Xcode, not just the Command Line Tools"
    # Tauri generates a Podfile, so a missing pod only shows up as a confusing
    # failure part-way through the first build.
    command -v pod >/dev/null || fail "cocoapods not found — brew install cocoapods"
    # x86_64 is the Intel simulator, still needed on an Intel Mac.
    for t in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
      rustup target list --installed | grep -qx "$t" || {
        echo "adding rust target $t"
        rustup target add "$t"
      }
    done
    ;;
  *) usage ;;
esac

[ -d node_modules ] || npm install

case "$ACTION" in
  init)
    npm run tauri -- "$PLATFORM" init
    echo
    echo "Generated src-tauri/gen/$PLATFORM. It is gitignored on purpose — re-run"
    echo "this on any machine that needs it."
    echo
    echo "Next: ./mobile.sh $PLATFORM dev   (with a device connected or an emulator running)"
    ;;
  dev|build)
    [ -d "src-tauri/gen/$PLATFORM" ] || fail "run './mobile.sh $PLATFORM init' first"
    npm run tauri -- "$PLATFORM" "$ACTION"
    ;;
  *) usage ;;
esac
