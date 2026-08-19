/**
 * The uwd client: one snapshot fetch, one long-lived event stream, and the
 * reconnect policy between them.
 *
 * Kept apart from `extension.js` so the shell-facing code deals only in
 * "here is a snapshot" / "the connection dropped", and so this file can be
 * reasoned about without any St or Clutter in scope.
 *
 * GNOME Shell runs this on the compositor's main loop. Nothing here may block:
 * every read is async, and everything started is cancellable, because an
 * extension that leaves a socket or a timeout behind on `disable()` keeps the
 * whole shell process dirty until logout.
 */

import Gio from "gi://Gio";
import GLib from "gi://GLib";
import Soup from "gi://Soup?version=3.0";

/** Reconnect backoff in seconds. Mirrors the daemon's own poll backoff. */
const BACKOFF = [1, 2, 5, 10, 30, 60];

export class Daemon {
  /**
   * @param {object} opts
   * @param {() => {url: string, token: string}} opts.settings read on every
   *   (re)connect rather than captured once, so changing the daemon URL in
   *   preferences takes effect on the next attempt without a reload.
   * @param {(snapshot: object) => void} opts.onSnapshot
   * @param {(alert: object) => void} opts.onAlert
   * @param {(state: "connecting"|"live"|"offline") => void} opts.onState
   */
  constructor({ settings, onSnapshot, onAlert, onState }) {
    this._settings = settings;
    this._onSnapshot = onSnapshot;
    this._onAlert = onAlert;
    this._onState = onState;

    this._session = new Soup.Session({ timeout: 0 });
    this._cancellable = null;
    this._retryId = 0;
    this._attempt = 0;
    this._stopped = true;
  }

  start() {
    this._stopped = false;
    this._connect();
  }

  /**
   * Tear everything down. Safe to call twice, and safe to call from
   * `disable()` — which the shell may do at any moment, including while a read
   * is in flight.
   */
  stop() {
    this._stopped = true;
    this._clearRetry();

    if (this._cancellable) {
      this._cancellable.cancel();
      this._cancellable = null;
    }
    if (this._session) {
      this._session.abort();
      this._session = null;
    }
  }

  _clearRetry() {
    if (this._retryId) {
      GLib.source_remove(this._retryId);
      this._retryId = 0;
    }
  }

  _base() {
    const { url } = this._settings();
    return url.replace(/\/+$/, "");
  }

  /**
   * `EventSource` semantics without `EventSource`: the token rides as a query
   * parameter for the stream, matching what the widget does and what the
   * daemon accepts, because neither client can set a header on it.
   */
  _url(path) {
    const { token } = this._settings();
    const base = this._base();
    return token ? `${base}${path}?token=${encodeURIComponent(token)}` : `${base}${path}`;
  }

  _retryLater() {
    if (this._stopped) return;

    const delay = BACKOFF[Math.min(this._attempt, BACKOFF.length - 1)];
    this._attempt += 1;
    this._clearRetry();
    this._retryId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, delay, () => {
      this._retryId = 0;
      this._connect();
      return GLib.SOURCE_REMOVE;
    });
  }

  _connect() {
    if (this._stopped) return;

    // A fresh cancellable per attempt: cancelling one must not poison the next.
    this._cancellable = new Gio.Cancellable();
    this._onState("connecting");

    const message = Soup.Message.new("GET", this._url("/events"));
    if (!message) {
      // Only happens if the configured URL is not parseable as one.
      this._onState("offline");
      this._retryLater();
      return;
    }
    message.request_headers.append("Accept", "text/event-stream");

    this._session.send_async(
      message,
      GLib.PRIORITY_DEFAULT,
      this._cancellable,
      (session, result) => {
        let stream;
        try {
          stream = session.send_finish(result);
        } catch (e) {
          if (!e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) {
            this._onState("offline");
            this._retryLater();
          }
          return;
        }

        if (message.get_status() !== Soup.Status.OK) {
          // 401 here means a token mismatch, and retrying forever will not fix
          // it — but neither should the extension give up silently, since the
          // user may be about to correct it in preferences.
          this._onState("offline");
          this._retryLater();
          return;
        }

        this._attempt = 0;
        this._onState("live");
        this._read(new Gio.DataInputStream({ base_stream: stream }), { event: null, data: [] });
      },
    );
  }

  /**
   * Read the stream a line at a time, assembling SSE frames.
   *
   * Recursion through the async callback rather than a loop is the GJS idiom
   * here: each `read_line_async` returns to the main loop, so a stream that
   * goes quiet costs nothing and the shell stays responsive.
   */
  _read(dis, frame) {
    if (this._stopped) return;

    dis.read_line_async(GLib.PRIORITY_DEFAULT, this._cancellable, (source, result) => {
      let line;
      try {
        [line] = source.read_line_finish_utf8(result);
      } catch (e) {
        if (!e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) {
          this._onState("offline");
          this._retryLater();
        }
        return;
      }

      // End of stream: the daemon went away, or the socket was closed.
      if (line === null) {
        this._onState("offline");
        this._retryLater();
        return;
      }

      if (line === "") {
        this._dispatch(frame);
        this._read(dis, { event: null, data: [] });
        return;
      }

      if (line.startsWith(":")) {
        // A comment, which is how SSE keep-alives are sent. Nothing to do —
        // but arriving at all is proof the connection is still good.
      } else if (line.startsWith("event:")) {
        frame.event = line.slice(6).trim();
      } else if (line.startsWith("data:")) {
        frame.data.push(line.slice(5).trimStart());
      }

      this._read(dis, frame);
    });
  }

  _dispatch(frame) {
    if (!frame.event || frame.data.length === 0) return;

    let payload;
    try {
      payload = JSON.parse(frame.data.join("\n"));
    } catch {
      // A frame we cannot parse is not worth dropping the connection over.
      return;
    }

    if (frame.event === "snapshot") this._onSnapshot(payload);
    else if (frame.event === "alert") this._onAlert(payload);
  }
}
