import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import ProviderList from "./ProviderList.vue";
import { api } from "../lib/api";
import { providersView } from "../lib/providers";
import type { ConfiguredProvider } from "../types/ConfiguredProvider";
import type { ProvidersView } from "../types/ProvidersView";

vi.mock("../lib/api", () => ({
  api: { reorder: vi.fn(), remove: vi.fn() },
}));

const ICON = "data:image/png;base64,iVBORw0KGgo=";

function row(id: string, label: string): ConfiguredProvider {
  return { id, label, icon: ICON, auth: "own", enabled: true, signed_in: true };
}

function view(...ids: string[]): ProvidersView {
  return { catalogue: [], configured: ids.map((id) => row(id, id.toUpperCase())) };
}

/** Row height in the fake layout, so a midpoint is a number a test can name. */
const H = 40;

/**
 * jsdom lays nothing out — every rect is zero — and the drop position is
 * computed entirely from rects. Without this the whole feature would be
 * untestable here, which is the same as untested: none of this code runs in
 * WSL, where the app cannot even be compiled.
 */
function fakeLayout() {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (
    this: Element,
  ) {
    const siblings = Array.from(this.parentElement?.children ?? []);
    const i = Math.max(0, siblings.indexOf(this));
    return {
      top: i * H,
      bottom: (i + 1) * H,
      height: H,
      left: 0,
      right: 340,
      width: 340,
      x: 0,
      y: i * H,
      toJSON: () => ({}),
    } as DOMRect;
  });
}

/** Midpoint of row `i` in that fake layout. */
const middleOf = (i: number) => i * H + H / 2;

enableAutoUnmount(afterEach);

beforeEach(() => {
  vi.restoreAllMocks();
  fakeLayout();
  // jsdom has no pointer capture, and the drag would die on the first call.
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.releasePointerCapture = vi.fn();
  providersView.value = view("claude", "codex", "openrouter");
  vi.mocked(api.reorder).mockImplementation((ids: string[]) =>
    Promise.resolve(view(...ids)),
  );
});

function labels(w: ReturnType<typeof mount>) {
  return w.findAll("li.row strong").map((n) => n.text());
}

async function drag(w: ReturnType<typeof mount>, from: number, toY: number) {
  const grip = w.findAll(".grip")[from];
  await grip.trigger("pointerdown", { button: 0, clientY: middleOf(from), pointerId: 1 });
  await grip.trigger("pointermove", { clientY: toY, pointerId: 1 });
  await grip.trigger("pointerup", { clientY: toY, pointerId: 1 });
  await flushPromises();
}

describe("ProviderList reordering", () => {
  it("gives every row a handle to drag it by", () => {
    const w = mount(ProviderList);
    expect(w.findAll(".grip")).toHaveLength(3);
    // Named, because a bare handle tells a screen reader nothing about which
    // row it would move.
    expect(w.findAll(".grip")[0].attributes("aria-label")).toContain("CLAUDE");
  });

  it("drops a row where the pointer left it and saves that order", async () => {
    const w = mount(ProviderList);
    // Past the middle of the third row: the first row lands at the end.
    await drag(w, 0, middleOf(2) + 5);

    expect(api.reorder).toHaveBeenCalledWith(["codex", "openrouter", "claude"]);
    expect(labels(w)).toEqual(["CODEX", "OPENROUTER", "CLAUDE"]);
  });

  it("moves a row up as well as down", async () => {
    const w = mount(ProviderList);
    await drag(w, 2, middleOf(0) - 5);

    expect(api.reorder).toHaveBeenCalledWith(["openrouter", "claude", "codex"]);
  });

  it("does not save when the row is put back where it came from", async () => {
    // A click on the handle is a drag of zero distance, and it must not write
    // the config file or repaint every open panel.
    const w = mount(ProviderList);
    await drag(w, 1, middleOf(1) + 2);

    expect(api.reorder).not.toHaveBeenCalled();
    expect(labels(w)).toEqual(["CLAUDE", "CODEX", "OPENROUTER"]);
  });

  it("abandons the drag on Escape", async () => {
    const w = mount(ProviderList);
    const grip = w.findAll(".grip")[0];
    await grip.trigger("pointerdown", { button: 0, clientY: middleOf(0), pointerId: 1 });
    await grip.trigger("pointermove", { clientY: middleOf(2) + 5, pointerId: 1 });

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await flushPromises();
    await grip.trigger("pointerup", { clientY: middleOf(2) + 5, pointerId: 1 });
    await flushPromises();

    expect(api.reorder).not.toHaveBeenCalled();
    expect(labels(w)).toEqual(["CLAUDE", "CODEX", "OPENROUTER"]);
  });

  it("shows where the row would land while it is being dragged", async () => {
    const w = mount(ProviderList);
    const grip = w.findAll(".grip")[0];
    await grip.trigger("pointerdown", { button: 0, clientY: middleOf(0), pointerId: 1 });
    await grip.trigger("pointermove", { clientY: middleOf(1) + 5, pointerId: 1 });

    expect(w.findAll("li.row")[0].classes()).toContain("lifted");
    // Between rows 1 and 2, i.e. drawn above the row currently at index 2.
    expect(w.findAll("li.row")[2].classes()).toContain("drop-above");
  });

  it("moves a row with the arrow keys, for anyone not using a pointer", async () => {
    const w = mount(ProviderList);
    await w.findAll(".grip")[0].trigger("keydown", { key: "ArrowDown" });
    await flushPromises();

    expect(api.reorder).toHaveBeenCalledWith(["codex", "claude", "openrouter"]);
    expect(labels(w)).toEqual(["CODEX", "CLAUDE", "OPENROUTER"]);
  });

  it("will not walk a row off either end of the list", async () => {
    const w = mount(ProviderList);
    await w.findAll(".grip")[0].trigger("keydown", { key: "ArrowUp" });
    await w.findAll(".grip")[2].trigger("keydown", { key: "ArrowDown" });
    await flushPromises();

    expect(api.reorder).not.toHaveBeenCalled();
  });

  it("keeps showing the daemon's order when the save fails", async () => {
    vi.mocked(api.reorder).mockRejectedValue(new Error("uwd said no"));
    const w = mount(ProviderList);
    await drag(w, 0, middleOf(2) + 5);

    // The optimistic order is dropped rather than left on screen pretending to
    // be saved — a panel that disagrees with the config file is worse than one
    // that snaps back.
    expect(labels(w)).toEqual(["CLAUDE", "CODEX", "OPENROUTER"]);
    expect(w.text()).toContain("uwd said no");
  });

  it("ignores a drag that did not start with the left button", async () => {
    const w = mount(ProviderList);
    const grip = w.findAll(".grip")[0];
    await grip.trigger("pointerdown", { button: 2, clientY: middleOf(0), pointerId: 1 });
    await grip.trigger("pointermove", { clientY: middleOf(2) + 5, pointerId: 1 });
    await grip.trigger("pointerup", { clientY: middleOf(2) + 5, pointerId: 1 });
    await flushPromises();

    expect(api.reorder).not.toHaveBeenCalled();
  });
});
