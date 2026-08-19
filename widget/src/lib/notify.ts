/**
 * Threshold notifications, in whichever runtime we happen to be in.
 *
 * The plain web `Notification` API is not available inside a Tauri v2 webview —
 * notifications go through a plugin over IPC instead — but the same build has
 * to keep working in an ordinary browser during `npm run dev` and as the
 * eventual PWA. The plugin is therefore imported dynamically and only on the
 * path that needs it, so a browser never loads it and a Tauri build never
 * reaches for an API that isn't there.
 */
const IN_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function notify(title: string, body: string): Promise<void> {
  if (IN_TAURI) {
    const { isPermissionGranted, requestPermission, sendNotification } = await import(
      "@tauri-apps/plugin-notification"
    );
    const granted = (await isPermissionGranted()) || (await requestPermission()) === "granted";
    if (granted) sendNotification({ title, body });
    return;
  }

  if ("Notification" in window && Notification.permission === "granted") {
    new Notification(title, { body });
  }
}

/**
 * Browsers only grant permission from a user gesture, so this is called on the
 * first click rather than at startup. Under Tauri the plugin handles it.
 */
export function requestBrowserPermission(): void {
  if (!IN_TAURI && "Notification" in window && Notification.permission === "default") {
    void Notification.requestPermission();
  }
}
