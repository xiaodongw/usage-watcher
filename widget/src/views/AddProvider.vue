<script setup lang="ts">
import { ref } from "vue";
import { addable } from "../lib/providers";
import type { AuthMethod } from "../types/AuthMethod";
import type { ProviderInfo } from "../types/ProviderInfo";

/**
 * The catalogue. Every row on this screen — the names, the summaries, which
 * ways in each provider offers, and why one of them is greyed out — comes from
 * the daemon's `/providers` manifest, which the adapters generate. Adding a
 * fifth provider changes nothing in this file.
 */
const emit = defineEmits<{
  close: [];
  choose: [provider: ProviderInfo, method: AuthMethod];
}>();

/** Which provider's method list is expanded. */
const open = ref<string | null>(null);

function pick(p: ProviderInfo) {
  const usable = p.methods.filter((m) => m.available);
  // One way in and nothing to decide: skip the menu and get on with it. Making
  // someone confirm a choice they had no alternative to is a wasted screen.
  if (usable.length === 1) {
    emit("choose", p, usable[0]);
    return;
  }
  open.value = open.value === p.id ? null : p.id;
}
</script>

<template>
  <section class="view">
    <header class="bar">
      <button class="link" title="Back" @click="emit('close')">‹</button>
      <strong>Add a provider</strong>
    </header>

    <p v-if="addable.length === 0" class="empty">
      Every provider usage-watcher knows about has already been added.
    </p>

    <ul class="rows">
      <li v-for="p in addable" :key="p.id">
        <button class="card" @click="pick(p)">
          <img class="icon" :src="p.icon" alt="" />
          <span class="name">
            <strong>{{ p.label }}</strong>
            <small>{{ p.summary }}</small>
          </span>
          <span class="chev">{{ open === p.id ? "▾" : "›" }}</span>
        </button>

        <ul v-if="open === p.id" class="methods">
          <li v-for="m in p.methods" :key="m.auth">
            <button
              class="method"
              :class="{ off: !m.available }"
              :disabled="!m.available"
              @click="emit('choose', p, m)"
            >
              <span class="mlabel">
                {{ m.label }}
                <em v-if="m.recommended && m.available">Recommended</em>
              </span>
              <!-- The unavailable reason is the useful half: "Codex is not
                   signed in on this machine" tells you what to do, where a
                   greyed-out row with no explanation does not. -->
              <small>{{ m.unavailable_reason ?? m.detail }}</small>
            </button>
          </li>
        </ul>
      </li>
    </ul>
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

.rows,
.methods {
  list-style: none;
  margin: 0;
  padding: 0;
}

.card {
  display: grid;
  grid-template-columns: auto 1fr auto;
  align-items: center;
  gap: 0.55rem;
  width: 100%;
  text-align: left;
  font: inherit;
  background: none;
  border: none;
  border-bottom: 1px solid var(--rule);
  padding: 0.6rem 0.75rem;
  cursor: pointer;
  color: var(--fg);
}

.card:hover {
  background: var(--track);
}

/* Larger than the list's, because this row is two lines tall and an icon
   sized for a one-line row looks lost beside it. */
.icon {
  width: 24px;
  height: 24px;
  border-radius: 6px;
}

.name {
  display: flex;
  flex-direction: column;
  min-width: 0;
  gap: 0.1rem;
}

.name strong {
  font-size: 0.8rem;
  font-weight: 600;
}

.name small {
  font-size: 0.66rem;
  line-height: 1.4;
  color: var(--fg-faint);
}

.chev {
  color: var(--fg-faint);
  font-size: 0.8rem;
}

.methods {
  background: var(--track);
  border-bottom: 1px solid var(--rule);
}

.method {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  width: 100%;
  text-align: left;
  font: inherit;
  background: none;
  border: none;
  padding: 0.5rem 0.75rem 0.5rem 1.6rem;
  cursor: pointer;
  color: var(--fg);
}

.method:hover:not(.off) {
  color: var(--fg);
  background: var(--bg);
}

.method.off {
  cursor: not-allowed;
  color: var(--fg-faint);
}

.mlabel {
  font-size: 0.74rem;
  display: flex;
  align-items: center;
  gap: 0.4rem;
}

.mlabel em {
  font-style: normal;
  font-size: 0.6rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--ok);
}

.method small {
  font-size: 0.65rem;
  line-height: 1.45;
  color: var(--fg-faint);
}

.empty {
  margin: 0;
  padding: 1rem 0.75rem;
  font-size: 0.72rem;
  line-height: 1.5;
  color: var(--fg-dim);
}

.link {
  font: inherit;
  font-size: 0.72rem;
  background: none;
  border: none;
  color: var(--fg-dim);
  padding: 0 0.2rem;
  cursor: pointer;
}

.link:hover {
  color: var(--fg);
}
</style>
