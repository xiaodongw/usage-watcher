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

The Vue app runs anywhere Node does. The Tauri shell needs the platform webview
toolchain, which a WSL checkout usually lacks, so `widget/src-tauri` is
deliberately **not** a member of the Cargo workspace: `cargo test` at the repo
root stays green without it.

- **On Windows/macOS** — install Rust and Node natively, then
  `npm --prefix widget run app:dev`. Point it at the WSL daemon via
  `VITE_UWD_URL`; WSL2 forwards `localhost` by default, so the default usually
  just works.
- **Inside WSL** — needs `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`,
  `librsvg2-dev`, `libxdo-dev` and `pkg-config` first.

`npm --prefix widget run tauri icon` replaces the placeholder icon with a real
platform set.

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
