# Usage Watcher — GNOME Shell extension

The Linux front end. GNOME removed the system tray years ago, and the
`AppIndicator` shim it left behind gives a menu and nothing else — no live
figure in the top bar, which is the entire point. So Linux gets a real panel
indicator rather than the Tauri widget pretending to be one.

Like the widget and the CLI, it is a **viewer**: it holds no credentials, does
no polling, and only renders what `uwd` sends on `/events`. That is what makes
it safe to run inside the compositor process.

```
uwd ──HTTP/SSE──► extension.js ──► top bar
```

## Install

```sh
./install.sh
```

Then, as it prints: restart the shell (X11: <kbd>Alt</kbd>+<kbd>F2</kbd>, `r`;
Wayland: log out and back in — it cannot reload in place), and

```sh
gnome-extensions enable usage-watcher@usagewatcher.dev
```

Requires GNOME Shell 45 or newer, which is where extensions moved to ES modules.
`glib-compile-schemas` must be present to build the settings schema —
`libglib2.0-dev-bin` on Debian and Ubuntu, `glib2-devel` on Fedora.

## Settings

```sh
gnome-extensions prefs usage-watcher@usagewatcher.dev
```

Address and token, plus whether to show the figure and whether to notify. The
defaults assume a daemon on `127.0.0.1:7878` with no token, which is what `uwd`
does out of the box. A daemon on another machine needs a token, because `uwd`
refuses to bind anything but loopback without one.

## What it shows

The top bar carries the most-constrained meter across every provider, coloured
by severity — the same scale the CLI and the widget use, because the daemon
decides it and all three only render it. The menu breaks that down per provider,
one row per meter, with a bar and the time until it resets.

Two states that look alike but are not, and are drawn differently on purpose:

- **error** — red. Something is wrong and you can fix it.
- **unavailable** — dim. This provider structurally has nothing to report;
  opencode Zen publishes no usage API, an OpenRouter free key has no balance.
  Permanent, expected, and not worth a colour that means "act now".

When the stream drops, the last reading stays on screen dimmed rather than
blanking — a number you know is old still beats no number, as long as it does
not pretend to be current.

## Files

| | |
|---|---|
| `extension.js` | the indicator, the menu, and the shell lifecycle |
| `daemon.js` | SSE client over libsoup3, with reconnect backoff |
| `format.js` | mirrors `widget/src/lib/format.ts`, so the two agree |
| `prefs.js` | preferences, which run in their own process |

## Debugging

```sh
journalctl -f -o cat /usr/bin/gnome-shell
```

An extension that throws during `enable()` is disabled by the shell and the
reason lands there. The most common cause is a daemon that is not running,
which is *not* an error here — it shows as "Cannot reach uwd" and keeps
retrying.
