<script setup lang="ts">
import { ref } from "vue";
import { api } from "../lib/api";
import { configured, mutate, providersError } from "../lib/providers";
import type { ConfiguredProvider } from "../types/ConfiguredProvider";
import type { AuthPreference } from "../types/AuthPreference";

const emit = defineEmits<{ close: []; add: []; settings: []; signIn: [id: string] }>();

/** Which row is awaiting confirmation. One at a time, inline. */
const confirming = ref<string | null>(null);
const busy = ref<string | null>(null);
const error = ref<string | null>(null);

async function remove(p: ConfiguredProvider) {
  busy.value = p.id;
  error.value = await mutate(() => api.remove(p.id));
  busy.value = null;
  confirming.value = null;
}

/**
 * Deleting a credential is not undoable — a browser login has to be done
 * again — so it asks first. Inline rather than a dialog: the panel is 340px
 * wide and a modal in it is a wall.
 */
function askRemove(p: ConfiguredProvider) {
  confirming.value = confirming.value === p.id ? null : p.id;
}

function describe(auth: AuthPreference): string {
  switch (auth) {
    case "own":
      return "own sign-in";
    case "delegated":
      return "borrows the CLI";
    case "token":
      return "pasted key";
  }
}
</script>

<template>
  <section class="view">
    <header class="bar">
      <button class="link" title="Back to the panel" @click="emit('close')">‹</button>
      <strong>Providers</strong>
      <span class="spacer" />
      <button class="add" title="Add a provider" @click="emit('add')">+</button>
    </header>

    <p v-if="error ?? providersError" class="error">{{ error ?? providersError }}</p>

    <p v-if="configured.length === 0" class="empty">
      Nothing is being watched yet. Press <strong>+</strong> to add a provider.
    </p>

    <ul class="rows">
      <li v-for="p in configured" :key="p.id" class="row">
        <img class="icon" :src="p.icon" alt="" />
        <span class="name">
          <strong>{{ p.label }}</strong>
          <small>
            {{ describe(p.auth) }}
            <template v-if="!p.signed_in"> · not signed in</template>
          </small>
        </span>

        <button
          v-if="!p.signed_in && p.auth === 'own'"
          class="ghost"
          @click="emit('signIn', p.id)"
        >
          Sign in
        </button>
        <span v-else />

        <button
          class="trash"
          :disabled="busy === p.id"
          :title="`Remove ${p.label}`"
          @click="askRemove(p)"
        >
          🗑
        </button>

        <!-- The note explains an unusable mode: delegated with no vendor CLI
             installed, most often. Worth a line, not a colour. -->
        <p v-if="p.note" class="note">{{ p.note }}</p>

        <p v-if="confirming === p.id" class="confirm">
          Remove {{ p.label }} and delete its stored credential?
          <button class="danger" :disabled="busy === p.id" @click="remove(p)">Remove</button>
          <button class="link" @click="confirming = null">Cancel</button>
        </p>
      </li>
    </ul>

    <footer class="foot">
      <button class="link" @click="emit('settings')">Daemon settings</button>
    </footer>
  </section>
</template>

<style scoped>
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

.add {
  font: inherit;
  font-size: 1.05rem;
  line-height: 1;
  background: none;
  border: none;
  color: var(--fg-dim);
  cursor: pointer;
  padding: 0.1rem 0.3rem;
}

.add:hover {
  color: var(--fg);
}

.rows {
  list-style: none;
  margin: 0;
  padding: 0;
}

.row {
  display: grid;
  grid-template-columns: auto 1fr auto auto;
  align-items: center;
  gap: 0.5rem;
  padding: 0.55rem 0.75rem;
  border-bottom: 1px solid var(--rule);
}

/* The vendor's own mark, from the manifest. Nothing tints, inverts or
   filters it: each one already carries whatever background it needs to read
   on both themes, and a `filter` that fixed one of them would wreck another. */
.icon {
  width: 20px;
  height: 20px;
  border-radius: 5px;
}

.name {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.name strong {
  font-size: 0.8rem;
  font-weight: 600;
}

.name small {
  font-size: 0.66rem;
  color: var(--fg-faint);
}

.note,
.confirm {
  grid-column: 1 / -1;
  margin: 0.35rem 0 0;
  font-size: 0.68rem;
  line-height: 1.45;
  color: var(--fg-dim);
}

.confirm {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.4rem;
}

.foot {
  padding: 0.5rem 0.6rem;
}

.error {
  margin: 0;
  padding: 0.5rem 0.75rem;
  font-size: 0.7rem;
  color: var(--crit);
}

.empty {
  margin: 0;
  padding: 1rem 0.75rem;
  font-size: 0.72rem;
  line-height: 1.5;
  color: var(--fg-dim);
}

button {
  font: inherit;
  font-size: 0.7rem;
  cursor: pointer;
  border-radius: 5px;
  min-height: 1.8rem;
  padding: 0 0.55rem;
}

.ghost {
  background: var(--track);
  color: var(--fg);
  border: 1px solid var(--rule);
}

.ghost:hover {
  border-color: var(--fg-faint);
}

.trash {
  background: none;
  border: none;
  color: var(--fg-faint);
  font-size: 0.85rem;
  padding: 0 0.2rem;
}

.trash:hover {
  color: var(--crit);
}

.danger {
  background: none;
  border: 1px solid var(--crit);
  color: var(--crit);
}

.danger:hover {
  background: var(--crit);
  color: var(--bg);
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
</style>
