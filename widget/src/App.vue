<script setup lang="ts">
import { computed } from "vue";
import ProviderTile from "./components/ProviderTile.vue";
import { useDaemon, useNow } from "./lib/daemon";
import { ago, mostConstrained } from "./lib/format";
import type { Alert } from "./types/Alert";

const now = useNow();

/**
 * Threshold crossings arrive as their own event so the widget does not have to
 * diff snapshots to notice one. In the browser this is a `Notification`; under
 * Tauri the same handler will call the native notification plugin.
 */
function notify(alert: Alert) {
  if (!("Notification" in window) || Notification.permission !== "granted") return;
  new Notification("Usage Watcher", { body: alert.message });
}

const { snapshot, connection, everConnected } = useDaemon(notify);

const providers = computed(() => snapshot.value?.providers ?? []);
const headline = computed(() => mostConstrained(providers.value));

function askForNotifications() {
  if ("Notification" in window && Notification.permission === "default") {
    void Notification.requestPermission();
  }
}
</script>

<template>
  <main @click="askForNotifications">
    <header class="bar">
      <span class="dot" :class="connection" :title="`daemon ${connection}`" />
      <strong v-if="headline">
        {{ headline.provider.label }} · {{ headline.meter.label }}
      </strong>
      <strong v-else>Usage Watcher</strong>
      <span class="spacer" />
      <span v-if="snapshot" class="when">{{ ago(snapshot.generated_at, now) }}</span>
    </header>

    <!-- Three distinct empty states. "Waiting" and "cannot reach" look the
         same on screen otherwise, and they need completely different fixes. -->
    <p v-if="!snapshot && connection === 'connecting'" class="empty">Connecting to uwd…</p>
    <p v-else-if="!snapshot && !everConnected" class="empty">
      Cannot reach uwd. Start it with <code>uwd</code>, or set
      <code>VITE_UWD_URL</code> if it runs elsewhere.
    </p>
    <p v-else-if="providers.length === 0" class="empty">No providers enabled.</p>

    <ProviderTile
      v-for="provider in providers"
      :key="provider.id"
      :provider="provider"
      :now="now"
    />

    <p v-if="snapshot && connection === 'offline'" class="offline">
      Lost the connection to uwd — retrying.
    </p>
  </main>
</template>

<style scoped>
main {
  min-width: 18rem;
  max-width: 26rem;
}

.bar {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.45rem 0.75rem;
  border-bottom: 1px solid var(--rule);
  font-size: 0.72rem;
  color: var(--fg-dim);
  /* The panel is dragged by its title bar under Tauri. */
  -webkit-user-select: none;
  user-select: none;
}

.bar strong {
  font-weight: 600;
  color: var(--fg);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.spacer {
  flex: 1;
}

.when {
  color: var(--fg-faint);
  font-variant-numeric: tabular-nums;
}

.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex: none;
  background: var(--fg-faint);
}

.dot.live {
  background: var(--ok);
}

.dot.connecting {
  background: var(--warn);
}

.dot.offline {
  background: var(--crit);
}

.empty,
.offline {
  margin: 0;
  padding: 1rem 0.75rem;
  font-size: 0.72rem;
  color: var(--fg-dim);
  line-height: 1.5;
}

.offline {
  padding: 0.5rem 0.75rem;
  color: var(--crit);
  border-top: 1px solid var(--rule);
}

code {
  font-size: 0.68rem;
  background: var(--track);
  border-radius: 3px;
  padding: 0.05rem 0.25rem;
}
</style>
