import { describe, expect, it } from "vitest";
import type { Meter } from "../types/Meter";
import type { Provider } from "../types/Provider";
import { countdown, fill, mostConstrained, readout } from "./format";

const NOW = Date.parse("2026-08-19T06:00:00Z");

function window_(id: string, pct: number, resets: string | null = null): Meter {
  return {
    id,
    label: id,
    kind: { type: "window", used_pct: pct, resets_at: resets, window_mins: null },
    severity: pct >= 95 ? "critical" : pct >= 80 ? "warning" : "normal",
  };
}

function balance(amount: number, of_total: number | null, unlimited = false): Meter {
  return {
    id: "credits",
    label: "Credits",
    kind: { type: "balance", amount, currency: "USD", of_total, unlimited },
    severity: "normal",
  };
}

describe("countdown", () => {
  it("shows at most two units", () => {
    expect(countdown("2026-08-19T10:38:00Z", NOW)).toBe("4h 38m");
    expect(countdown("2026-08-24T01:00:00Z", NOW)).toBe("4d 19h");
    expect(countdown("2026-08-19T06:12:00Z", NOW)).toBe("12m");
  });

  it("does not go negative once the window has reset", () => {
    expect(countdown("2026-08-19T05:00:00Z", NOW)).toBe("now");
  });

  it("rounds sub-minute up rather than showing 0m", () => {
    expect(countdown("2026-08-19T06:00:30Z", NOW)).toBe("<1m");
  });

  it("has nothing to show for a meter with no reset", () => {
    expect(countdown(null, NOW)).toBeNull();
  });
});

describe("fill", () => {
  it("fills as a window is consumed", () => {
    expect(fill(window_("s", 25))).toBeCloseTo(0.25);
  });

  it("fills as a balance is *spent*, so a full bar always means trouble", () => {
    // $2 left of $10 is 80% gone: the bar must read 0.8, not 0.2. Both meter
    // kinds have to mean the same thing visually or the panel is unreadable.
    expect(fill(balance(2, 10))).toBeCloseTo(0.8);
  });

  it("shows no scale for an unlimited or unbounded balance", () => {
    expect(fill(balance(5, null))).toBe(0);
    expect(fill(balance(5, 10, true))).toBe(0);
  });

  it("clamps a provider reporting over 100%", () => {
    expect(fill(window_("s", 130))).toBe(1);
  });
});

describe("readout", () => {
  it("rounds percentages", () => {
    expect(readout(window_("s", 18.6))).toBe("19%");
  });

  it("says unlimited rather than showing a meaningless zero", () => {
    expect(readout(balance(0, null, true))).toBe("unlimited");
  });
});

function provider(id: string, meters: Meter[]): Provider {
  return {
    id,
    label: id,
    plan: null,
    status: { state: "ok" },
    auth: "own_grant",
    updated_at: "2026-08-19T06:00:00Z",
    meters,
  };
}

describe("mostConstrained", () => {
  it("prefers higher severity over a fuller bar", () => {
    const best = mostConstrained([
      provider("a", [window_("busy", 70)]),
      provider("b", [window_("hot", 81)]),
    ]);
    expect(best?.meter.id).toBe("hot");
  });

  it("breaks ties within a severity by how full the meter is", () => {
    const best = mostConstrained([
      provider("a", [window_("warm", 81)]),
      provider("b", [window_("warmer", 90)]),
    ]);
    expect(best?.meter.id).toBe("warmer");
  });

  it("has no answer when nothing reports a meter", () => {
    expect(mostConstrained([provider("a", [])])).toBeNull();
    expect(mostConstrained([])).toBeNull();
  });
});
