import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import App from "./App.vue";
import { DEFAULTS, settings } from "./lib/settings";
import { providersView } from "./lib/providers";
import type { ProvidersView } from "./types/ProvidersView";
import type { Snapshot } from "./types/Snapshot";

/**
 * A stand-in for the browser's `EventSource`, so the whole path — stream to
 * composable to tiles — is exercised without a live daemon or a real browser.
 */
class FakeEventSource {
  static last: FakeEventSource | null = null;
  static opened: FakeEventSource[] = [];
  listeners: Record<string, ((e: unknown) => void)[]> = {};
  closed = false;

  constructor(public url: string) {
    FakeEventSource.last = this;
    FakeEventSource.opened.push(this);
  }

  addEventListener(type: string, fn: (e: unknown) => void) {
    (this.listeners[type] ??= []).push(fn);
  }

  close() {
    this.closed = true;
  }

  emit(type: string, data?: unknown) {
    for (const fn of this.listeners[type] ?? []) {
      fn(data === undefined ? {} : { data: JSON.stringify(data) });
    }
  }
}

const SNAPSHOT: Snapshot = {
  generated_at: new Date().toISOString(),
  providers: [
    {
      id: "claude",
      label: "Claude Code",
      plan: "max",
      status: { state: "ok" },
      auth: "own_grant",
      updated_at: new Date().toISOString(),
      meters: [
        {
          id: "session",
          label: "5-hour",
          kind: { type: "window", used_pct: 8, resets_at: null, window_mins: 300 },
          severity: "normal",
        },
      ],
    },
    {
      id: "codex",
      label: "OpenAI Codex",
      plan: "plus",
      status: { state: "ok" },
      auth: "delegated",
      updated_at: new Date().toISOString(),
      meters: [
        {
          id: "primary",
          label: "7-day",
          kind: { type: "window", used_pct: 88, resets_at: null, window_mins: 10080 },
          severity: "warning",
        },
      ],
    },
  ],
};

/**
 * Stands in for the vendor mark the daemon inlines. A real one is a few
 * kilobytes of base64 and jsdom never decodes it, so the bytes would only be
 * noise in the fixture.
 */
const ICON = "data:image/png;base64,iVBORw0KGgo=";

/**
 * The provider manifest, as the daemon builds it from the adapters. Only the
 * fields the UI actually renders — the point of the manifest is that the UI
 * does not know what a "claude" is.
 */
const CATALOGUE: ProvidersView = {
  catalogue: [
    {
      id: "claude",
      label: "Claude Code",
      summary: "Session and weekly windows.",
      icon: ICON,
      methods: [
        {
          auth: "own",
          kind: "browser",
          label: "Sign in with your browser",
          detail: "usage-watcher gets its own credential.",
          recommended: false,
          available: true,
        },
        {
          auth: "delegated",
          kind: "borrow",
          label: "Use the Claude Code sign-in",
          detail: "Reads the CLI's file, read-only.",
          recommended: true,
          available: false,
          unavailable_reason: "Claude Code is not signed in on this machine.",
        },
        {
          auth: "token",
          kind: "paste",
          label: "Paste a long-lived token",
          detail: "Run `claude setup-token`.",
          recommended: false,
          available: true,
          token: {
            action: "Paste a long-lived token",
            label: "Token",
            placeholder: "sk-ant-oat01-…",
            help: "Run `claude setup-token` and paste what it prints.",
          },
        },
      ],
    },
  ],
  configured: [],
};

const CONFIGURED: ProvidersView = {
  ...CATALOGUE,
  configured: [
    {
      id: "claude",
      label: "Claude Code",
      icon: ICON,
      auth: "own",
      enabled: true,
      signed_in: true,
    },
  ],
};

// Without this, every App mounted by an earlier case stays alive with its
// watcher attached to the shared `settings` singleton — so changing the address
// in one test redials in all of them.
enableAutoUnmount(afterEach);

beforeEach(() => {
  FakeEventSource.last = null;
  FakeEventSource.opened = [];
  vi.stubGlobal("EventSource", FakeEventSource);
  // `settings` is a module singleton, so without this each case inherits
  // whatever address the previous one dialled.
  localStorage.clear();
  settings.value = { ...DEFAULTS };
  // Another module singleton, shared by the panel and the config screens.
  providersView.value = null;
  // Nothing here should reach the network. Without this the provider store
  // would happily talk to a real daemon on the developer's own machine, and
  // the suite would pass or fail depending on whether one was running.
  vi.stubGlobal("fetch", vi.fn(() => Promise.reject(new Error("no network in tests"))));
});

describe("App", () => {
  it("subscribes to the daemon's event stream on mount", () => {
    mount(App);
    expect(FakeEventSource.last?.url).toContain("/events");
  });

  it("says it is connecting before the first frame arrives", () => {
    const w = mount(App);
    expect(w.text()).toContain("Connecting");
  });

  it("renders a tile per provider once a snapshot arrives", async () => {
    const w = mount(App);
    FakeEventSource.last!.emit("snapshot", SNAPSHOT);
    await flushPromises();

    expect(w.findAll(".tile")).toHaveLength(2);
    expect(w.text()).toContain("Claude Code");
    expect(w.text()).toContain("OpenAI Codex");
    expect(w.text()).toContain("8%");
    expect(w.text()).toContain("88%");
  });

  it("headlines the most constrained meter, which is what the tray badge shows", async () => {
    const w = mount(App);
    FakeEventSource.last!.emit("snapshot", SNAPSHOT);
    await flushPromises();
    // Codex at 88% (warning) outranks Claude at 8%.
    expect(w.find(".bar strong").text()).toContain("OpenAI Codex");
  });

  it("keeps the last reading on screen when the stream drops", async () => {
    const w = mount(App);
    FakeEventSource.last!.emit("snapshot", SNAPSHOT);
    await flushPromises();
    FakeEventSource.last!.emit("error");
    await flushPromises();

    // Blanking the panel on a dropped connection would lose data we still have.
    expect(w.text()).toContain("Claude Code");
    expect(w.text()).toContain("Lost the connection");
    expect(w.find(".dot").classes()).toContain("offline");
  });

  it("distinguishes never-connected from lost-connection", async () => {
    const w = mount(App);
    FakeEventSource.last!.emit("error");
    await flushPromises();
    // These need different fixes, so they must not look the same.
    expect(w.text()).toContain("Cannot reach uwd");
  });

  it("opens the provider list from the header, and closes it again", async () => {
    const w = mount(App);
    providersView.value = CONFIGURED;
    FakeEventSource.last!.emit("snapshot", SNAPSHOT);
    await flushPromises();

    await w.find(".gear").trigger("click");
    expect(w.text()).toContain("Providers");
    // The tiles are replaced, not merely covered.
    expect(w.findAll(".tile")).toHaveLength(0);

    await w.find(".bar .link").trigger("click");
    expect(w.findAll(".tile")).toHaveLength(2);
  });

  it("offers the settings screen from the cannot-reach state, where the fix lives", async () => {
    const w = mount(App);
    FakeEventSource.last!.emit("error");
    await flushPromises();

    expect(w.text()).toContain("Cannot reach uwd");
    await w.find(".empty .link").trigger("click");
    expect(w.text()).toContain("Address");
  });

  it("offers to add a provider when none is configured", async () => {
    // The first screen anyone sees. A panel that is merely blank reads as
    // broken, and the one useful thing to do from here is add something.
    const w = mount(App);
    providersView.value = CATALOGUE;
    FakeEventSource.last!.emit("snapshot", { generated_at: new Date().toISOString(), providers: [] });
    await flushPromises();

    expect(w.text()).toContain("Nothing is being watched yet");
    await w.find(".welcome .primary").trigger("click");
    expect(w.text()).toContain("Add a provider");
  });

  it("builds the add screen from the daemon's manifest, not from a hard-coded list", async () => {
    const w = mount(App);
    providersView.value = CATALOGUE;
    FakeEventSource.last!.emit("snapshot", { generated_at: new Date().toISOString(), providers: [] });
    await flushPromises();

    await w.find(".welcome .primary").trigger("click");
    expect(w.text()).toContain("Claude Code");
    expect(w.text()).toContain("Session and weekly windows.");

    // Two usable methods, so it asks rather than guessing. (With only one it
    // goes straight there — confirming a choice you had no alternative to is
    // a wasted screen.)
    await w.find(".card").trigger("click");
    expect(w.text()).toContain("Sign in with your browser");
    // An unavailable method is shown with its reason rather than hidden —
    // "why can I not borrow the CLI token" is the question it answers.
    expect(w.text()).toContain("not signed in on this machine");
    expect(w.findAll("button.method.off")).toHaveLength(1);
  });

  it("picks up a provider list pushed over the stream", async () => {
    // How a browser sign-in lands on screen: it completes on the daemon long
    // after the request that started it was answered.
    const w = mount(App);
    FakeEventSource.last!.emit("snapshot", SNAPSHOT);
    await flushPromises();
    await w.find(".gear").trigger("click");

    FakeEventSource.last!.emit("providers", CONFIGURED);
    await flushPromises();

    expect(w.text()).toContain("Claude Code");
    expect(w.findAll(".row")).toHaveLength(1);
  });

  it("marks each row with the provider's own icon from the manifest", async () => {
    const w = mount(App);
    providersView.value = CONFIGURED;
    FakeEventSource.last!.emit("snapshot", SNAPSHOT);
    await flushPromises();
    await w.find(".gear").trigger("click");

    // Four rows of similar-length names are read one by one; four marks are
    // recognised at a glance. The `src` must come from the manifest — binding
    // a field the daemon does not send renders a broken image, and the row
    // still looks fine in a passing test that only checks the tag is there.
    const icon = w.find(".row .icon");
    expect(icon.exists()).toBe(true);
    expect(icon.attributes("src")).toBe(ICON);
  });

  it("asks before deleting a credential, which cannot be undone", async () => {
    const w = mount(App);
    providersView.value = CONFIGURED;
    FakeEventSource.last!.emit("snapshot", SNAPSHOT);
    await flushPromises();
    await w.find(".gear").trigger("click");

    await w.find(".trash").trigger("click");
    expect(w.text()).toContain("delete its stored credential");
    // Still there: the click armed the confirmation, it did not act.
    expect(w.findAll(".row")).toHaveLength(1);

    await w.findAll("button").find((b) => b.text() === "Cancel")!.trigger("click");
    expect(w.text()).not.toContain("delete its stored credential");
  });

  it("redials when the daemon address changes", async () => {
    const w = mount(App);
    FakeEventSource.last!.emit("snapshot", SNAPSHOT);
    await flushPromises();
    const first = FakeEventSource.last!;
    expect(first.url).toContain("127.0.0.1:7878");

    settings.value = { url: "http://100.64.0.1:7878", token: "tok" };
    await flushPromises();

    // A phone that has just been told the right address must not keep serving
    // the old stream, and must not leak it either.
    expect(first.closed).toBe(true);
    expect(FakeEventSource.opened).toHaveLength(2);
    expect(FakeEventSource.last!.url).toBe("http://100.64.0.1:7878/events?token=tok");
    w.unmount();
  });

  it("forgets it ever connected when pointed at a different daemon", async () => {
    const w = mount(App);
    FakeEventSource.last!.emit("snapshot", SNAPSHOT);
    await flushPromises();

    settings.value = { url: "http://elsewhere:7878", token: "" };
    await flushPromises();
    FakeEventSource.last!.emit("error");
    await flushPromises();

    // Having reached *a* daemon once says nothing about this one, so this is
    // "cannot reach", not "lost the connection".
    expect(w.text()).toContain("Cannot reach uwd");
    expect(w.text()).not.toContain("Lost the connection");
  });

  it("closes the stream when unmounted rather than leaking it", () => {
    const w = mount(App);
    const source = FakeEventSource.last!;
    w.unmount();
    expect(source.closed).toBe(true);
  });
});
