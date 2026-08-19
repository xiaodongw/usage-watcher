<script setup lang="ts">
import { computed } from "vue";
import type { Provider } from "../types/Provider";
import MeterRow from "./MeterRow.vue";
import { ago } from "../lib/format";

const props = defineProps<{ provider: Provider; now: number }>();

const stale = computed(() => props.provider.status.state === "stale");
const failed = computed(
  () => props.provider.status.state === "error" || props.provider.status.state === "unavailable",
);

/** `state` is the serde tag, so narrowing on it gives us the payload field. */
const problem = computed(() => {
  const s = props.provider.status;
  if (s.state === "error") return s.message;
  if (s.state === "unavailable") return s.reason;
  return null;
});

const authBadge = computed(() => {
  switch (props.provider.auth) {
    case "own_grant":
      return { text: "own", title: "Signed in with our own OAuth grant" };
    case "delegated":
      return {
        text: "borrowed",
        title: "Reading the vendor CLI's token, read-only — never refreshed",
      };
    case "api_key":
      return { text: "token", title: "Using a long-lived token you pasted in" };
    case "none":
      return null;
  }
});
</script>

<template>
  <section class="tile">
    <header>
      <h2>{{ provider.label }}</h2>
      <span v-if="provider.plan" class="plan">{{ provider.plan }}</span>
      <span class="spacer" />
      <span v-if="authBadge" class="badge" :title="authBadge.title">{{ authBadge.text }}</span>
    </header>

    <p v-if="failed" class="problem">{{ problem }}</p>

    <template v-else>
      <MeterRow
        v-for="meter in provider.meters"
        :key="meter.id"
        :meter="meter"
        :now="now"
        :stale="stale"
      />
      <p v-if="provider.meters.length === 0" class="problem">Nothing reported.</p>
      <p v-if="stale" class="stale-note">
        Not updating — showing the reading from {{ ago(provider.updated_at, now) }}.
      </p>
    </template>
  </section>
</template>

<style scoped>
.tile {
  padding: 0.6rem 0.75rem 0.7rem;
  border-bottom: 1px solid var(--rule);
}

.tile:last-child {
  border-bottom: none;
}

header {
  display: flex;
  align-items: baseline;
  gap: 0.4rem;
  margin-bottom: 0.35rem;
}

h2 {
  font-size: 0.8rem;
  font-weight: 600;
  margin: 0;
  color: var(--fg);
}

.plan {
  font-size: 0.65rem;
  color: var(--fg-faint);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.spacer {
  flex: 1;
}

.badge {
  font-size: 0.6rem;
  color: var(--fg-faint);
  border: 1px solid var(--rule);
  border-radius: 999px;
  padding: 0.05rem 0.35rem;
  cursor: help;
}

.problem {
  margin: 0.2rem 0 0;
  font-size: 0.7rem;
  color: var(--crit);
  /* Adapter errors are sentences, not codes — let them wrap and be read. */
  line-height: 1.35;
}

.stale-note {
  margin: 0.3rem 0 0;
  font-size: 0.65rem;
  color: var(--fg-faint);
}
</style>
