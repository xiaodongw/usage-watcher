# usage-watcher — Plan

One place to see how much headroom is left across Claude Code, OpenAI Codex, opencode Zen (Go), and OpenRouter.

---

## 0. What I verified on this machine

Before choosing an architecture I probed each provider on your actual install. This is not
guesswork — every row below was executed and the result recorded.

| Provider | Source of truth | Verified result |
|---|---|---|
| **Claude Code** | `GET https://api.anthropic.com/api/oauth/usage`<br>`Authorization: Bearer <token>`, `anthropic-beta: oauth-2025-04-20`<br>Token from `~/.claude/.credentials.json` → `claudeAiOauth.accessToken` | ✅ **HTTP 200.** Returned `five_hour.utilization=28%` (resets `2026-08-17T10:00Z`), `seven_day.utilization=13%` (resets `2026-08-24T02:00Z`), plus a `limits[]` array with a per-model `weekly_scoped` bucket (Fable at 22%), an `extra_usage` credits block, and a `spend` block. `subscriptionType: pro`. |
| **OpenAI Codex** | **`GET https://chatgpt.com/backend-api/wham/usage`**<br>`Authorization: Bearer …` + `chatgpt-account-id: …`<br>*(or `codex app-server` JSON-RPC `account/rateLimits/read`)* | ✅ **Both work, and the HTTP one is better.** `wham/usage` returns the identical snapshot as a plain GET: `primary_window: {used_percent: 19, limit_window_seconds: 604800, reset_at: 1787284072}` → a **7-day** window resetting `2026-08-21T03:47Z`, `secondary_window: null`, `credits: {has_credits: false, unlimited: false, balance: "0"}`, `plan_type: plus`. No CLI, no subprocess, no JSON-RPC. See §3.3. |
| **OpenRouter** | `GET https://openrouter.ai/api/v1/credits` and `/api/v1/key` | ⚠️ **Routes confirmed live** (HTTP 401 without a key — i.e. the endpoint exists and wants auth). **No OpenRouter key is stored anywhere on this machine.** You will need to supply one. |
| **opencode Zen (Go)** | Local SQLite `~/.local/share/opencode/opencode.db`; `opencode stats` | ⚠️ **Spend only, no balance.** `opencode stats` reports $0.06 total across 16 sessions. I probed `api.opencode.ai/billing/{usage,packages,seats,actions}`, `opencode.ai/zen/go/v1/{credits,key}` and the gateway response headers — **no public balance/credits API exists.** The `/billing/*` strings in the binary are client-side routes of the `app.opencode.ai` web console (cookie auth), not an API. |

### Two findings that drive the whole design

**1. The four sources have different *locality*.** This is the single most important fact:

| | Needs the dev machine? | Why |
|---|---|---|
| OpenRouter | ❌ No | Plain portable API key, pure cloud call |
| Claude Code | ⚠️ Only for the token | The call is pure cloud; only the OAuth token lives locally |
| Codex | ✅ Yes | Requires the `codex` binary + its local OAuth token |
| opencode | ✅ Yes | Data exists *only* in the local SQLite DB |

**A phone-only app therefore cannot work.** It cannot reach Codex without reimplementing the
ChatGPT OAuth flow, and it cannot reach opencode at all. Any design that covers all four needs a
process on this machine. So the collector is not optional — build it first, and every UI becomes
cheap afterward.

**2. You are on WSL2.** The credentials and CLIs live inside WSL; a desktop widget wants to render
on Windows. These are different machines as far as process and filesystem access are concerned.
The split-binary design below handles this natively (WSL2 forwards `localhost` to Windows by
default). A single fused app would not.

### ⚠️ The token-rotation hazard

`~/.claude/.credentials.json` and `~/.codex/auth.json` hold *rotating* refresh tokens. **If the
watcher borrows one and refreshes it, it invalidates the token the real CLI holds and logs you out
of Claude Code / Codex.**

There are exactly two ways out, and the app supports both per provider — see §3.6:

- **Own grant** — the watcher does its own OAuth login and gets an *independent* token pair, which
  it may refresh freely. Preferred, and available for real on OpenRouter.
- **Delegated** — borrow the CLI's token strictly read-only, never refresh, render `stale` on
  expiry. The safe fallback where an own grant isn't available.

---

## 1. Functional requirements

### Must have (v1)

- **F1 — Unified headroom view.** One screen, one tile per provider, each showing the meters that
  actually matter for that provider:
  - Claude Code: 5-hour window %, 7-day window %, per-model weekly (Fable/Opus) %
  - Codex: weekly window %, credit balance
  - OpenRouter: remaining credits in $
  - opencode: rolling 30-day spend (with a visible "balance unavailable" note)
- **F2 — Time-to-reset.** Every window meter shows a live countdown to `resets_at`, not just a
  percentage. "72% used, resets in 1h 14m" is the number you actually act on.
- **F3 — Normalized model.** All four providers collapse into one shape (§3.2) so the UI never
  special-cases a provider and adding a fifth is a single adapter file.
- **F4 — Freshness + honesty.** Every tile shows its own last-updated time and one of
  `ok | stale | error | unavailable`. A provider that is down must look different from a provider
  reading 0%. Never render a stale number as if it were live.
- **F5 — Threshold alerts.** Desktop notification when a meter crosses a configurable threshold
  (default: 80% and 95% used; credits below $5). Fire once per crossing, not once per poll.
- **F6 — Polite polling.** Adaptive intervals, jitter, exponential backoff on error (§3.4). The
  watcher must never become the reason you hit a limit.

### Should have (v2)

- **F7 — Burn rate + projection.** From the sample history: current tokens/min and a projected
  "you will hit the cap at HH:MM" versus the reset time. This turns the tool from a gauge into a
  warning system. (Anthropic's payload gives `utilization` only, so this is derived locally from
  the sample series.)
- **F8 — Phone access.** Read the same dashboard from your phone over Tailscale (§4.3).
- **F9 — History.** 7-day sparkline per meter; answers "am I burning faster than last week".

### Explicitly out of scope

- Per-project / per-repo cost attribution. `ccusage` (already installed) does this well for Claude;
  don't rebuild it.
- Any *write* action — buying credits, redeeming Codex reset credits, toggling extra usage. The
  `account/rateLimitResetCredit/consume` method exists but a monitoring tool should not spend money.
- Team/org dashboards. This is a single-user tool.

---

## 2. Answering your actual question: widget or mobile?

**Neither, exactly — and the reason matters.**

### "Desktop widget" is a trap as literally specified

There is no cross-platform desktop widget standard. A genuinely *native* widget means three
separate implementations:

| Platform | Native widget requires |
|---|---|
| macOS | WidgetKit — Swift, plus an enclosing app container |
| Windows | Widgets Board — MSIX packaging + Adaptive Cards |
| Linux | *Nothing standard* — KDE plasmoids or GNOME Shell extensions, mutually incompatible |

Three codebases for four progress bars is a bad trade.

**The portable equivalent is a frameless, always-on-top, transparent window plus a tray icon.**
Every desktop toolkit supports this, it is one codebase, and on all three OSes it behaves exactly
like a widget. That's what to build.

### Mobile-only cannot cover your providers

Per §0, Codex and opencode both require a process on this machine. A Flutter app would still need
a daemon here plus a reachable transport. So mobile doesn't *replace* the daemon — it's a second
viewer on top of it.

### The call

Build a **headless collector daemon** + **thin viewers**. The daemon is the product; the widget and
the phone view are both ~200 lines of UI over the same JSON.

```
                    ┌─────────────────────────────────────┐
   WSL2             │  uwd  (headless collector daemon)   │
   (creds + CLIs)   │  ┌──────────┬──────────┬─────────┐  │
                    │  │ claude   │ codex    │ openr.  │  │
                    │  │ adapter  │ adapter  │ adapter │  │──▶ HTTP + SSE
                    │  └──────────┴──────────┴─────────┘  │    127.0.0.1:7878
                    │        normalize → cache → alert    │    (+ tailscale0)
                    └─────────────────────────────────────┘
                                      │
                  ┌───────────────────┼───────────────────┐
                  ▼                   ▼                   ▼
            Tauri widget          `uw` CLI            PWA on phone
            (Windows/Mac/Linux)   (terminal)          (over Tailscale)
```

---

## 3. Technical decisions

### 3.1 Stack

| Layer | Decision | Why |
|---|---|---|
| **Core + daemon** | **Rust**, one Cargo workspace | Single static binary ~5 MB, no runtime to install, cross-compiles to all three OSes, and it is Tauri's native language. Directly serves "very small and light". |
| **Widget** | **Tauri v2** + **Vue 3** + TypeScript + Vite | ~8 MB installers (vs 100 MB+ for Electron) because it uses the OS webview. Ships tray icon, frameless/transparent/always-on-top windows, autostart, and native notifications as first-class plugins. One codebase for Windows/Mac/Linux. **Vue because your other projects are Vue** — shared conventions, components and muscle memory carry over. |
| **Rust → TS types** | `ts-rs` | The normalized model (§3.2) is defined once in Rust and `#[derive(TS)]` emits the `.d.ts` the Vue app consumes, so there's no hand-maintained second copy of the schema to drift. |

> **The frontend framework choice is independent of Rust.** In Tauri the UI is a plain web app in
> the OS webview — Vue, Svelte and React all reach the backend the same way (`invoke()` over the IPC
> bridge, or plain HTTP/SSE as here), and none of them has any Rust integration. `ts-rs` emits
> ordinary TypeScript, consumable by any of them. Vue wins here purely because you already know it;
> the bundle-size gap is tens of KB inside an ~8 MB installer and doesn't move the needle.
| **Storage** | In-memory ring buffer + periodic JSONL snapshot | Enough for burn-rate over 24h. Add SQLite only when F9 (history charts) lands — don't pay for it before then. |
| **Transport** | HTTP `GET /snapshot` + `GET /events` (SSE) on `127.0.0.1:7878` | SSE means the widget is push-driven and never polls the daemon. Trivial for any future client. |
| **Config** | `~/.config/usage-watcher/config.toml` | Providers on/off, thresholds, intervals, bind address. |
| **Secrets** | OS keychain via `keyring` crate, env fallback (`OPENROUTER_API_KEY`) | The OpenRouter key is the only secret *we* own. Never write it to the config file. |

**Rejected:** Electron (size), Flutter for v1 (§4.3), Go (would mean two languages; Rust's only real
downside here — async subprocess management for the Codex adapter — is handled fine by tokio),
native per-OS widgets (three codebases).

### 3.2 The normalized model

Everything collapses to this. Adding a provider = writing one adapter that emits it.

```rust
struct Snapshot { generated_at: DateTime<Utc>, providers: Vec<Provider> }

struct Provider {
    id: String,              // "claude" | "codex" | "openrouter" | "opencode"
    label: String,
    plan: Option<String>,    // "pro", "plus"
    status: Status,          // Ok | Stale{since} | Error{msg} | Unavailable{reason}
    updated_at: DateTime<Utc>,
    meters: Vec<Meter>,
}

struct Meter {
    id: String,              // "session_5h", "weekly_all", "weekly_opus", "credits"
    label: String,
    kind: MeterKind,
    severity: Severity,      // Normal | Warning | Critical  (from thresholds)
}

enum MeterKind {
    Window   { used_pct: f32, resets_at: Option<DateTime<Utc>>, window_mins: Option<u32> },
    Balance  { amount: Decimal, currency: String, of_total: Option<Decimal> },
    Spend    { amount: Decimal, currency: String, period: Period },
}
```

Mapping from verified payloads:

- **Claude** → iterate the `limits[]` array (not the legacy top-level keys — `limits[]` is the
  forward-compatible one and already carries `kind`, `group`, `percent`, `severity`, `resets_at`,
  `scope.model.display_name`). Emit one `Window` per entry, plus `Balance` from `extra_usage` when
  `is_enabled`.
- **Codex** → `rateLimitsByLimitId` → per bucket, `primary`/`secondary` become `Window`
  (`usedPercent`, `resetsAt` as epoch seconds, `windowDurationMins`); `credits.balance` becomes
  `Balance`. Honour `credits.unlimited`.
- **OpenRouter** → `total_credits - total_usage` → `Balance { of_total: total_credits }`.
- **opencode** → aggregate cost from the local DB → `Spend { period: Rolling30d }`, and emit the
  balance meter as `Unavailable { reason: "no public credits API" }` so the gap is visible rather
  than silently missing.

### 3.3 Adapter notes

- **Claude** — read `~/.claude/.credentials.json` read-only, inotify-watch it. Plain GET. Cheap.
  Note the plan (`pro`) is **not** on the usage endpoint — it comes from the credential
  (`claudeAiOauth.subscriptionType`) or the token response, so carry it across explicitly.
- **Codex** — **use `GET /backend-api/wham/usage`, not `app-server`.** The discovery of that
  endpoint removed the single most complex adapter in the project: no supervised child process, no
  JSON-RPC handshake, no sparse-rolling-update merge semantics, and no multi-second startup per
  poll. It is one HTTP GET with two headers, so the Codex adapter is now the same shape as Claude's.
  The `account/rateLimits/read` route stays documented as a fallback if the endpoint ever moves.
  Two details that bite: `credits.balance` arrives as a **string**, and `has_credits: false` on a
  Plus plan means credits are not part of the plan at all — rendering that `"0"` as an
  empty wallet is a permanent false alarm, so the meter must be suppressed.
- **OpenRouter** — plain GET with the key. No rotation risk.
- **opencode** — copy the `.db` + `-wal` + `-shm` to a temp path and open `mode=ro` (the live DB is
  in WAL mode and opencode holds it open). Cost/token data lives in the JSON blob in
  `message.data`.

### 3.4 Polling policy

| Provider | Active | Idle | Notes |
|---|---|---|---|
| Claude | 60 s | 300 s | "Active" = 5h window > 0% |
| Codex | push (notifications) | 600 s full read | Long-lived app-server |
| OpenRouter | 300 s | 900 s | |
| opencode | 300 s | 300 s | Local read, essentially free |

All intervals get ±10% jitter. On error: exponential backoff 2× up to 15 min, tile goes `Error`,
then `Stale` after 3 consecutive failures. Hard floor of 30 s on any provider — the watcher must
never contribute meaningfully to your own usage.

### 3.5 Security

- Default bind `127.0.0.1` only.
- Phone access (F8) requires **Tailscale**: bind additionally to the `tailscale0` address and
  require a bearer token from the config. Never `0.0.0.0` without auth.
- No secret ever appears in `/snapshot`.
- Credential files are opened read-only, always.

### 3.6 Authentication — "log in per provider"

The goal is right: **an own grant means an own refresh token, which kills the rotation hazard
entirely.** But how far it can be taken differs sharply per provider, and I verified each.

| Provider | Own OAuth grant? | Evidence |
|---|---|---|
| **OpenRouter** | ✅ **Yes — real, public, documented** | `GET /auth?callback_url=…&code_challenge=…&code_challenge_method=S256` → user approves → callback carries `code` → `POST /api/v1/auth/keys` `{code, code_verifier}` → returns a **user-scoped API key**. I confirmed the exchange endpoint live: posting without a `code` returns a Zod validation error naming the missing `code` field, i.e. the route is real and this is exactly its contract. |
| **Claude** | ⚠️ **Mechanically yes, officially no** | PKCE public client. But `client_id` is an **OAuth 2.0 Client ID Metadata Document** URL that Anthropic owns: `https://claude.ai/oauth/claude-code-client-metadata` (fetched: `token_endpoint_auth_method: "none"`, `redirect_uris: ["http://localhost/callback","http://127.0.0.1/callback"]`, grants `authorization_code` + `refresh_token`). There is **no third-party app registration for consumer-subscription usage data**, so an "own" login means presenting as Claude Code. Whether their authorize endpoint would accept a self-hosted metadata URL is untested — Cloudflare blocks non-browser probes. |
| **Codex** | ⚠️ **Same** | `https://auth.openai.com/oauth/{authorize,token,revoke}`, PKCE `S256`, first-party client id `app_EMoamEEZ73f0CkXaXp7hrann`. No third-party registration for ChatGPT-subscription rate-limit data. |
| **opencode** | ⛔ **Moot** | The binary does support `"api" \| "oauth" \| "wellknown"` auth types, so a token is obtainable — but there is still **no balance/usage API to call with it** (§0). A login buys nothing. |

#### Why own-grant is required, not just preferred

Delegated mode borrows a vendor CLI's credential — so it only exists where that CLI is installed.
**On Android neither `claude` nor `codex` exists**, which makes own-grant the only mechanism that
can reach those two providers from a phone. That is what settles the toggle: it isn't a
nice-to-have hedge, it's the mobile client's only route.

Two facts make it actually viable, both verified:

- **Claude** already answers a plain `GET /api/oauth/usage` with nothing but a bearer token.
- **Codex** does too, via `GET /backend-api/wham/usage` (§0). Before finding that, own-grant on
  Android would have been *useless* for Codex — the app-server route needs the CLI binary, and the
  only other known source was response headers on a real inference call, which would have meant
  burning quota just to read the quota.

So a standalone Android client is feasible for both, holding its own tokens and calling both
endpoints directly, with no daemon on the far end.

**Note the distinction that trips people up:** Anthropic and OpenAI *do* run real OAuth programs —
for API-org access and MCP connectors. Those grant metered API access billed to an org. They do
**not** expose your Pro/Plus subscription's 5-hour window. The subscription meters are only visible
to the first-party CLI identity.

#### Design: one `Connect` button, three mechanisms

Model auth as a trait so the UI never branches on provider:

```rust
enum AuthMode {
    OwnGrant   { authorize: Url, token: Url, client_id: String, scopes: Vec<String> },
    Delegated  { cred_path: PathBuf },   // read-only borrow, never refresh
    ApiKey     { keyring_entry: String },
}

trait ProviderAuth {
    fn modes(&self) -> Vec<AuthMode>;          // ordered by preference
    async fn connect(&self, m: &AuthMode) -> Result<Credential>;
    async fn access_token(&self) -> Result<Token>;  // refreshes iff OwnGrant
}
```

Per provider:

- **OpenRouter** → `OwnGrant`. Do exactly what you asked. Ship it.
- **Claude** → offer both. Default **`Delegated`** (safe, zero setup, works today); expose
  **"Sign in separately"** for an independent grant. Same button, a toggle in settings.
- **Codex** → default `Delegated` **via `codex app-server`**, and note this is genuinely the
  *better* option, not just the safer one: app-server owns refresh itself (no rotation risk at all)
  *and* hands back already-normalized rate-limit data, so an own grant would mean reimplementing
  the ChatGPT backend call for strictly less. Own grant stays available behind the toggle.
- **opencode** → `ApiKey`, pasted. No usage API regardless.

#### OAuth mechanics in Tauri v2

- **Loopback redirect, not a custom scheme.** Spin an ephemeral `axum` listener on `127.0.0.1`,
  open the system browser with `tauri-plugin-opener`, capture the `code`, shut the listener down.
  Custom URI schemes (`usagewatcher://`) are flakier across the three OSes and Claude's registered
  redirects are loopback anyway.
- **PKCE S256 always**, plus a `state` nonce checked on return. All four flows are public clients —
  there is no client secret to protect and none should be invented.
- **Storage is platform-split, and Linux is the exception.** macOS and Windows use the OS keychain
  (`keyring` with `apple-native` / `windows-native` — no extra system libraries). **Linux does
  not:** the Secret Service backend needs D-Bus and `libdbus-1-dev`, and WSL — the primary target
  here — runs no Secret Service daemon at all, so a keychain-only design fails to even build.
  Linux gets an owner-only `0600` file at `~/.local/share/usage-watcher/credentials.json`, written
  via temp-file-and-rename. This is what `gh`, `aws` and `docker` do in the same position.
- **Refresh discipline** even on an own grant: refresh ~60 s before `expires_at`, behind a
  single-flight mutex so concurrent pollers can't double-refresh, and persist the rotated refresh
  token *before* using the new access token. A crash between those two steps is the classic way to
  lock yourself out.
- **Revoke on disconnect** where the provider offers it (`auth.openai.com/oauth/revoke`).

---

## 4. Delivery phases

### Phase 0 — `uw` CLI (~1 day) ▸ *start here*

A single command that prints all four providers as one table, plus `--json`.

**Why first:** it forces every adapter and the normalized model to be correct before any UI exists,
and it is *already useful on its own* — you can run it from a shell or wire it into your Claude Code
statusline (`~/.claude/statusline-command.sh`, which you already customize). Zero UI risk.

Auth here is `Delegated` + pasted key only — **no OAuth yet**. That keeps phase 0 to one day and
proves the adapters and the normalized model in isolation.

Exit criteria: `uw` prints Claude 5h/7d/per-model, Codex weekly + credits, OpenRouter balance,
opencode spend — with correct countdowns and honest status for anything unavailable.

### Phase 1 — `uwd` daemon + auth (~3 days)

Poll loop, adaptive intervals, backoff, in-memory history ring, `GET /snapshot`, `GET /events`
(SSE), threshold alerts with edge-triggering, config file.

Then the `ProviderAuth` trait from §3.6: loopback `axum` listener, PKCE + `state`, keychain storage,
single-flight refresh with rotated-token-persisted-first ordering. **Land OpenRouter's `OwnGrant`
first** — it's the one flow that is fully supported, so it validates the whole machinery against a
provider that is guaranteed to work before the ambiguous ones are attempted.

Exit criteria: runs 24h without drift, without leaking the Codex child process, and — critically —
a `uw auth login openrouter` round-trip survives a daemon restart and an access-token expiry.

### Phase 2 — Tauri + Vue widget (~3 days)

Tray icon showing the most-constrained meter as its badge. Click → frameless always-on-top panel,
one row per meter: label, bar, percentage, countdown. Draggable, position remembered, autostart.
Native notifications on threshold crossings.

Vue 3 + `<script setup>` + TypeScript, types generated by `ts-rs`. Skip Pinia — the entire client
state is one snapshot object, so a single `useSnapshot()` composable holding a `ref<Snapshot>` fed
by `EventSource` is the whole store. Second window: a settings pane with the per-provider **Connect**
buttons (§3.6), each showing connected/expired state and a disconnect action.

**WSL note:** run `uwd` inside WSL, run the widget natively on Windows, point it at
`http://localhost:7878`. WSL2's default `localhostForwarding` makes this work with no extra
configuration.

Exit criteria: idles under ~40 MB RSS, installer under ~10 MB, survives daemon restart.

### Phase 3 — Burn rate + phone (~2 days)

F7 projection from the history ring. Then **serve the widget's web UI as a PWA from `uwd`** and
reach it over Tailscale — installs to the home screen on both iOS and Android, no app store, and it
reuses the Vue app you already built (same Vite build, different target), so the marginal cost is
close to zero.

### On Flutter

Skip it unless one specific requirement appears: **background push notifications on iOS.** A PWA
cannot do those, and that would justify a real app. But note it wouldn't be cheap — iOS push
requires a cloud relay with APNs credentials, so the daemon could no longer stay purely on your
LAN. Revisit only if "tell me on my phone while I'm away from the desk" becomes the point.

---

## 5. Open questions

1. **OpenRouter** — nothing to do manually: the `OwnGrant` flow (§3.6) mints the key for you on
   first Connect. A hand-made read key at openrouter.ai/keys stays available as a fallback.

1b. **Do you want the unsanctioned own-grant path for Claude and Codex at all?** Defaulting to
   `Delegated` gives you working meters today with zero risk. The own-grant toggle is maybe half a
   day of extra work and carries two real costs: it presents your watcher as the first-party CLI,
   and it can break without warning if either vendor tightens client checks. My recommendation is
   to build the trait now, ship `Delegated` as the default, and leave the toggle unbuilt until you
   actually hit a case where the borrowed token isn't enough — which, for read-only meter polling,
   may never happen.
2. **Codex 5-hour window** — your account currently reports a single **7-day** primary window and
   `secondary: null`. The schema fully supports a secondary window, so the adapter should render
   whatever arrives rather than assume "5h + weekly". Worth re-checking once you're mid-session,
   in case the 5h bucket only materializes under load.
3. **opencode balance** — genuinely unavailable via API today. Options: (a) ship spend-only and
   label the gap, ← *recommended*; (b) scrape `app.opencode.ai` with a session cookie (fragile,
   will break); (c) manually record top-ups in config and subtract local spend (approximate but
   offline-safe). Start with (a).
4. **Anthropic endpoint stability** — `/api/oauth/usage` is what Claude Code's own `/usage` uses.
   It is not a documented public API, so treat the field set as unstable: drive the UI from
   `limits[]` generically, and tolerate unknown/renamed keys instead of hard-failing.
