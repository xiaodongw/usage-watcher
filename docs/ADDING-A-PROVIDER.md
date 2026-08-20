# Adding a provider

One file, one enum arm. Nothing in the daemon, the CLI, the panel or the GNOME
extension needs to change — they are all driven off the registry and the
manifest, and this document is mostly about what that means in practice.

## What the interface actually is

`Adapter` in `crates/uw-core/src/providers/mod.rs`. Four methods are required:

| | |
|---|---|
| `id` / `label` | the config key, and what the user reads |
| `oauth_config` | the browser flow, or an error saying why there is not one |
| `delegated_path` | where the vendor CLI keeps its credential, or `None` |
| `spec` | the prose and the paste prompt — see below |
| `fetch` | one HTTP response in, one normalized `Provider` out |

Everything else has a default: `poll_intervals`, `enrich`, `read_delegated`,
`adopt_as`, `relogin_hint`, `read_full_credential`, `default_auth`.

This is not a plugin system in the dynamic-loading sense, and deliberately so.
Dynamically loaded providers would mean a stable ABI, a distribution channel and
a signing story, in exchange for nothing a recompile does not already give you
at this size. What it *is* is a system where a provider describes itself well
enough that no consumer has to know it exists.

## The manifest is derived, not declared

`Spec` carries only what cannot be worked out: the one-line summary, the icon,
the docs link, the wording for a pasted key, and which vendor CLI a delegated
read borrows from.

Which login methods a provider offers is **not** in there. It comes from the
other methods:

- a browser sign-in exists iff `oauth_config()` returns `Ok`, and when it does
  not, the error text is what the greyed-out row says;
- borrowing a CLI token exists iff `delegated_path()` is `Some`, and is
  *available* iff that path exists right now;
- pasting a key exists iff `spec()` supplied a `TokenPrompt`.

Two sources of truth would drift the first time someone added a flow without
updating a list of flows. Codex is the case that makes this concrete: it has no
`TokenPrompt`, because its usage endpoint needs an OAuth access token plus the
account id from the id_token, so a pasted API key would be accepted by the field
and then fail on every poll. Leaving the prompt out is the whole of expressing
that.

## The icon

`Spec::new` takes PNG bytes, not a path or a URL, and the file lives beside the
adapters in `crates/uw-core/src/providers/icons/`. Compiled in, base64'd into
the manifest, and rendered from a `data:` URI.

That is three deliberate constraints in a row, and each has a reason. It is
*compiled in* because a config screen that has to reach the network to draw a
list of things it already knows is a config screen that shows four broken
squares on a plane. It is a `data:` URI because the panel's CSP is
`img-src 'self' data:` — a `http://localhost:<port>/icon` route would be
blocked, and widening the CSP to allow one is a poor trade for one image. And
it is *small* — 64×64, palette-quantised, under 2 KB — because every one of
them rides along in every `/providers` response.

To add one, take the provider's own favicon or apple-touch-icon, resize to
64×64 and quantise:

```python
from PIL import Image
im = Image.open("downloaded.png").convert("RGBA").resize((64, 64), Image.LANCZOS)
im.quantize(colors=64, method=Image.FASTOCTREE, dither=Image.NONE).save(
    "crates/uw-core/src/providers/icons/acme.png", optimize=True)
```

Check it on **both** themes before committing. Nothing in the CSS tints or
inverts the mark — a `filter` that rescued a black-on-transparent logo in dark
mode would wreck the three that carry their own background — so a mark that is
invisible on one theme stays invisible. Record where you got it in
`icons/SOURCES.md`; these are vendor trademarks, used to identify the vendor.

## Sketch

```rust
pub struct Acme;

impl Adapter for Acme {
    fn id(&self) -> &'static str { "acme" }
    fn label(&self) -> &'static str { "Acme AI" }

    fn spec(&self) -> Spec {
        Spec::new("Acme — monthly credit balance.", include_bytes!("icons/acme.png"))
            .docs("https://acme.example/docs")
            .token(
                "Paste an API key",        // the button
                "API key",                 // the field
                "sk-acme-…",               // the placeholder
                "Create a key in your Acme dashboard.",
                Some("https://acme.example/keys"),
            )
    }

    fn oauth_config(&self) -> Result<OAuthConfig> {
        bail!("Acme issues keys from its dashboard; there is no browser sign-in.")
    }

    fn delegated_path(&self) -> Option<PathBuf> { None }

    fn default_auth(&self) -> AuthPreference { AuthPreference::Token }

    async fn fetch(&self, http: &Client, cred: &Credential, kind: AuthKind)
        -> Result<Provider> { /* … */ }
}
```

Then add `Acme(acme::Acme)` to the `Any` enum, to `dispatch!`, and to
`Any::all()`. The compiler finds the two you forget.

## What you get for free

The CLI table, `uw provider list`, `uw auth status`, the daemon's schedule and
backoff, alerting, the tray readout, the GNOME menu, and both config screens —
including the row in "Add provider", its method list, and the reason any method
is unavailable on this machine.

## Rules that are not negotiable

- **Never refresh a borrowed token.** Claude and Codex both rotate refresh
  tokens; refreshing one you borrowed signs the user out of their real CLI.
  `read_delegated` must drop the refresh token, and there is a test for it.
  `read_full_credential` is the single deliberate exception, reached only from
  `uw auth adopt`, which then tells the user to re-run the vendor login.
- **Open the vendor's file read-only, and never write to it.**
- **No secrets in `config.toml`, and none in any response body.** Credentials go
  through `TokenStore`.
- **A 200 that yields nothing is an error, not an empty tile.** The opencode
  adapter shipped with the wrong response shape and rendered a healthy tile with
  no rows, which is how it went unnoticed. Fail loudly instead.
- **Judge severity with the shared thresholds** in `model.rs`, so every provider
  is measured on one scale.
