<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import ProviderTile from "./components/ProviderTile.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import AddProvider from "./views/AddProvider.vue";
import LoginFlow from "./views/LoginFlow.vue";
import ProviderList from "./views/ProviderList.vue";
import { useDaemon, useNow } from "./lib/daemon";
import { ago, mostConstrained } from "./lib/format";
import { notify, requestBrowserPermission } from "./lib/notify";
import { catalogue, configured, isEmpty } from "./lib/providers";
import { onNavigate, setPanelMode } from "./lib/shell";
import { updateTray } from "./lib/tray";
import type { Alert } from "./types/Alert";
import type { AuthMethod } from "./types/AuthMethod";
import type { ProviderInfo } from "./types/ProviderInfo";

const now = useNow();

// Threshold crossings arrive as their own event, so the widget never has to
// diff snapshots to notice one.
function onAlert(alert: Alert) {
  void notify("Usage Watcher", alert.message);
}

const { snapshot, connection, everConnected } = useDaemon(onAlert);

const providers = computed(() => snapshot.value?.providers ?? []);
const headline = computed(() => mostConstrained(providers.value));

/**
 * Which screen is showing.
 *
 * A ref rather than a router: there are five screens, they nest one level deep,
 * and a routing library in a 340px popover with no address bar would be
 * machinery for its own sake.
 */
type View = "panel" | "providers" | "add" | "login" | "settings";
const view = ref<View>("panel");

/** The provider and method a login is running for. */
const pending = ref<{ provider: ProviderInfo; method: AuthMethod } | null>(null);

function choose(provider: ProviderInfo, method: AuthMethod) {
  pending.value = { provider, method };
  view.value = "login";
}

/** Sign in again to something already on the list. */
function signIn(id: string) {
  const provider = catalogue.value.find((p) => p.id === id);
  const method = provider?.methods.find((m) => m.auth === "own");
  if (provider && method) choose(provider, method);
}

/**
 * A tray popover hides when it loses focus. That is right for the panel and
 * wrong for everything else here — clicking the browser mid-login would dismiss
 * the very screen waiting for you to come back — so the shell is told which
 * mode it is in.
 */
watch(
  view,
  (v) => void setPanelMode(v === "panel" ? "panel" : "config"),
  { immediate: true },
);

// The tray icon carries the headline so it can be read without opening
// anything — the whole reason for a tray widget. A no-op off the desktop.
watch(
  [providers, connection],
  () => void updateTray(providers.value, connection.value === "live"),
  { immediate: true },
);

let unlisten: (() => void) | undefined;
onMounted(async () => {
  unlisten = await onNavigate((to) => (view.value = to as View));
});
onUnmounted(() => unlisten?.());
</script>

<template>
  <main @click="requestBrowserPermission">
    <SettingsPanel v-if="view === 'settings'" @close="view = 'providers'" />

    <ProviderList
      v-else-if="view === 'providers'"
      @close="view = 'panel'"
      @add="view = 'add'"
      @settings="view = 'settings'"
      @sign-in="signIn"
    />

    <AddProvider v-else-if="view === 'add'" @close="view = 'providers'" @choose="choose" />

    <LoginFlow
      v-else-if="view === 'login' && pending"
      :provider="pending.provider"
      :method="pending.method"
      @done="view = 'providers'"
      @cancel="view = 'providers'"
    />

    <template v-else>
      <header class="bar">
        <span class="dot" :class="connection" :title="`daemon ${connection}`" />
        <strong v-if="headline">
          {{ headline.provider.label }} · {{ headline.meter.label }}
        </strong>
        <strong v-else>Usage Watcher</strong>
        <span class="spacer" />
        <span v-if="snapshot" class="when">{{ ago(snapshot.generated_at, now) }}</span>
        <button class="gear" title="Configure" @click.stop="view = 'providers'">⚙</button>
      </header>

      <!-- Four distinct empty states. They look alike on screen and need
           completely different fixes, so each says which one it is. -->
      <p v-if="!snapshot && connection === 'connecting'" class="empty">Connecting to uwd…</p>
      <p v-else-if="!snapshot && !everConnected" class="empty">
        Cannot reach uwd. Start it with <code>uwd</code>, or
        <button class="link" @click.stop="view = 'settings'">set its address</button>
        if it runs elsewhere.
      </p>

      <!-- The first screen on a fresh install: nothing is being watched, and
           the only thing worth offering is the way to fix that. -->
      <div v-else-if="isEmpty" class="welcome">
        <p>Nothing is being watched yet.</p>
        <button class="primary" @click.stop="view = 'add'">Add provider</button>
      </div>

      <p v-else-if="providers.length === 0 && configured.length > 0" class="empty">
        Waiting for the first reading…
      </p>

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

.welcome {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.7rem;
  padding: 1.6rem 0.75rem 1.8rem;
  text-align: center;
}

.welcome p {
  margin: 0;
  font-size: 0.75rem;
  color: var(--fg-dim);
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

.primary {
  font: inherit;
  font-size: 0.75rem;
  cursor: pointer;
  border-radius: 6px;
  min-height: 2.1rem;
  padding: 0 1rem;
  background: var(--track);
  color: var(--fg);
  border: 1px solid var(--rule);
}

.primary:hover {
  border-color: var(--fg-faint);
}
</style>
