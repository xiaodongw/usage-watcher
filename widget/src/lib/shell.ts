/**
 * The bits of the native shell the config flow needs, each degrading to a
 * sensible no-op in a browser.
 *
 * Everything is imported dynamically for the same reason the tray and autostart
 * helpers are: `npm run dev` in an ordinary browser must not try to load a
 * Tauri module, and the mobile builds do not compile every plugin.
 */
const IN_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const isDesktopShell = IN_TAURI;

/**
 * Where the app found a daemon.
 *
 * The desktop build starts one in-process, on the configured port or an
 * ephemeral one if that is taken, so the address is not known until runtime.
 * `null` means "no shell to ask" — a browser, where the conventional port is
 * the best guess and the user can override it in settings.
 */
export async function discoverDaemonUrl(): Promise<string | null> {
  if (!IN_TAURI) return null;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<string>("daemon_url");
  } catch {
    return null;
  }
}

/**
 * Open a URL in the user's real browser.
 *
 * It has to be the real browser, not this webview: the consent page needs the
 * session you are already signed in to, and the webview has no cookies. In a
 * browser build `window.open` is the same thing.
 */
export async function openExternal(url: string): Promise<boolean> {
  if (!IN_TAURI) {
    return window.open(url, "_blank", "noopener,noreferrer") !== null;
  }
  try {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
    return true;
  } catch {
    // Every screen that calls this also shows the URL, so a failure here costs
    // a click rather than the flow.
    return false;
  }
}

/**
 * Whether the panel is acting as a tray popover or as a settings window.
 *
 * The two want opposite behaviour, and the difference is not cosmetic. A
 * popover hides the moment it loses focus — which is exactly right until you
 * are half way through a browser login, at which point clicking the browser
 * would dismiss the thing waiting for you to come back.
 */
export type PanelMode = "panel" | "config";

export async function setPanelMode(mode: PanelMode): Promise<void> {
  if (!IN_TAURI) return;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("set_panel_mode", { mode });
  } catch {
    // Mobile: one window, always full screen, nothing to resize.
  }
}

/** Fires when the tray menu asks for a screen. Returns an unlisten function. */
export async function onNavigate(fn: (view: string) => void): Promise<() => void> {
  if (!IN_TAURI) return () => {};
  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen<string>("navigate", (e) => fn(e.payload));
  } catch {
    return () => {};
  }
}
