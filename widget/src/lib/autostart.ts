/**
 * Start at login.
 *
 * A tray widget you have to remember to launch is a tray widget you stop
 * using, so this is worth a toggle — but it only means anything on the desktop.
 * A browser cannot register a login item, and on Android and iOS the platform
 * decides what runs at boot, so everything here degrades to "unsupported"
 * rather than throwing.
 *
 * The plugin is imported dynamically for the same reason the notification one
 * is: a plain `npm run dev` in a browser must not try to load a Tauri module.
 */
const IN_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** `null` when the platform has no such concept — render no toggle at all. */
export async function isAutostartEnabled(): Promise<boolean | null> {
  if (!IN_TAURI) return null;
  try {
    const { isEnabled } = await import("@tauri-apps/plugin-autostart");
    return await isEnabled();
  } catch {
    // Mobile, where the plugin is not compiled in.
    return null;
  }
}

/** Returns the state actually achieved, which may not be the one requested. */
export async function setAutostart(on: boolean): Promise<boolean | null> {
  if (!IN_TAURI) return null;
  try {
    const { enable, disable, isEnabled } = await import("@tauri-apps/plugin-autostart");
    if (on) await enable();
    else await disable();
    // Read back rather than assume: on Linux this writes a .desktop file, and
    // a read-only autostart directory fails quietly enough to be worth checking.
    return await isEnabled();
  } catch {
    return null;
  }
}
