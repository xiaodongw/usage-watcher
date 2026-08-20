import { ref, shallowRef, onUnmounted, watch, type Ref } from "vue";
import type { Snapshot } from "../types/Snapshot";
import type { Alert } from "../types/Alert";
import { daemonUrl, settings } from "./settings";
import { applyProviders, refreshProviders } from "./providers";
import type { ProvidersView } from "../types/ProvidersView";

export type Connection = "connecting" | "live" | "offline";

export interface DaemonFeed {
  snapshot: Ref<Snapshot | null>;
  connection: Ref<Connection>;
  /** Set once we have been live at least once, to tell "starting up" from "lost it". */
  everConnected: Ref<boolean>;
  /** Drop the stream and dial again — for the settings screen's "Reconnect". */
  reconnect: () => void;
}

/**
 * Subscribe to the daemon's event stream.
 *
 * The widget deliberately has no timer of its own: `uwd` decides the poll
 * rhythm and pushes, so the two can never disagree about how fresh the numbers
 * are, and a backgrounded window costs nothing. `EventSource` reconnects by
 * itself, and the daemon replays the current snapshot as its first frame, so a
 * dropped connection heals without any code here.
 *
 * The address is read at connect time rather than captured at module load, so
 * editing it in settings takes effect at once. That matters most on a phone,
 * where the address is the only thing standing between a blank screen and
 * working — and where there is no way to restart the app "with a new env var".
 */
export function useDaemon(onAlert?: (a: Alert) => void): DaemonFeed {
  const snapshot = shallowRef<Snapshot | null>(null);
  const connection = ref<Connection>("connecting");
  const everConnected = ref(false);

  let source: EventSource | null = null;

  function close() {
    source?.close();
    source = null;
  }

  function open() {
    close();
    connection.value = "connecting";

    source = new EventSource(daemonUrl("/events"));

    source.addEventListener("open", () => {
      connection.value = "live";
      everConnected.value = true;
      // The provider list is not part of the snapshot — it is configuration
      // rather than data — so it has to be fetched once per connection. Doing
      // it here rather than on mount means reconnecting to a *different*
      // daemon picks up that daemon's providers.
      void refreshProviders();
    });

    source.addEventListener("snapshot", (e) => {
      snapshot.value = JSON.parse((e as MessageEvent).data) as Snapshot;
      connection.value = "live";
      everConnected.value = true;
    });

    source.addEventListener("alert", (e) => {
      onAlert?.(JSON.parse((e as MessageEvent).data) as Alert);
    });

    // Pushed whenever a provider is added, removed, or finishes signing in.
    // The last of those is the one that matters: a browser login completes on
    // the daemon minutes after the request that started it returned, and this
    // is the only way the config screen hears about it.
    source.addEventListener("providers", (e) => {
      applyProviders(JSON.parse((e as MessageEvent).data) as ProvidersView);
    });

    source.addEventListener("error", () => {
      // Fired both while retrying and on a hard failure; either way we are not
      // receiving. The last snapshot stays on screen, marked stale by the UI.
      connection.value = "offline";
    });
  }

  open();

  // Editing the address must dial the new one, not keep serving the old.
  // `everConnected` is reset too: having reached a *different* daemon once
  // says nothing about this one, and it is what distinguishes "starting up"
  // from "lost it" on screen.
  watch(
    () => daemonUrl("/events"),
    () => {
      everConnected.value = false;
      snapshot.value = null;
      open();
    },
  );

  onUnmounted(close);

  return { snapshot, connection, everConnected, reconnect: open };
}

/** A ticking clock, so countdowns move without the daemon having to push. */
export function useNow(intervalMs = 1000): Ref<number> {
  const now = ref(Date.now());
  const id = window.setInterval(() => (now.value = Date.now()), intervalMs);
  onUnmounted(() => window.clearInterval(id));
  return now;
}

/** Re-exported so views do not each import the settings module separately. */
export { settings };
