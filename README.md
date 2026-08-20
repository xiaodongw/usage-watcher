# usage-watcher

One place to see how much headroom is left across Claude Code, OpenAI Codex,
opencode and OpenRouter.

One collector, several viewers:

| | what it is | where it runs |
|---|---|---|
| `uwd` | collector daemon — polls, alerts, serves `/snapshot` + `/events` | wherever the credentials are |
| `uw` | CLI — one-shot read, and all the auth commands | same |
| `widget/` | Vue panel in a Tauri shell | Windows and macOS tray, Android and iOS |
| `gnome-extension/` | GNOME Shell panel indicator | Linux |

The daemon is the product. Everything else is a read-only viewer over the same
JSON, holding no credentials and doing no polling — which is what lets the UI
run on Windows, or on a phone, while the credentials and vendor CLIs stay inside
WSL.

```
                                  ┌─ tray widget (Windows, macOS)
uwd ──/snapshot + /events (SSE) ──┼─ GNOME extension (Linux)
 │                                ├─ phone app (Android, iOS)
 │                                └─ uw --json
 └── Claude · Codex · opencode · OpenRouter
```

Build prerequisites for every target and host are in
[docs/BUILDING.md](docs/BUILDING.md).

## Quick start

```sh
cargo install --path crates/uw-cli     # the `uw` command
uw auth login claude                   # own OAuth grant, refreshed by us
uw auth login codex                    # needs port 1455 free — no `codex login` running
uw auth login openrouter               # browser consent, mints a key for us
uw                                     # one-shot read

cargo run -p uwd                       # daemon on http://127.0.0.1:7878
npm --prefix widget install
npm --prefix widget run dev            # panel at http://localhost:5173
```

opencode needs no login: it keeps a static API key on disk and the default
`delegated` mode reads it.

## Providers

| | what it reports | how to authenticate |
|---|---|---|
| **Claude Code** | 5-hour, 7-day and per-model weekly windows | `login` (own grant), `delegated`, or `claude setup-token` |
| **OpenAI Codex** | its rate-limit windows, plus credits when the plan has them | `login` (own grant) or `delegated` |
| **OpenRouter** | credit balance, per-key spend cap, spend this month | `login` (own grant) or a key from the dashboard |
| **opencode** | rolling, weekly and monthly windows on the **Go** plan | `delegated` (reads the CLI's key) or `uw auth token` |

Two of these behave differently from the other two, and it is worth knowing why
before reading a tile:

- **OpenRouter has no vendor CLI**, so `delegated` is meaningless and the default
  is its own grant. Its PKCE flow exists precisely so a third-party app can mint
  a key on your behalf; what comes back is a durable API key with no expiry and
  no refresh token, which makes it the easiest credential here to carry to a
  phone. A free-tier key has no balance and no cap, so the tile says there is
  nothing to measure rather than showing an empty bar.
- **opencode only reports usage on the Go subscription.** `GET /zen/go/v1/usage`
  is undocumented — it was added in response to a request for exactly this, and
  the adapter is written against the console's own source — so treat it as
  liable to move. Zen proper is pay-as-you-go and publishes no usage or balance
  API at all; a Zen key therefore gets an honest "nothing to report" tile, not a
  red error. If the shape does change, the adapter fails loudly rather than
  quietly rendering a healthy tile with no rows.

Adding a provider means one file in `crates/uw-core/src/providers/` and one arm
in the `Any` enum beside it. Everything downstream — the CLI table, the panel,
alerting, the schedule — is driven off that registry and needs no edit.

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

The tray icon, the frameless popover, native notifications and start-at-login
live in the Tauri shell, which is a separate build — see [Viewers](#viewers).
Everything else, including the alert stream and the gear that sets the daemon
address, is identical.

## Auth modes

Set per provider with `uw auth mode <provider> <mode>`:

- **`own`** — our own OAuth grant and our own refresh token. The only mode that
  works where the vendor CLI does not exist, which is what a phone needs.
- **`delegated`** — read the vendor CLI's credential, strictly read-only. Claude
  and Codex both rotate refresh tokens, so refreshing a borrowed one would sign
  you out of your real CLI; the code drops the refresh token on read and a test
  keeps it that way. An expired borrowed token shows as an error telling you to
  run the vendor CLI, never as a silent refresh. opencode is the exception —
  its credential is a static key, so there is nothing to rotate and nothing to
  break.
- **`token`** — a long-lived token or API key pasted in by hand.

The default is `delegated` for anything with a vendor CLI to borrow from, and
the adapter's own choice otherwise — OpenRouter defaults to `own` because it has
no CLI, so the borrow would only ever produce an error tile.

`uw auth adopt <provider>` copies the vendor CLI's stored credential into our
own store, so the watcher keeps working where that CLI is not installed. What
that means depends on the provider, and `adopt` says which it did:

- Claude and Codex hand over a **rotating** OAuth grant. We then own and refresh
  it, and you must re-run the vendor login or whichever of you refreshes first
  signs the other out.
- opencode hands over a **static** API key. That is a copy, not a transfer —
  nothing rotates and the opencode CLI is unaffected.

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

[providers.opencode]
auth = "delegated"        # reads the key `opencode auth login` stored

[providers.openrouter]
enabled = false           # drop a provider you do not use, rather than
                          # leaving a permanent "not signed in" tile
```

Every provider is enabled by default, so an account you have not set up shows as
an error tile until you either sign in or switch it off.

Claude's usage endpoint is rate limited more tightly than the others, and a 429
there is normal rather than alarming. The daemon absorbs it: the first two
consecutive failures keep the last reading and merely dim it, and only a
sustained outage drops the numbers. A one-shot `uw` has nothing to fall back on,
so it prints the 429 — most often because `uwd` is already polling and the two
asked within a second of each other.

If Claude spends more time dimmed than not, raise its poll interval rather than
living with it. The 5-hour window moves about a third of a percent per minute,
so nothing is lost:

```toml
[providers.claude]
interval_active = 180
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

## Viewers

One Vue app, four shells, plus a native one for Linux. It runs in a plain
browser too — `npm --prefix widget run dev` — and that is the fastest way to
check a change before building anything native.

| shell | what it adds | build |
|---|---|---|
| browser | nothing; the panel as-is | `npm --prefix widget run dev` |
| Windows tray | tray icon, frameless popover, notifications, start at login | `npm run app:build` |
| macOS menu bar | the same, plus no Dock icon and the figure in the menu bar | `npm run app:build` |
| Android / iOS | full-screen panel, daemon address set in-app | `./mobile.sh <platform> build` |
| GNOME | native panel indicator, no webview at all | `gnome-extension/install.sh` |

**Prerequisites, per platform and per target, are in
[docs/BUILDING.md](docs/BUILDING.md)** — SDKs, system packages, environment
variables, and which combinations are impossible (iOS off a Mac, chiefly).

The daemon address is a **runtime** setting — the gear in the header — stored
per install. `VITE_UWD_URL` and `VITE_UWD_TOKEN` still work, but they are only
the initial default now. That change is what makes the phone builds possible:
the same signed binary has to reach a daemon whose address the user only learns
after installing it.

### The tray shells

Windows and macOS are the same Tauri app; the differences amount to a dozen
lines in `widget/src-tauri/src/tray.rs`.

Clicking the tray icon toggles the panel, which opens next to the icon — above
it or below it depending on where the taskbar or menu bar actually is, rather
than assuming. Clicking away dismisses it, the way every tray popover does.
Closing the panel does not quit: the tray icon is the entry point, and a second
launch reveals the existing window instead of starting a rival copy.

The tray carries the most-constrained figure so it can be read without opening
anything — macOS beside the menu-bar icon, Windows in the hover tooltip, each
ignoring the surface it does not support.

`widget/src-tauri` is deliberately **not** a member of the Cargo workspace:
building it needs the platform webview toolchain, which a WSL checkout usually
lacks, and keeping it out means `cargo test` at the repo root stays green
everywhere.

### The phone is a viewer, not a collector

It reads a daemon you are already running — over Tailscale, in practice, which
is also the only sane way to expose `uwd` beyond loopback. It does **not** poll
the providers itself, so it needs no credentials on the device.

That is a deliberate stopping point rather than an oversight. Polling on-device
is what the own-grant OAuth work was for and `uw-core` is already portable
enough for it, but it additionally needs a mobile credential store, a login flow
that does not assume a loopback redirect, and a way to test both — none of which
exist yet. Until then the phone reaches the daemon, and the daemon holds the
secrets.

### Linux gets an extension, not the widget

GNOME dropped the system tray, and the `AppIndicator` shim that replaced it
gives a menu but no live figure in the top bar — which is the whole point. So
Linux gets a real panel indicator instead of a webview pretending to be one.
See [`gnome-extension/README.md`](gnome-extension/README.md). It is plain GJS,
needs no build step, and like every other viewer holds no credentials.

## Types

`widget/src/types/` is generated from the Rust model by `ts-rs`; `cargo test`
rewrites it. Never edit those files — change `crates/uw-core/src/model.rs` and
re-run the tests.

## Tests

```sh
cargo test --workspace              # daemon, CLI, adapters
npm --prefix widget run test        # panel, formatting, settings
npm --prefix widget run check       # vue-tsc
```

The Tauri shell and the GNOME extension have no test suite and are not covered
by those: one needs a platform webview toolchain to compile at all, the other
needs a running GNOME session. Both are checked by building and running them.
