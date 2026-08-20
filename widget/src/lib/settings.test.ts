import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The module holds a singleton loaded from `localStorage` at import time, so
 * each case has to start from a fresh module graph or it inherits the previous
 * one's state.
 */
async function load() {
  vi.resetModules();
  return import("./settings");
}

beforeEach(() => {
  localStorage.clear();
});

describe("settings", () => {
  it("falls back to whatever address the app discovered", async () => {
    // The stored URL is empty by design — "auto". The desktop shell starts a
    // daemon on whatever port is free and reports the address at boot, so a
    // hard-coded 7878 in here would send the UI to a dead port.
    const { settings, daemonUrl } = await load();
    expect(settings.value.url).toBe("");
    expect(daemonUrl("/events")).toBe("http://127.0.0.1:7878/events");
  });

  it("uses the discovered address until the user overrides it", async () => {
    const { setAutoUrl, settings, daemonUrl } = await load();
    setAutoUrl("http://127.0.0.1:49213");
    expect(daemonUrl("/snapshot")).toBe("http://127.0.0.1:49213/snapshot");

    // An explicit address always wins — that is the remote-daemon case.
    settings.value = { url: "http://100.64.0.1:7878", token: "" };
    expect(daemonUrl("/snapshot")).toBe("http://100.64.0.1:7878/snapshot");
  });

  it("sends the token as a header for fetch, which can set one", async () => {
    const { authHeaders, settings } = await load();
    expect(authHeaders()).toEqual({});
    settings.value = { url: "", token: "  s3cret  " };
    // Not in the query string: a token in a URL ends up in devtools, in logs,
    // and in whatever the user pastes into a bug report.
    expect(authHeaders()).toEqual({ authorization: "Bearer s3cret" });
  });

  it("appends the token as a query parameter, since EventSource cannot set headers", async () => {
    const { settings, daemonUrl } = await load();
    settings.value = { url: "http://100.64.0.1:7878", token: "s3 cret/&" };
    expect(daemonUrl("/events")).toBe("http://100.64.0.1:7878/events?token=s3%20cret%2F%26");
  });

  it("trims trailing slashes, which are the usual paste error", async () => {
    const { settings, daemonUrl } = await load();
    settings.value = { url: "http://host:7878///", token: "" };
    // Not "http://host:7878///events".
    expect(daemonUrl("/snapshot")).toBe("http://host:7878/snapshot");
  });

  it("ignores surrounding whitespace in both fields", async () => {
    const { settings, daemonUrl } = await load();
    settings.value = { url: "  http://host:7878  ", token: "  tok  " };
    expect(daemonUrl("/events")).toBe("http://host:7878/events?token=tok");
  });

  it("persists across a reload", async () => {
    const first = await load();
    first.settings.value = { url: "http://phone:7878", token: "abc" };
    // The watcher writes on the microtask after the mutation.
    await Promise.resolve();

    const second = await load();
    expect(second.settings.value.url).toBe("http://phone:7878");
    expect(second.settings.value.token).toBe("abc");
  });

  it("survives a corrupt stored value rather than starting blank", async () => {
    localStorage.setItem("usage-watcher.daemon", "{not json");
    const { daemonUrl } = await load();
    expect(daemonUrl("/events")).toBe("http://127.0.0.1:7878/events");
  });

  it("fills in a missing field rather than trusting a partial record", async () => {
    // An older build, or a hand-edited entry.
    localStorage.setItem("usage-watcher.daemon", JSON.stringify({ token: "kept" }));
    const { daemonUrl, settings } = await load();
    expect(daemonUrl("/snapshot")).toBe("http://127.0.0.1:7878/snapshot?token=kept");
    expect(settings.value.token).toBe("kept");
  });

  it("keeps working when localStorage throws, as it does in private mode", async () => {
    const spy = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    const { daemonUrl } = await load();
    expect(daemonUrl("/events")).toBe("http://127.0.0.1:7878/events");
    spy.mockRestore();
  });
});
