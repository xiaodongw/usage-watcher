# Configuration

- [Sign-in modes](#sign-in-modes)
- [Where credentials live](#where-credentials-live)
- [The config file](#the-config-file)
- [Changing it](#changing-it)
- [How often it polls](#how-often-it-polls)
- [Rate limits](#rate-limits)

## Sign-in modes

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

## Where credentials live

The OS keychain on macOS and Windows, and an owner-only `0600` file on Linux —
WSL runs no Secret Service daemon, so `keyring` is not a dependency there at
all. Nothing secret is ever written to `config.toml` or served over HTTP.

Windows caps one credential at 2560 bytes and counts them as UTF-16, which a
Codex credential — a ChatGPT JWT plus a refresh token — exceeds. Those are split
across numbered entries (`codex`, `codex#0`, `codex#1`, …), so a provider may
occupy several rows in the Credential Manager. macOS and Linux have no such
limit and store one entry each.

## The config file

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
# interval_active = 180   # seconds; floored at 30 so the watcher never becomes
# interval_idle = 600     # a meaningful share of your own quota

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

Nothing secret is in there. Credentials go where [Where credentials
live](#where-credentials-live) says, and removing a provider deletes its
credential along with its entry — so "remove" means removed rather than merely
hidden. `enabled = false` is the other half of that pair: everything kept,
nothing polled.

## Changing it

Two ways, and they do the same thing:

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

## How often it polls

**Every provider polls on the same rhythm: 180s while something is being
consumed, 600s otherwise.** The adapters used to disagree — a minute for Claude,
because its 5-hour window can move several percent in that time; five for
OpenRouter, because a prepaid balance only moves when you spend. All true, and
all beside the point once you notice these are undocumented endpoints counted
per IP. Three minutes costs a percent or two of resolution on the one meter fast
enough to notice; a minute costs the whole tile for an hour.

"Consumed" is decided by the window that resets soonest, and only that one.
Claude reports a 5-hour window beside two weekly ones, and the weeklies sit at
double digits for days after a single request — so under the older rule of "any
window above zero" Claude never once reached the idle tier. It polled every
minute around the clock whether or not it had been touched since Tuesday.

## Rate limits

Claude's usage endpoint is limited more tightly than the others, and a 429 there
is normal rather than alarming. The daemon absorbs it: the first two consecutive
failures keep the last reading and merely dim it, and only a sustained outage
drops the numbers. A one-shot `uw` has nothing to fall back on, so it prints the
429 — most often because `uwd` is already polling and the two asked within a
second of each other.

**Anthropic limits that endpoint by IP, not by account.** An unauthenticated
request draws the same `429`, and the refusal carries `Retry-After: 3600` — an
hour. Everything on that address shares the budget: two daemons (a Windows app
and one in WSL both count), and every restart, since a poller fetches once
before its first sleep so a fresh sign-in appears straight away. A 429 is obeyed
rather than retried through: the provider waits exactly as long as it was asked
to, and the tile says when it will be back. If you trip it while testing,
nothing is broken — leave it alone for the hour. Only `/api/oauth/usage` is
limited, so Claude Code itself keeps working.
