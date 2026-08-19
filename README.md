# usage-watcher

One place to see how much headroom is left across Claude Code, OpenAI Codex, and
(later) opencode and OpenRouter.

Three pieces:

| | what it is | where it runs |
|---|---|---|
| `uw` | CLI — one-shot read, and all the auth commands | wherever the credentials are |
| `uwd` | collector daemon — polls, alerts, serves `/snapshot` + `/events` | same |
| `widget/` | Vue panel in a Tauri tray app | the desktop you actually look at |

The daemon is the product. The widget and the CLI are read-only viewers over the
same JSON, which is what lets the UI run on Windows while the credentials and
vendor CLIs stay inside WSL.

## Quick start

```sh
cargo install --path crates/uw-cli     # the `uw` command
uw auth login claude                   # own OAuth grant, refreshed by us
uw auth login codex                    # needs port 1455 free — no `codex login` running
uw                                     # one-shot read

cargo run -p uwd                       # daemon on http://127.0.0.1:7878
npm --prefix widget install
npm --prefix widget run dev            # panel at http://localhost:5173
```

## Running it in WSL

This is the fastest way to see the UI, and it needs no toolchain beyond Rust and
Node — the browser view is the same Vue app the tray widget hosts, just without
the tray. Two terminals:

```sh
# 1 — the collector. Foreground, so you can watch it poll.
UWD_LOG=uwd=debug cargo run -p uwd

# 2 — the panel.
npm --prefix widget run dev
```

Then open **http://localhost:5173** in your Windows browser. WSL2 forwards
`localhost`, so the page reaches the daemon on `127.0.0.1:7878` inside WSL
without any configuration.

To leave them running while you do something else, detach and keep the logs:

```sh
cargo build -p uwd
(UWD_LOG=uwd=debug nohup ./target/debug/uwd > /tmp/uwd.log 2>&1 &)
(cd widget && nohup npm run dev > /tmp/vite.log 2>&1 &)

tail -f /tmp/uwd.log     # one line per poll at debug level
pkill -x uwd            # stop the daemon (-x matches the process name exactly,
                        # so it cannot catch a shell that merely mentions the path)
```

### Checking it works

```sh
curl -s localhost:7878/health                 # {"ok":true,...}
curl -s localhost:7878/snapshot | jq          # what the panel is rendering
curl -sN localhost:7878/events                # watch frames arrive live
uw                                            # same data, no daemon involved
```

In the panel itself: the dot beside the headline is green while the stream is
live, and the timestamp on the right counts up between polls, resetting each
time a provider re-polls.

Killing the daemon is a good check of the failure path — the panel keeps the
numbers on screen, dims them, and says it lost the connection rather than going
blank. Start it again and `EventSource` reconnects on its own; the daemon sends
the current snapshot as its first frame, so the panel repaints immediately
instead of waiting for the next poll.

### What the browser view does not have

The tray icon, the frameless always-on-top panel, and native notifications live
in the Tauri shell, which is a separate build — see
[Building the widget](#building-the-widget). Everything else, including the
alert stream, is identical.

## Auth modes

Set per provider with `uw auth mode <provider> <mode>`:

- **`own`** — our own OAuth grant and our own refresh token. The only mode that
  works where the vendor CLI does not exist, which is what a phone needs.
- **`delegated`** (default) — read the vendor CLI's credential, strictly
  read-only. Both vendors rotate refresh tokens, so refreshing a borrowed one
  would sign you out of your real CLI; the code drops the refresh token on read
  and a test keeps it that way. An expired borrowed token shows as an error
  telling you to run the vendor CLI, never as a silent refresh.
- **`token`** — a long-lived token pasted in by hand.

`uw auth adopt <provider>` copies the CLI's grant into our store as a one-off.
Re-run the vendor login afterwards so the two stop sharing a refresh token.

Credentials go to the OS keychain on macOS and Windows, and to an owner-only
`0600` file on Linux — WSL runs no Secret Service daemon, so `keyring` is not a
dependency there at all. Nothing secret is ever written to `config.toml` or
served over HTTP.

## Configuration

`~/.config/usage-watcher/config.toml`:

```toml
[daemon]
bind = "127.0.0.1:7878"   # non-loopback requires `token`, or uwd refuses to start
# token = "…"             # also accepted as ?token= , since EventSource cannot set headers
history = 1500            # snapshots kept in memory for burn-rate

[providers.claude]
auth = "own"
enabled = true
# interval_active = 60    # seconds; floored at 30 so the watcher never becomes
# interval_idle = 300     # a meaningful share of your own quota
```

## API

| route | |
|---|---|
| `GET /snapshot` | current reading, all providers |
| `GET /events` | SSE; `snapshot` on every poll, `alert` on a severity increase |
| `GET /history?since=` | recent snapshots from the ring |
| `GET /health` | unauthenticated liveness |

Alerts are edge-triggered — one notification when a meter crosses into warning
or critical, not one per poll for the next four hours.

## Building the widget

The Vue app runs anywhere Node does — that is what `npm --prefix widget run dev`
gives you, and it is enough to see live data in a browser.

The **Tauri shell** (tray icon, frameless always-on-top panel, native
notifications) needs the platform webview toolchain, which a WSL checkout
usually lacks. `widget/src-tauri` is therefore deliberately **not** a member of
the Cargo workspace, so `cargo test` at the repo root stays green without it.

### On Windows (the intended target)

The daemon stays in WSL — it is where the credentials and vendor CLIs live. Only
the widget is built natively.

1. **Toolchain**, once:
   - [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
     with the *Desktop development with C++* workload — Rust's MSVC target needs
     the linker.
   - [rustup](https://win.rustup.rs) (defaults are right: `stable-msvc`).
   - [Node LTS](https://nodejs.org).
   - WebView2 runtime — already present on Windows 11 and updated Windows 10.

2. **Get the source onto a Windows drive.** Building across `\\wsl.localhost`
   works but is slow enough to be unpleasant, because every file read crosses
   the 9p filesystem:

   ```powershell
   git clone \\wsl.localhost\Ubuntu\home\xiaodong\work\usage-watcher C:\dev\usage-watcher
   ```

   That clone is a normal git remote, so `git pull` there picks up commits made
   on the WSL side.

3. **Run it** — daemon in WSL, widget on Windows:

   ```sh
   cargo run -p uwd          # in WSL
   ```
   ```powershell
   cd C:\dev\usage-watcher\widget
   npm install
   npm run app:dev
   ```

   No `VITE_UWD_URL` needed: WSL2 forwards `localhost`, so a daemon bound to
   `127.0.0.1:7878` inside WSL answers on `localhost:7878` from Windows.

4. **Replace the placeholder icon** when you have a real one:

   ```powershell
   npm run tauri icon path\to\icon.png
   ```

#### If localhost forwarding is not working

It occasionally breaks after a Windows update or a VPN change. Symptom: the
panel says *Cannot reach uwd* while `curl` inside WSL is fine. Then bind the
daemon to the WSL interface instead — which it will not do unauthenticated:

```toml
[daemon]
bind = "0.0.0.0:7878"
token = "pick-something-long"
```

and on the Windows side set `widget/.env.local`:

```
VITE_UWD_URL=http://<wsl-ip>:7878
VITE_UWD_TOKEN=pick-something-long
```

`<wsl-ip>` is what `hostname -I` prints inside WSL. It changes on every WSL
restart, which is the main reason to prefer localhost forwarding. You must also
add that origin to `connect-src` in `widget/src-tauri/tauri.conf.json` — the
webview's CSP names the daemons it may talk to, and an unlisted one is blocked
before a request is sent.

### Inside WSL

Possible, but the tray behaviour under WSLg is not representative of a real
desktop. Needs, first:

```sh
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
                 librsvg2-dev libxdo-dev pkg-config build-essential
```

## Types

`widget/src/types/` is generated from the Rust model by `ts-rs`; `cargo test`
rewrites it. Never edit those files — change `crates/uw-core/src/model.rs` and
re-run the tests.

## Tests

```sh
cargo test --workspace
npm --prefix widget run test
npm --prefix widget run check
```
