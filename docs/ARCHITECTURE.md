# Architecture

- [One collector, several viewers](#one-collector-several-viewers)
- [Providers](#providers)
- [The API](#the-api)
- [Viewers](#viewers)
- [Generated types](#generated-types)

## One collector, several viewers

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
the "Add provider" screen, alerting, the schedule — is driven off that registry
and needs no edit. See [ADDING-A-PROVIDER.md](ADDING-A-PROVIDER.md).

Sign-in modes and polling intervals are in
[CONFIGURATION.md](CONFIGURATION.md).

## The API

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
| `PUT /providers` | `{ids}` — the display order, as dragged |
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
[BUILDING.md](BUILDING.md)** — SDKs, system packages, environment variables, and
which combinations are impossible (iOS off a Mac, chiefly).

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
See [`gnome-extension/README.md`](../gnome-extension/README.md). It is plain
GJS, needs no build step, and like every other viewer holds no credentials.

## Generated types

`widget/src/types/` is generated from the Rust model by `ts-rs`; `cargo test`
rewrites it. Never edit those files — change `crates/uw-core/src/model.rs` and
re-run the tests.
