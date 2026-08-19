import { ref, shallowRef, onUnmounted, type Ref } from "vue";
import type { Snapshot } from "../types/Snapshot";
import type { Alert } from "../types/Alert";

const BASE = (import.meta.env.VITE_UWD_URL ?? "http://127.0.0.1:7878").replace(/\/$/, "");
const TOKEN = import.meta.env.VITE_UWD_TOKEN ?? "";

/** `EventSource` cannot set headers, so the token travels as a query param. */
function url(path: string): string {
  return TOKEN ? `${BASE}${path}?token=${encodeURIComponent(TOKEN)}` : `${BASE}${path}`;
}

export type Connection = "connecting" | "live" | "offline";

export interface DaemonFeed {
  snapshot: Ref<Snapshot | null>;
  connection: Ref<Connection>;
  /** Set once we have been live at least once, to tell "starting up" from "lost it". */
  everConnected: Ref<boolean>;
}

/**
 * Subscribe to the daemon's event stream.
 *
 * The widget deliberately has no timer of its own: `uwd` decides the poll
 * rhythm and pushes, so the two can never disagree about how fresh the numbers
 * are, and a backgrounded window costs nothing. `EventSource` reconnects by
 * itself, and the daemon replays the current snapshot as its first frame, so a
 * dropped connection heals without any code here.
 */
export function useDaemon(onAlert?: (a: Alert) => void): DaemonFeed {
  const snapshot = shallowRef<Snapshot | null>(null);
  const connection = ref<Connection>("connecting");
  const everConnected = ref(false);

  const source = new EventSource(url("/events"));

  source.addEventListener("open", () => {
    connection.value = "live";
    everConnected.value = true;
  });

  source.addEventListener("snapshot", (e) => {
    snapshot.value = JSON.parse((e as MessageEvent).data) as Snapshot;
    connection.value = "live";
    everConnected.value = true;
  });

  source.addEventListener("alert", (e) => {
    onAlert?.(JSON.parse((e as MessageEvent).data) as Alert);
  });

  source.addEventListener("error", () => {
    // Fired both while retrying and on a hard failure; either way we are not
    // receiving. The last snapshot stays on screen, marked stale by the UI.
    connection.value = "offline";
  });

  onUnmounted(() => source.close());

  return { snapshot, connection, everConnected };
}

/** A ticking clock, so countdowns move without the daemon having to push. */
export function useNow(intervalMs = 1000): Ref<number> {
  const now = ref(Date.now());
  const id = window.setInterval(() => (now.value = Date.now()), intervalMs);
  onUnmounted(() => window.clearInterval(id));
  return now;
}
