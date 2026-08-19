<script setup lang="ts">
import { onMounted, ref } from "vue";
import { DEFAULTS, settings } from "../lib/settings";
import { isAutostartEnabled, setAutostart } from "../lib/autostart";

const emit = defineEmits<{ close: [] }>();

// `null` means the platform has no login items — a browser, or a phone. The
// row is then not rendered at all, rather than shown as a switch that does
// nothing when tapped.
const autostart = ref<boolean | null>(null);
onMounted(async () => (autostart.value = await isAutostartEnabled()));

async function toggleAutostart(e: Event) {
  const wanted = (e.target as HTMLInputElement).checked;
  autostart.value = await setAutostart(wanted);
}

// Edited on a copy so an abandoned edit does not reconnect the stream on every
// keystroke — typing "100.64.0.1" would otherwise dial nine dead addresses.
const url = ref(settings.value.url);
const token = ref(settings.value.token);

function apply() {
  settings.value = { url: url.value.trim(), token: token.value.trim() };
  emit("close");
}

function reset() {
  url.value = DEFAULTS.url;
  token.value = DEFAULTS.token;
}
</script>

<template>
  <section class="settings">
    <header class="bar">
      <strong>Daemon</strong>
      <span class="spacer" />
      <button class="link" @click="emit('close')">Close</button>
    </header>

    <form @submit.prevent="apply">
      <label>
        <span>Address</span>
        <input
          v-model="url"
          type="url"
          inputmode="url"
          autocapitalize="off"
          autocorrect="off"
          spellcheck="false"
          placeholder="http://127.0.0.1:7878"
        />
      </label>

      <label>
        <span>Token</span>
        <input
          v-model="token"
          type="password"
          autocapitalize="off"
          autocorrect="off"
          spellcheck="false"
          placeholder="only when not on loopback"
        />
      </label>

      <p class="hint">
        <code>uwd</code> needs no token on loopback, and refuses to bind anything
        else without one.
      </p>

      <label v-if="autostart !== null" class="row">
        <input type="checkbox" :checked="autostart" @change="toggleAutostart" />
        <span>Start at login</span>
      </label>

      <div class="actions">
        <button type="button" class="link" @click="reset">Reset</button>
        <span class="spacer" />
        <button type="submit" class="primary">Connect</button>
      </div>
    </form>
  </section>
</template>

<style scoped>
.settings {
  padding-bottom: 0.6rem;
}

.bar {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.45rem 0.75rem;
  border-bottom: 1px solid var(--rule);
  font-size: 0.72rem;
  color: var(--fg-dim);
}

.bar strong {
  color: var(--fg);
  font-weight: 600;
}

.spacer {
  flex: 1;
}

form {
  display: flex;
  flex-direction: column;
  gap: 0.55rem;
  padding: 0.7rem 0.75rem 0;
}

label {
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
  font-size: 0.7rem;
  color: var(--fg-dim);
}

input {
  /* 16px: anything smaller makes iOS Safari zoom the whole page on focus, and
     the panel never zooms back out. */
  font: inherit;
  font-size: 16px;
  padding: 0.4rem 0.5rem;
  color: var(--fg);
  background: var(--track);
  border: 1px solid var(--rule);
  border-radius: 5px;
}

input:focus {
  outline: none;
  border-color: var(--fg-faint);
}

.hint {
  margin: 0;
  font-size: 0.68rem;
  line-height: 1.4;
  color: var(--fg-faint);
}

.row {
  flex-direction: row;
  align-items: center;
  gap: 0.45rem;
  font-size: 0.72rem;
  color: var(--fg);
  cursor: pointer;
}

.row input {
  width: auto;
  min-height: 0;
  padding: 0;
  accent-color: var(--fg-dim);
}

.actions {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding-top: 0.15rem;
}

button {
  font: inherit;
  font-size: 0.72rem;
  cursor: pointer;
  border-radius: 5px;
  /* Comfortably tappable; the desktop panel has room to spare. */
  min-height: 2rem;
  padding: 0 0.7rem;
}

.link {
  background: none;
  border: none;
  color: var(--fg-dim);
  padding: 0 0.2rem;
}

.link:hover {
  color: var(--fg);
}

.primary {
  background: var(--track);
  color: var(--fg);
  border: 1px solid var(--rule);
}

.primary:hover {
  border-color: var(--fg-faint);
}

code {
  font-size: 0.66rem;
  background: var(--track);
  border-radius: 3px;
  padding: 0.05rem 0.25rem;
}
</style>
