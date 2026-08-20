# usage-watcher

One place to see how much headroom is left across Claude Code, OpenAI Codex,
opencode and OpenRouter.

One collector, several viewers:

| | what it is | where it runs |
|---|---|---|
| `uwd` | collector — polls, alerts, serves the API | wherever the credentials are |
| `uw` | CLI — one-shot read, and every auth command | same |
| `widget/` | Vue panel in a Tauri shell, **with the collector inside it** | Windows and macOS tray, Android and iOS |
| `gnome-extension/` | GNOME Shell panel indicator | Linux |

The collector is the product; everything else is a viewer over the same JSON.
On the desktop the app links `uwd` as a library and runs it in-process, so there
is one file to launch. That is not a different daemon — it is the same code,
reached the same way over loopback — which is what still lets the panel run on
Windows against a collector inside WSL, or on a phone against your desktop.

```
                                  ┌─ tray widget (Windows, macOS) ─┐
uwd ──/snapshot + /events (SSE) ──┼─ GNOME extension (Linux)       │ usually the
 │                                ├─ phone app (Android, iOS)      │ same process
 │                                └─ uw --json                     ┘
 └── Claude · Codex · opencode · OpenRouter
```

Build prerequisites for every target and host are in
[docs/BUILDING.md](docs/BUILDING.md).

## Quick start

### From a release

Unzip it and double-click **usage-watcher**. Nothing to install, nothing to
start first. It puts an icon in the tray; click that for the panel.

The first screen is empty with an **Add provider** button. Pick a provider, pick
how to sign in — the choices, and which of them work on this machine, come from
the provider itself — and it walks you through the rest. A browser sign-in opens
your real browser and the panel waits for you to come back.

`uw` and `uwd` are in the same folder for anyone who wants a terminal or a
headless box, and neither needs installing.

### From source

```sh
scripts/package.sh                     # or scripts\package.ps1 on Windows
```

…which builds the panel, the app and both binaries and leaves a zip in `dist/`.

To develop against it instead:

```sh
cargo install --path crates/uw-cli     # the `uw` command; --force to update
uw provider add claude                 # or do it from the panel
uw auth login claude                   # own OAuth grant, refreshed by us
uw                                     # one-shot read

cargo run -p uwd                       # collector on http://127.0.0.1:7878
npm --prefix widget install
npm --prefix widget run dev            # panel at http://localhost:5173
```

The version never changes during development, so `cargo install` refuses to
replace an existing `uw` and you keep running the old one — which shows up as a
provider or a flag that the source plainly supports being reported as unknown.
After a pull:

```sh
cargo install --path crates/uw-cli --force
```

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

**Anthropic rate limits the usage endpoint by IP, not by account.** An
unauthenticated request draws the same `429`, and the refusal carries
`Retry-After: 3600` — an hour. Everything on that address shares the budget:
two daemons (a Windows app and one in WSL both count), and every restart, since
a poller fetches once before its first sleep so a fresh sign-in appears
straight away. A 429 is obeyed rather than retried through: the provider waits
exactly as long as it was asked to, and the tile says when it will be back. If
you trip it while testing, nothing is broken — leave it alone for the hour.
Only `/api/oauth/usage` is limited, so Claude Code itself keeps working.

Adding a provider means one file in `crates/uw-core/src/providers/` and one arm
in the `Any` enum beside it. Everything downstream — the CLI table, the panel,
the "Add provider" screen, alerting, the schedule — is driven off that registry
and needs no edit. See
[docs/ADDING-A-PROVIDER.md](docs/ADDING-A-PROVIDER.md).

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

Chosen when you add a provider, in the panel or with
`uw auth mode <provider> <mode>`. The panel offers only the modes that provider
actually supports, and greys out the ones that cannot work on this machine with
the reason attached — "Codex is not signed in on this machine" rather than a
disabled row with no explanation.

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

Windows caps one credential at 2560 bytes and counts them as UTF-16, which a
Codex credential — a ChatGPT JWT plus a refresh token — exceeds. Those are split
across numbered entries (`codex`, `codex#0`, `codex#1`, …), so a provider may
occupy several rows in the Credential Manager. macOS and Linux have no such
limit and store one entry each.

## Configuration

`~/.config/usage-watcher/config.toml` — written by the config screen as well as
by hand.

**Being in `[providers]` is what "added" means.** An id that is not a key in
that table is not polled and does not appear in the panel, which is why a fresh
install opens on an empty screen rather than on four tiles nobody asked for. A
config file written before this change is migrated on first read: everything
that used to be implicitly on gets written down explicitly, so nothing stops
being watched.

```toml
version = 1               # written for you; see the migration note above
order = ["codex", "claude"]   # panel order, as dragged; see below

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
enabled = false           # keep the credential, stop the polling — what you
                          # want while a provider is having an outage
```

`order` appears once you drag a row in the provider list, and the panel's tiles
follow it. It names ids only, and is advisory: an id it does not mention is
shown after the ones it does, in alphabetical order, and an id that is no longer
added is ignored. So a file written before ordering existed keeps the
alphabetical order it always had, and mistyping an entry can move a provider but
never hide one.

Nothing secret is in there. Credentials go to the Windows Credential Manager,
the macOS Keychain, or an owner-only `0600` file on Linux, and removing a
provider deletes its credential along with its entry — so "remove" means removed
rather than merely hidden. `enabled = false` is the other half of that pair:
everything kept, nothing polled.

Two ways to change it, and they do the same thing:

```sh
uw provider list                  # what is watched, and what else could be
uw provider add openrouter        # `own`, `delegated` or `token` as a second argument
uw provider remove openrouter     # config entry and credential both
```

The collector reads this file at startup and then owns it. Editing it by hand
while `uwd` is running is fine, but the change lands on the next restart —
whereas anything done from the panel or through the API takes effect at once,
including starting or stopping that provider's polling.

A provider you have added but not signed in to shows as an error tile until you
do. Remove it, or switch it off, if you were only trying it.

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

Reading:

| route | |
|---|---|
| `GET /snapshot` | current reading, all providers |
| `GET /events` | SSE; `snapshot` per poll, `alert` on a severity increase, `providers` on a config change |
| `GET /history?since=` | recent snapshots from the ring |
| `GET /health` | unauthenticated liveness |

Writing — what the config screen drives:

| route | |
|---|---|
| `GET /providers` | the catalogue, and what is configured |
| `POST /providers` | `{id, auth}` — add, or change how one authenticates |
| `DELETE /providers/{id}` | remove, and delete its stored credential |
| `POST /providers/{id}/login` | begin a browser sign-in; returns the URL to open |
| `GET /providers/{id}/login` | how that sign-in is going |
| `POST /providers/{id}/login/code` | `{session, code}` for a provider that shows a code |
| `POST /providers/{id}/token` | `{token}` — store a pasted key |
| `POST /providers/{id}/logout` | forget the credential, keep the provider |

Alerts are edge-triggered — one notification when a meter crosses into warning
or critical, not one per poll for the next four hours.

The two halves have deliberately different CORS policies. Reading stays open to
any origin: it is a GET of data holding no secrets, so a scratch HTML file can
chart your usage. Writing does not, because those routes mint and delete
credentials — a page on some unrelated site must not reach them merely because
your collector is on loopback. They accept only origins that are plausibly this
app (the Tauri webview, or something served from localhost), and a bearer token
is still required off loopback because `uwd` will not bind a public address
without one.

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

The daemon address is a **runtime** setting, stored per install and reachable
from Providers → Daemon settings. Leave it blank and the panel uses whatever
collector the app started — which may be on an ephemeral port, if something else
already held 7878. Fill it in to watch a collector on another machine.
`VITE_UWD_URL` and `VITE_UWD_TOKEN` still work as the initial default. All of
this is what makes the phone builds possible: the same signed binary has to
reach a daemon whose address the user only learns after installing it.

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
