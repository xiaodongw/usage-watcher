# Building

Seven things can be built out of this repo, and no single machine can build all
of them. This is what each one needs, and where it has to be built.

- [What builds where](#what-builds-where)
- [Common to everything](#common-to-everything)
- [Linux and WSL](#linux-and-wsl)
- [Windows](#windows)
- [macOS](#macos)
- [Android](#android)
- [iOS](#ios)
- [GNOME extension](#gnome-extension)
- [Portable zips](#portable-zips)
- [Release builds](#release-builds)
- [Troubleshooting](#troubleshooting)

## What builds where

| target | Linux / WSL | Windows | macOS | needs |
|---|:---:|:---:|:---:|---|
| `uw` (CLI) | ✅ | ✅ | ✅ | Rust only |
| `uwd` (daemon) | ✅ | ✅ | ✅ | Rust only |
| widget in a browser | ✅ | ✅ | ✅ | Node only |
| Tauri desktop app | ⚠️ | ✅ | ✅ | platform webview toolchain |
| Android app | ✅ | ✅ | ✅ | JDK + Android SDK + NDK |
| iOS app | ❌ | ❌ | ✅ | Xcode — no cross-compiling, ever |
| GNOME extension | ✅ | ❌ | ❌ | `glib-compile-schemas`, and a GNOME session to run it |

⚠️ The Tauri app *does* build on Linux, but under WSLg the tray behaviour is not
representative of a real desktop. Linux users want the GNOME extension instead.

Two things follow from this table and are worth internalising before you start:

**The desktop app contains the collector.** `widget/src-tauri` links `uwd` as a
library, so building the app also builds the whole daemon and its dependency
tree — a first build is noticeably longer than it was, and needs nothing extra
beyond the platform webview toolchain. The mobile targets do not link it: a
phone stays a viewer.

**The daemon and the UI still do not have to be on the same machine.** `uwd`
belongs wherever the credentials and the vendor CLIs are — inside WSL, in the
usual setup — and every viewer reads it over HTTP. So you build `uwd` once in
WSL and the widget natively on Windows, leave the panel's address blank or point
it at the WSL one, and they talk over `localhost` because WSL2 forwards it. When
the app finds a collector already listening on the configured port it uses that
one instead of starting a second.

**`widget/src-tauri` is deliberately not a workspace member.** The root
`Cargo.toml` excludes it. That is what keeps `cargo test --workspace` green on a
machine with no webview toolchain, which is most of them. It also means
`cargo build` at the root never builds the app — you go through `npm run
app:build`, which invokes its own Cargo workspace.

## Common to everything

**Rust 1.89 or newer**, via [rustup](https://rustup.rs). The workspace pins
`rust-version = "1.89"`; anything newer is fine.

That number belongs to the dependency tree rather than to our own code, which
needs nothing past 1.82 — `zbus`, `zvariant` and `time-macros` all want 1.89,
and cargo refuses to resolve before compiling a line. It read 1.82 for months
without anyone noticing, because every machine here runs something far newer.
The containerised Linux build is what caught it, since that pins its compiler,
and it is the thing to re-check with after a `cargo update`.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # not Windows
rustc --version
```

**Node 20 LTS or newer**, for anything involving `widget/`. Vite 6 and
`vue-tsc` need it; nothing else in the repo does.

The CLI and the daemon need nothing else on any platform:

```sh
cargo build --release -p uw-cli -p uwd
cargo install --path crates/uw-cli     # puts `uw` on PATH
```

There is no C toolchain requirement hiding in there. TLS is `rustls`, not
OpenSSL, and the Linux credential store is a plain 0600 file rather than
`keyring` — see [Credential storage](#credential-storage) below for why that
matters to your package list.

### Credential storage

`uw-core` picks its store by target, and this is the one place where an
innocuous-looking dependency would change what you must install:

| target | store | build dependency |
|---|---|---|
| macOS, iOS | Keychain | none (`keyring` with `apple-native`) |
| Windows | Credential Manager | none (`keyring` with `windows-native`) |
| Linux, WSL, Android | 0600 file under `$XDG_DATA_HOME` | **none** |

Linux deliberately does *not* use `keyring`. The Secret Service backend needs
D-Bus and `libdbus-1-dev`, and WSL runs no Secret Service daemon at all — a
keychain-only design would simply fail to start there. So there is no
`libdbus-1-dev` in any package list below, and that is on purpose.

## Linux and WSL

### CLI and daemon

Nothing beyond Rust:

```sh
cargo build --release -p uw-cli -p uwd
```

### Widget in a browser

```sh
npm --prefix widget install
npm --prefix widget run dev          # http://localhost:5173
```

Under WSL this is reachable from a Windows browser with no configuration —
WSL2 forwards `localhost`. It is the fastest way to see a UI change, and it is
the same Vue app the native shells host.

### Tauri desktop app (possible, not recommended)

```sh
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
                 libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Fedora:

```sh
sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file \
                 libappindicator-gtk3-devel librsvg2-devel libxdo-devel
sudo dnf group install "c-development"
```

Arch:

```sh
sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file openssl \
                        appmenu-gtk-module libappindicator-gtk3 librsvg xdotool
```

Then:

```sh
npm --prefix widget run app:dev
```

`libwebkit2gtk-4.1-dev` is the one that actually matters; without it the build
fails at `pkg-config` before compiling anything. Check for it with:

```sh
pkg-config --exists webkit2gtk-4.1 && echo present || echo missing
```

On a real GNOME desktop, prefer the [extension](#gnome-extension). The tray this
app draws is a `libayatana-appindicator` item, which GNOME renders as a menu
with no live figure in the top bar — which is the point of the thing.

## Windows

This is the intended home for the tray widget. The daemon stays in WSL.

### Prerequisites

1. **The MSVC toolchain**, via the **Desktop development with C++** workload.
   Either source works and they install the same thing:
   - [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
     — the toolchain without the IDE, if you have neither.
   - **Visual Studio 2022** (any edition, including Community) with that
     workload ticked — already enough. Do not install the Build Tools as well.

   What Rust actually needs out of it is `link.exe` and a Windows SDK, both of
   which that workload provides. A non-default install location is fine; rustup
   finds it through `vswhere` rather than a fixed path.

   **Mobile development with C++** is *not* wanted. It is for C++
   cross-platform mobile projects and has nothing to do with the Android build
   here, which uses the Android SDK and NDK instead.
2. **[rustup](https://win.rustup.rs)**, and a toolchain whose host ends in
   **`-msvc`**. Check it:

   ```powershell
   rustc -vV        # host: x86_64-pc-windows-msvc
   ```

   `x86_64-pc-windows-gnu` will not build the app. `webview2-com-sys` links an
   MSVC-format import library, so a GNU host fails at link time no matter how
   complete the Visual Studio install is.

   The usual way to end up with the wrong one is **`choco install rust`**,
   which installs the GNU toolchain and no rustup at all. Replace it:

   ```powershell
   choco uninstall rust -y
   choco install rustup.install -y
   # then, in a NEW terminal so PATH is picked up:
   rustup default stable-x86_64-pc-windows-msvc
   rustc -vV
   where rustc      # ...\.cargo\bin\rustc.exe, not a chocolatey shim
   ```

   Prefer `rustup.install` over `choco install rust-ms`: the latter gives the
   right ABI but no rustup, and [Android](#android) needs
   `rustup target add` for four cross-compilation targets.
3. **[Node LTS](https://nodejs.org)** — the installer from the site, or, if you
   already have Chocolatey from the step above:

   ```powershell
   choco install nodejs-lts -y
   # then, in a NEW terminal so PATH is picked up:
   node --version
   npm --version
   ```

   `nodejs-lts`, not `nodejs`: the plain package tracks Current, which moves to
   a new major every six months. Either satisfies the version floor in [Common
   to everything](#common-to-everything), but only one of them stops changing
   under you. Pick one source and stay with
   it — a `choco install` over an existing nodejs.org install leaves two copies
   on `PATH`, and which `npm` you get then depends on the order.
4. **WebView2 runtime** — already present on Windows 11 and on Windows 10 since
   version 1803. Only needed on genuinely old installs.
5. **VBSCRIPT**, a Windows optional feature, if you want the `.msi` installer.
   Enabled by default; only worth knowing about if someone turned it off.

### Build and run

```sh
cargo run -p uwd            # in WSL, where the credentials are
```

```powershell
cd <your-clone>\widget
npm install
npm run app:dev             # or: npm run app:build
```

Nothing to configure: WSL2 forwards `localhost`, so a daemon bound to
`127.0.0.1:7878` inside WSL answers on `localhost:7878` from Windows. If that
stops working, see [Troubleshooting](#troubleshooting).

### Icon

The full icon set is committed — including `icons/icon.ico`, which
`tauri-build` **requires** on Windows and whose absence fails the build before
a line of Rust is compiled. Nothing to do unless you want a different picture.

The artwork is still a placeholder. To replace it, edit or replace
`src-tauri/app-icon.png` (1024×1024, transparent) and regenerate every size and
format Windows, macOS, Linux, Android and iOS each want:

```sh
cd widget
npm run tauri icon src-tauri/app-icon.png
```

That command needs no platform toolchain — it runs anywhere Node does, so the
whole set can be regenerated from Linux and committed for the platforms that
cannot build there.

## macOS

```sh
xcode-select --install                 # Command Line Tools are enough for this
brew install rustup node               # if you have neither
```

Full Xcode is **only** needed for [iOS](#ios). Desktop builds are happy with the
Command Line Tools. macOS 10.15 or later.

```sh
npm --prefix widget install
npm --prefix widget run app:dev        # or: app:build
```

It launches as a menu-bar accessory — no Dock icon, no app-switcher entry. That
is set in two places on purpose, and both are needed:

- `set_activation_policy(Accessory)` in `src-tauri/src/lib.rs` covers
  `tauri dev`, which does not bundle and so never reads a plist.
- `LSUIElement` in `src-tauri/Info.plist` covers the shipped `.app`, from its
  very first launch, before any Rust has run.

Universal binary for both Apple Silicon and Intel:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri build -- --target universal-apple-darwin
```

Unsigned builds are fine for yourself. Distributing to anyone else means an
Apple Developer account, a Developer ID certificate and notarisation, none of
which this repo sets up.

## Android

Buildable from Linux, macOS or Windows — the SDK is cross-platform. What is
*not* cross-platform is iOS, so a Mac is the only machine that can produce both.

### Prerequisites

1. **[Android Studio](https://developer.android.com/studio)**. Easiest source
   of all four pieces below; you never have to open the IDE.
2. In **SDK Manager**, install:
   - Android SDK Platform (a recent API level)
   - Android SDK Platform-Tools
   - Android SDK Build-Tools
   - Android SDK Command-line Tools
   - **NDK (Side by side)** — the one people forget, and the one whose absence
     surfaces ten minutes into a Gradle sync
3. **A JDK 17 or newer.** Android Studio bundles one (JetBrains Runtime); point
   `JAVA_HOME` at it or install your own.
4. **Environment variables.** Nothing works without these three:

   ```sh
   # ~/.zshrc or ~/.bashrc — adjust for your install
   export JAVA_HOME="/opt/android-studio/jbr"
   export ANDROID_HOME="$HOME/Android/Sdk"
   export NDK_HOME="$ANDROID_HOME/ndk/$(ls -1 "$ANDROID_HOME/ndk" | tail -1)"
   ```

   On Windows, set them in *System Properties → Environment Variables*;
   `ANDROID_HOME` is typically `%LOCALAPPDATA%\Android\Sdk`.

5. **Rust targets** — `mobile.sh` adds these for you, but for the record:

   ```sh
   rustup target add aarch64-linux-android armv7-linux-androideabi \
                     i686-linux-android x86_64-linux-android
   ```

### Build

```sh
cd widget
./mobile.sh android init      # once — generates src-tauri/gen/android
./mobile.sh android dev       # device attached, or an emulator running
./mobile.sh android build     # .apk / .aab
```

The script checks all of the above **before** starting, because a missing NDK
otherwise announces itself as a Gradle error with no mention of the NDK.

`mobile.sh` is bash, so on Windows run it from **Git Bash or WSL**, or skip it
and call the CLI directly from PowerShell — you then get none of the
preflight checks, which is exactly when the NDK error above turns up:

```powershell
npm run tauri android init
npm run tauri android dev
```

`src-tauri/gen/android` is gitignored on purpose: the generated Gradle project
records SDK versions and absolute toolchain paths from the machine that ran
`init`, so a committed copy is wrong everywhere else. Re-run `init` per machine.

For `dev` against a physical device, the phone loads the Vite dev server over
the network. Tauri sets `TAURI_DEV_HOST` and `vite.config.ts` already honours
it — the phone and the computer just have to be on the same network.

## iOS

**macOS only.** There is no cross-compiling this; Xcode does not exist for other
platforms and the toolchain cannot be reproduced.

### Prerequisites

1. **Full Xcode** from the App Store — not just the Command Line Tools, which
   are enough for the desktop build but cannot drive an Xcode project.
2. **Cocoapods**: `brew install cocoapods`. Tauri generates a `Podfile`; without
   `pod` the first build fails part-way through with an unhelpful message.
3. **Rust targets** (again, `mobile.sh` handles it):

   ```sh
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
   ```

   `aarch64-apple-ios-sim` is the simulator on Apple Silicon;
   `x86_64-apple-ios` is the simulator on an Intel Mac.

### Build

```sh
cd widget
./mobile.sh ios init
./mobile.sh ios dev
./mobile.sh ios build
```

The simulator needs no account. A **physical device** needs a signing team —
a free Apple ID works for a 7-day provisioning profile, which is enough to try
it — configured in the generated Xcode project under `src-tauri/gen/ios`.

## GNOME extension

No compiler and no build step. The only tool needed is the schema compiler:

```sh
sudo apt install libglib2.0-dev-bin      # Debian/Ubuntu
sudo dnf install glib2-devel             # Fedora
```

Then:

```sh
gnome-extension/install.sh
gnome-extensions enable usage-watcher@usagewatcher.dev
```

Requires **GNOME Shell 45 or newer**, which is where extensions moved to ES
modules. Check with `gnome-shell --version`. After installing, restart the
shell: on X11 press <kbd>Alt</kbd>+<kbd>F2</kbd> and type `r`; on **Wayland you
must log out and back in**, because the shell cannot reload itself in place —
and <kbd>Alt</kbd>+<kbd>F2</kbd> there silently does nothing, which is an easy
hour to lose.

## Portable zips

What a user should be given: unzip, double-click, done. No installer, no
elevation prompt, nothing left behind when the folder is deleted.

```sh
scripts/package.sh                                        # macOS, Linux, WSL
powershell -ExecutionPolicy Bypass -File scripts\package.ps1   # Windows
```

Each builds the panel, the app (`tauri build --no-bundle`, so you get the bare
executable rather than an installer around it), and both command-line binaries,
then writes `dist/usage-watcher-<version>-<platform>.zip` containing:

| | |
|---|---|
| `usage-watcher` | the app — tray icon, panel, and the collector inside it |
| `uw` | one-shot read from a terminal |
| `uwd` | the collector alone, for a headless box or WSL |
| `README.txt` | what to double-click, and where the settings live |

There is no cross-compiling here either: each zip is built on its own OS.

### The Linux zip is built in a container

```sh
scripts/package.sh --container
```

Same zip, built inside Ubuntu 22.04, and this is what a release wants. The
reason is glibc: its symbol versioning is backward compatible and not forward,
so a binary linked against 2.35 runs on 2.39 while one linked against 2.39 will
not start on 2.35 at all. **The build host, not the target, decides who can run
the result.** Building natively on a current distribution quietly set that floor
at glibc 2.39 — which excluded Debian 12, the current stable release.

22.04 is as far back as it goes, and the limit is WebKit rather than glibc:
Tauri v2 needs webkit2gtk **4.1**, 20.04 has only 4.0, and no amount of glibc
compatibility helps with a library that is not there. The reasoning, and the
one-line command that establishes it, are in the header of
[`scripts/Dockerfile.linux`](../scripts/Dockerfile.linux).

The result reaches further back than the base does. A binary's floor is the
newest symbol it actually references, not the glibc it was linked against, and
these land on **2.34** — Ubuntu 22.04, Debian 12, RHEL 9 and Rocky 9 included.
Worth re-measuring after a dependency bump rather than assuming it holds:

```sh
objdump -T usage-watcher | grep -oE 'GLIBC_[0-9.]+' | sort -uV | tail -1
```

glibc is only half of it for the app, which also needs webkit2gtk-4.1 at run
time — a distribution carrying only 4.0 can run `uw` and `uwd`, which link no
GUI library at all, but not the panel.

It runs as your own user, so nothing it writes is owned by root, and it keeps
its caches in `.container-cache/` rather than in `target/` — two toolchains
built against two different glibcs would otherwise invalidate each other's
artifacts every time you alternated between a native and a container build.

Nothing about this affects GNOME, which is the natural worry. The extension is
plain GJS and its schema is compiled on the user's machine by its own installer,
and the app's tray library is `dlopen`ed at runtime rather than linked — so both
bind to whatever the user actually has, not to what the container had.

Unsigned, which is worth being honest about. Windows SmartScreen will warn on
first run ("More info" → "Run anyway") and macOS Gatekeeper will refuse until
the binary is opened from the context menu once, or cleared with
`xattr -d com.apple.quarantine usage-watcher`. Signing is a certificate and a
notarisation pipeline, neither of which makes the app work better for the person
who built it.

## Release builds

```sh
cargo build --release -p uw-cli -p uwd          # any platform
npm --prefix widget run app:build               # native installer for the host
npm --prefix widget run android:build           # .apk / .aab
npm --prefix widget run ios:build               # .ipa, macOS only
```

`app:build` produces whatever is native to the host — `.msi` and `.exe` on
Windows, `.dmg` and `.app` on macOS, `.deb`/`.rpm`/`.AppImage` on Linux. There
is no cross-compiling between desktop platforms either: each installer must be
built on its own OS.

## Troubleshooting

**`pkg-config` cannot find `webkit2gtk-4.1`** — the Linux Tauri dependencies are
missing. See [Linux and WSL](#linux-and-wsl). Note it is `4.1`, not `4.0`;
Tauri v1 used the older one and a stale guide will send you to the wrong
package.

**`linker 'cc' not found` on Windows** — the C++ workload of the Visual Studio
Build Tools was not installed, only the installer itself.

**Gradle fails with no obvious cause** — almost always `NDK_HOME` unset or
pointing at a version that has been removed. `./mobile.sh android init` checks
this up front; if you invoked `tauri android` directly, it does not.

**`cargo test --workspace` tries to build the Tauri app** — it should not; the
root `Cargo.toml` excludes `widget/src-tauri`. If it does, you are running
Cargo from inside `widget/src-tauri`, which is its own workspace.

**The app starts but the panel says *Cannot reach uwd*** — the embedded
collector failed to start, and it logs why. Run the executable from a terminal
with `UWD_LOG=debug` and read the first few lines: the usual causes are a
`config.toml` that no longer parses and a `[daemon] bind` pointing at an address
this machine does not have. It falls back to an ephemeral port when the
configured one is merely busy, so "port in use" is not one of them.

**Signing in to Codex on Windows fails with "longer than platform limit of
2560 chars"** — fixed, but worth knowing what it was. The Windows Credential
Manager caps one credential blob at 2560 bytes and measures the value as UTF-16,
so an ASCII payload gets about 1280 characters. A Codex credential is a ChatGPT
JWT plus a refresh token, roughly three kilobytes, and did not fit; Claude and
OpenRouter have short tokens and were unaffected. Oversized credentials are now
split across numbered entries (`codex`, `codex#0`, `codex#1`, …), which is why
you may see several per provider in the Credential Manager. Nothing to do about
it, and entries written before the split still load as they are.

**Two copies of everything, or every provider polled twice** — a second
collector is running. The app probes `/health` on the configured port before
starting its own and defers to whatever answers, but a `uwd` bound somewhere
*else* is invisible to that check. Either stop it, or leave it running and point
the panel at it in Providers → Daemon settings.

**The panel says *Cannot reach uwd* but `curl` inside WSL works** — WSL2
localhost forwarding has broken, which happens after some Windows updates and
VPN changes. Bind the daemon to the WSL interface instead, which it refuses to
do unauthenticated:

```toml
[daemon]
bind = "0.0.0.0:7878"
token = "pick-something-long"
```

Then set that address and token in the panel's gear menu. `<wsl-ip>` is what
`hostname -I` prints inside WSL — it changes on every WSL restart, which is the
main reason to prefer localhost forwarding. You must **also** add that origin to
`connect-src` in `widget/src-tauri/tauri.conf.json`: the webview's CSP names the
daemons it may talk to, and an unlisted one is blocked before a request leaves.

**`Failed to unregister class Chrome_WidgetWin_0. Error = 1412` on exit** —
harmless, and not ours. 1412 is `ERROR_CLASS_HAS_WINDOWS`: the Chromium inside
WebView2 tries to unregister its window class while a window of that class is
still alive, and logs it at ERROR level. It has been an open Chromium issue
since 2012 and plain Chrome prints it too. The process is exiting, so nothing
leaks.

It is only visible because a debug build keeps a console attached —
`windows_subsystem = "windows"` in `main.rs` means release builds have none.

**A GNOME extension that will not enable** — the shell disables an extension
that throws during `enable()` and records why:

```sh
journalctl -f -o cat /usr/bin/gnome-shell
```

A daemon that is not running is *not* one of those cases; it shows as "Cannot
reach uwd" and keeps retrying.
