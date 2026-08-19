<script setup lang="ts">
import { computed, ref, watch } from "vue";
import ProviderTile from "./components/ProviderTile.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import { useDaemon, useNow } from "./lib/daemon";
import { ago, mostConstrained } from "./lib/format";
import { notify, requestBrowserPermission } from "./lib/notify";
import { updateTray } from "./lib/tray";
import type { Alert } from "./types/Alert";

const now = useNow();

// Threshold crossings arrive as their own event, so the widget never has to
// diff snapshots to notice one.
function onAlert(alert: Alert) {
  void notify("Usage Watcher", alert.message);
}

const { snapshot, connection, everConnected } = useDaemon(onAlert);

const providers = computed(() => snapshot.value?.providers ?? []);
const headline = computed(() => mostConstrained(providers.value));

const showSettings = ref(false);

// The tray icon carries the headline so it can be read without opening
// anything — the whole reason for a tray widget. A no-op off the desktop.
watch(
  [providers, connection],
  () => void updateTray(providers.value, connection.value === "live"),
  { immediate: true },
);
</script>

<template>
  <main @click="requestBrowserPermission">
    <SettingsPanel v-if="showSettings" @close="showSettings = false" />

    <template v-else>
    <header class="bar">
      <span class="dot" :class="connection" :title="`daemon ${connection}`" />
      <strong v-if="headline">
        {{ headline.provider.label }} · {{ headline.meter.label }}
      </strong>
      <strong v-else>Usage Watcher</strong>
      <span class="spacer" />
      <span v-if="snapshot" class="when">{{ ago(snapshot.generated_at, now) }}</span>
      <button class="gear" title="Daemon settings" @click.stop="showSettings = true">⚙</button>
    </header>

    <!-- Three distinct empty states. "Waiting" and "cannot reach" look the
         same on screen otherwise, and they need completely different fixes. -->
    <p v-if="!snapshot && connection === 'connecting'" class="empty">Connecting to uwd…</p>
    <p v-else-if="!snapshot && !everConnected" class="empty">
      Cannot reach uwd. Start it with <code>uwd</code>, or
      <button class="link" @click.stop="showSettings = true">set its address</button>
      if it runs elsewhere.
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
    </template>
  </main>
</template>

<style scoped>
main {
  min-width: 18rem;
  max-width: 26rem;
}

/* On a handset the window is whatever the screen is, so the panel stops being
   a popover and becomes the app. */
@media (max-width: 30rem) {
  main {
    min-width: 0;
    max-width: none;
    width: 100%;
  }

  /* Fingers, not a mouse pointer. */
  .gear {
    font-size: 1.05rem;
    padding: 0.35rem 0.45rem;
  }
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

.gear {
  font: inherit;
  font-size: 0.8rem;
  line-height: 1;
  background: none;
  border: none;
  color: var(--fg-faint);
  cursor: pointer;
  padding: 0.1rem 0.15rem;
}

.gear:hover {
  color: var(--fg);
}

.link {
  font: inherit;
  font-size: inherit;
  background: none;
  border: none;
  padding: 0;
  color: var(--fg);
  text-decoration: underline;
  cursor: pointer;
}
</style>
