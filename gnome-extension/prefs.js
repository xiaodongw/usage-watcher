/**
 * Preferences. Runs in its own process, not in the shell, so a mistake here
 * cannot take the compositor down with it.
 */

import Adw from "gi://Adw";
import Gio from "gi://Gio";
import Gtk from "gi://Gtk";

import { ExtensionPreferences } from "resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js";

export default class UsageWatcherPreferences extends ExtensionPreferences {
  fillPreferencesWindow(window) {
    const settings = this.getSettings();

    const page = new Adw.PreferencesPage({
      title: "Usage Watcher",
      icon_name: "utilities-system-monitor-symbolic",
    });
    window.add(page);

    const connection = new Adw.PreferencesGroup({
      title: "Daemon",
      description:
        "This extension renders what uwd collects; it holds no credentials of " +
        "its own. Point it at a daemon on this machine, or at one reachable " +
        "over Tailscale.",
    });
    page.add(connection);

    const url = new Adw.EntryRow({ title: "Address" });
    url.text = settings.get_string("daemon-url");
    url.connect("changed", () => settings.set_string("daemon-url", url.text.trim()));
    connection.add(url);

    // A password row rather than a plain entry: the token is not a secret worth
    // much, but it is a secret, and shoulder-surfing a preferences window is
    // not a threat model anyone should have to think about.
    const token = new Adw.PasswordEntryRow({ title: "Token" });
    token.text = settings.get_string("daemon-token");
    token.connect("changed", () => settings.set_string("daemon-token", token.text.trim()));
    connection.add(token);

    const hint = new Adw.ActionRow({
      title: "Leave the token empty on loopback",
      subtitle:
        "uwd only requires one when bound to a non-loopback address — and it " +
        "refuses to bind one without it.",
    });
    hint.add_prefix(new Gtk.Image({ icon_name: "dialog-information-symbolic" }));
    connection.add(hint);

    const appearance = new Adw.PreferencesGroup({ title: "Top bar" });
    page.add(appearance);

    const pct = new Adw.SwitchRow({
      title: "Show the figure",
      subtitle: "The most-constrained meter across every provider.",
    });
    appearance.add(pct);
    settings.bind("show-percentage", pct, "active", Gio.SettingsBindFlags.DEFAULT);

    const notify = new Adw.SwitchRow({
      title: "Notify on threshold crossings",
      subtitle: "One notification per crossing, not one per poll.",
    });
    appearance.add(notify);
    settings.bind("notifications", notify, "active", Gio.SettingsBindFlags.DEFAULT);
  }
}
