import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import ProviderTile from "./ProviderTile.vue";
import type { Provider } from "../types/Provider";

const NOW = Date.parse("2026-08-19T06:00:00Z");

function tile(overrides: Partial<Provider> = {}) {
  const provider: Provider = {
    id: "claude",
    label: "Claude Code",
    plan: "max",
    status: { state: "ok" },
    auth: "own_grant",
    updated_at: "2026-08-19T05:59:00Z",
    meters: [
      {
        id: "session",
        label: "5-hour",
        kind: {
          type: "window",
          used_pct: 42,
          resets_at: "2026-08-19T10:00:00Z",
          window_mins: 300,
        },
        severity: "normal",
      },
    ],
    ...overrides,
  };
  return mount(ProviderTile, { props: { provider, now: NOW } });
}

describe("ProviderTile", () => {
  it("renders the meter with its bar and countdown", () => {
    const w = tile();
    expect(w.text()).toContain("5-hour");
    expect(w.text()).toContain("42%");
    expect(w.text()).toContain("4h 0m");
    // jsdom normalises `42.0%` to `42%` when it reflects the style back.
    expect(w.find(".fill").attributes("style")).toContain("width: 42%");
  });

  it("shows the error message instead of any number when a poll failed", () => {
    // A number beside an error reads as current, which is worse than no number.
    const w = tile({
      status: { state: "error", message: "not signed in to `claude`" },
      meters: [],
    });
    expect(w.text()).toContain("not signed in");
    expect(w.find(".track").exists()).toBe(false);
  });

  it("keeps showing the last reading when it is merely stale, but says so", () => {
    const w = tile({ status: { state: "stale", since: "2026-08-19T05:55:00Z" } });
    expect(w.text()).toContain("42%");
    expect(w.text()).toContain("Not updating");
    expect(w.find(".row").classes()).toContain("stale");
  });

  it("marks a borrowed credential, because it behaves differently on expiry", () => {
    const w = tile({ auth: "delegated" });
    expect(w.find(".badge").text()).toBe("borrowed");
  });

  it("carries severity onto the bar so colour comes from the daemon's scale", () => {
    const w = tile({
      meters: [
        {
          id: "session",
          label: "5-hour",
          kind: { type: "window", used_pct: 97, resets_at: null, window_mins: null },
          severity: "critical",
        },
      ],
    });
    expect(w.find(".track").classes()).toContain("critical");
  });
});
