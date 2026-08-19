import { beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import App from "./App.vue";
import type { Snapshot } from "./types/Snapshot";

/**
 * A stand-in for the browser's `EventSource`, so the whole path — stream to
 * composable to tiles — is exercised without a live daemon or a real browser.
 */
class FakeEventSource {
  static last: FakeEventSource | null = null;
  listeners: Record<string, ((e: unknown) => void)[]> = {};
  closed = false;

  constructor(public url: string) {
    FakeEventSource.last = this;
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

beforeEach(() => {
  FakeEventSource.last = null;
  vi.stubGlobal("EventSource", FakeEventSource);
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

  it("closes the stream when unmounted rather than leaking it", () => {
    const w = mount(App);
    const source = FakeEventSource.last!;
    w.unmount();
    expect(source.closed).toBe(true);
  });
});
