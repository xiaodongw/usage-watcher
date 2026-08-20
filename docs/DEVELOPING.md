# Developing

Build prerequisites for every target and host are in [BUILDING.md](BUILDING.md).
This is the loop once you have them.

- [From source](#from-source)
- [The fast loop](#the-fast-loop)
- [Checking it works](#checking-it-works)
- [What the browser view does not have](#what-the-browser-view-does-not-have)
- [Tests](#tests)

## From source

```sh
scripts/package.sh                     # or scripts\package.ps1 on Windows
```

…which builds the panel, the app and both binaries and leaves a zip in `dist/`.
It checks its system dependencies first, so a missing one costs a second rather
than a ten-minute build.

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

## The fast loop

This is the quickest way to see the UI, and it needs no toolchain beyond Rust
and Node — the browser view is the same Vue app the tray widget hosts, just
without the tray. Two terminals:

```sh
# 1 — the collector. Foreground, so you can watch it poll.
UWD_LOG=uwd=debug cargo run -p uwd

# 2 — the panel.
npm --prefix widget run dev
```

Then open **http://localhost:5173**. Under WSL that works from a Windows browser
with no configuration: WSL2 forwards `localhost`, so the page reaches the daemon
on `127.0.0.1:7878` inside WSL.

To leave them running while you do something else, detach and keep the logs:

```sh
cargo build -p uwd
(UWD_LOG=uwd=debug nohup ./target/debug/uwd > /tmp/uwd.log 2>&1 &)
(cd widget && nohup npm run dev > /tmp/vite.log 2>&1 &)

tail -f /tmp/uwd.log     # one line per poll at debug level
pkill -x uwd            # stop the daemon (-x matches the process name exactly,
                        # so it cannot catch a shell that merely mentions the path)
```

## Checking it works

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

## What the browser view does not have

The tray icon, the frameless popover, native notifications and start-at-login
live in the Tauri shell, which is a separate build — see
[Viewers](ARCHITECTURE.md#viewers). Everything else, including the alert stream
and the gear that sets the daemon address, is identical.

## Tests

```sh
cargo test --workspace              # daemon, CLI, adapters
npm --prefix widget run test        # panel, formatting, settings
npm --prefix widget run check       # vue-tsc
```

The Tauri shell and the GNOME extension have no test suite and are not covered
by those: one needs a platform webview toolchain to compile at all, the other
needs a running GNOME session. Both are checked by building and running them.

`cargo test` also regenerates `widget/src/types/` from the Rust model — see
[Generated types](ARCHITECTURE.md#generated-types).
