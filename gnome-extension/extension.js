/**
 * Usage Watcher — GNOME Shell panel indicator.
 *
 * The third front end over the same `uwd` JSON, after the CLI and the Tauri
 * widget. It holds no credentials and does no polling: the daemon owns both,
 * and this only renders what arrives on `/events`. That is what makes it safe
 * to run inside the compositor process.
 *
 * GNOME 45+ only, which is where extensions became ESM.
 */

import Clutter from "gi://Clutter";
import GLib from "gi://GLib";
import GObject from "gi://GObject";
import St from "gi://St";

import * as Main from "resource:///org/gnome/shell/ui/main.js";
import * as PanelMenu from "resource:///org/gnome/shell/ui/panelMenu.js";
import * as PopupMenu from "resource:///org/gnome/shell/ui/popupMenu.js";
import { Extension } from "resource:///org/gnome/shell/extensions/extension.js";

import { Daemon } from "./daemon.js";
import { countdown, fill, mostConstrained, readout, resetsAt, severityOf } from "./format.js";

/** Bar width in pixels for a meter row in the menu. */
const BAR_WIDTH = 120;

const Indicator = GObject.registerClass(
  class UsageWatcherIndicator extends PanelMenu.Button {
    _init(extension) {
      super._init(0.5, "Usage Watcher");
      this._extension = extension;
      this._settings = extension.getSettings();

      const box = new St.BoxLayout({ style_class: "panel-status-menu-box" });
      this._icon = new St.Icon({
        icon_name: "utilities-system-monitor-symbolic",
        style_class: "system-status-icon",
      });
      this._label = new St.Label({
        text: "",
        y_align: Clutter.ActorAlign.CENTER,
        style_class: "uw-panel-label",
      });
      box.add_child(this._icon);
      box.add_child(this._label);
      this.add_child(box);

      this._body = new PopupMenu.PopupMenuSection();
      this.menu.addMenuItem(this._body);

      this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
      const prefs = new PopupMenu.PopupMenuItem("Settings…");
      prefs.connect("activate", () => extension.openPreferences());
      this.menu.addMenuItem(prefs);

      this._snapshot = null;
      this._state = "connecting";
      this._render();
    }

    setState(state) {
      this._state = state;
      this._render();
    }

    setSnapshot(snapshot) {
      this._snapshot = snapshot;
      this._state = "live";
      this._render();
    }

    /**
     * The whole menu is rebuilt on every snapshot rather than diffed.
     *
     * Providers appear, vanish and change their meter set between polls, and at
     * this size — four providers, a handful of rows — rebuilding costs less
     * than the bookkeeping a diff would need to stay correct.
     */
    _render() {
      this._renderPanel();
      this._body.removeAll();

      const providers = this._snapshot?.providers ?? [];

      if (!this._snapshot) {
        this._body.addMenuItem(
          new PopupMenu.PopupMenuItem(
            this._state === "connecting" ? "Connecting to uwd…" : "Cannot reach uwd",
            { reactive: false },
          ),
        );
        if (this._state === "offline") {
          const hint = new PopupMenu.PopupMenuItem(
            `Start it, or check the address in Settings.`,
            { reactive: false },
          );
          hint.label.add_style_class_name("uw-hint");
          this._body.addMenuItem(hint);
        }
        return;
      }

      if (providers.length === 0) {
        this._body.addMenuItem(new PopupMenu.PopupMenuItem("No providers enabled.", { reactive: false }));
        return;
      }

      const now = Date.now();
      providers.forEach((provider, i) => {
        if (i > 0) this._body.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._body.addMenuItem(this._providerHeader(provider));

        // `unavailable` is a fact about the provider, not a failure, so it is
        // rendered as dim prose rather than in the error colour — the same
        // distinction the Vue panel makes.
        const status = provider.status ?? {};
        if (status.state === "error" || status.state === "unavailable") {
          const note = new PopupMenu.PopupMenuItem(status.message ?? status.reason ?? "", {
            reactive: false,
          });
          note.label.add_style_class_name(status.state === "error" ? "uw-error" : "uw-note");
          note.label.clutter_text.line_wrap = true;
          this._body.addMenuItem(note);
          return;
        }

        for (const meter of provider.meters ?? []) {
          this._body.addMenuItem(this._meterRow(meter, now, status.state === "stale"));
        }
        if ((provider.meters ?? []).length === 0) {
          const note = new PopupMenu.PopupMenuItem("Nothing reported.", { reactive: false });
          note.label.add_style_class_name("uw-note");
          this._body.addMenuItem(note);
        }
      });

      // Blanking the menu on a dropped connection would throw away numbers we
      // still have, so the rows stay and the loss is stated instead.
      if (this._state === "offline") {
        this._body.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        const lost = new PopupMenu.PopupMenuItem("Lost the connection to uwd — retrying.", {
          reactive: false,
        });
        lost.label.add_style_class_name("uw-error");
        this._body.addMenuItem(lost);
      }
    }

    _providerHeader(provider) {
      const item = new PopupMenu.PopupBaseMenuItem({ reactive: false, style_class: "uw-header" });
      const name = new St.Label({ text: provider.label ?? provider.id, style_class: "uw-provider" });
      item.add_child(name);
      if (provider.plan) {
        item.add_child(new St.Label({ text: provider.plan, style_class: "uw-plan" }));
      }
      item.add_child(new St.Widget({ x_expand: true }));
      if (provider.status?.state === "stale") {
        item.add_child(new St.Label({ text: "stale", style_class: "uw-plan" }));
      }
      return item;
    }

    _meterRow(meter, now, stale) {
      const item = new PopupMenu.PopupBaseMenuItem({ reactive: false, style_class: "uw-row" });
      if (stale) item.add_style_class_name("uw-stale");

      item.add_child(new St.Label({ text: meter.label, style_class: "uw-meter-label" }));

      // St has no progress bar, so the track is a fixed-width box and the fill
      // is a child sized as a fraction of it.
      const track = new St.Bin({ style_class: "uw-track", style: `width: ${BAR_WIDTH}px;` });
      const width = Math.round(fill(meter) * BAR_WIDTH);
      track.set_child(
        new St.Widget({
          style_class: `uw-fill uw-${meter.severity}`,
          style: `width: ${width}px;`,
        }),
      );
      item.add_child(track);

      item.add_child(new St.Widget({ x_expand: true }));
      item.add_child(new St.Label({ text: readout(meter), style_class: "uw-value" }));

      const resets = countdown(resetsAt(meter), now);
      item.add_child(new St.Label({ text: resets ?? "", style_class: "uw-resets" }));
      return item;
    }

    _renderPanel() {
      const providers = this._snapshot?.providers ?? [];
      const head = mostConstrained(providers);

      for (const cls of ["uw-normal", "uw-warning", "uw-critical", "uw-offline"]) {
        this._label.remove_style_class_name(cls);
        this._icon.remove_style_class_name(cls);
      }

      if (!this._snapshot) {
        this._label.text = "";
        this._icon.add_style_class_name(this._state === "offline" ? "uw-offline" : "uw-normal");
        return;
      }

      // No meters anywhere is a legitimate state — every provider could be a
      // free OpenRouter key. Show a dot rather than a misleading "0%".
      this._label.text = this._settings.get_boolean("show-percentage")
        ? (head ? readout(head.meter) : "·")
        : "";

      const sev = severityOf(providers);
      const cls = this._state === "offline" ? "uw-offline" : `uw-${sev}`;
      this._label.add_style_class_name(cls);
      this._icon.add_style_class_name(cls);
    }
  },
);

export default class UsageWatcherExtension extends Extension {
  enable() {
    const settings = this.getSettings();
    this._settings = settings;

    this._indicator = new Indicator(this);
    Main.panel.addToStatusArea(this.uuid, this._indicator);

    this._daemon = new Daemon({
      settings: () => ({
        url: settings.get_string("daemon-url"),
        token: settings.get_string("daemon-token"),
      }),
      onSnapshot: (s) => this._indicator?.setSnapshot(s),
      onAlert: (a) => this._notify(a),
      onState: (s) => this._indicator?.setState(s),
    });
    this._daemon.start();

    // Changing the address must reconnect immediately rather than at the next
    // backoff tick, which can be a minute away.
    this._changedId = settings.connect("changed", (_s, key) => {
      if (key === "daemon-url" || key === "daemon-token") {
        this._daemon?.stop();
        this._daemon?.start();
      } else {
        this._indicator?.setState(this._indicator._state);
      }
    });

    // Repaint once a minute so the "resets in" countdowns keep moving between
    // polls; the daemon may be five minutes apart on a quiet provider.
    this._tickId = GLib.timeout_add_seconds(GLib.PRIORITY_LOW, 60, () => {
      this._indicator?._render();
      return GLib.SOURCE_CONTINUE;
    });
  }

  disable() {
    // Order matters: stop producing events before destroying what renders them.
    if (this._tickId) {
      GLib.source_remove(this._tickId);
      this._tickId = 0;
    }
    this._daemon?.stop();
    this._daemon = null;

    if (this._changedId && this._settings) {
      this._settings.disconnect(this._changedId);
      this._changedId = 0;
    }
    this._settings = null;

    this._indicator?.destroy();
    this._indicator = null;
  }

  _notify(alert) {
    if (!this._settings?.get_boolean("notifications")) return;
    Main.notify("Usage Watcher", alert?.message ?? "");
  }
}
