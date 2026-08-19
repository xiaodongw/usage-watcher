<script setup lang="ts">
import { computed } from "vue";
import type { Meter } from "../types/Meter";
import { countdown, fill, readout, resetsAt } from "../lib/format";

const props = defineProps<{
  meter: Meter;
  now: number;
  /** Dim everything when the reading is known to be out of date. */
  stale?: boolean;
}>();

const width = computed(() => `${(fill(props.meter) * 100).toFixed(1)}%`);
const resets = computed(() => countdown(resetsAt(props.meter), props.now));
</script>

<template>
  <div class="row" :class="{ stale }">
    <div class="label" :title="meter.label">{{ meter.label }}</div>
    <div class="track" :class="meter.severity">
      <div class="fill" :style="{ width }" />
    </div>
    <div class="value">{{ readout(meter) }}</div>
    <div class="resets">{{ resets ?? "" }}</div>
  </div>
</template>

<style scoped>
.row {
  display: grid;
  /* Fixed columns rather than auto: the bars must line up across providers,
     otherwise the eye cannot compare them, which is the whole point. */
  grid-template-columns: 5.5rem 1fr 2.75rem 3.25rem;
  align-items: center;
  gap: 0.5rem;
  padding: 0.2rem 0;
  font-variant-numeric: tabular-nums;
}

.row.stale {
  opacity: 0.45;
}

.label {
  color: var(--fg-dim);
  font-size: 0.75rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.track {
  height: 6px;
  border-radius: 3px;
  background: var(--track);
  overflow: hidden;
}

.fill {
  height: 100%;
  border-radius: 3px;
  background: var(--ok);
  transition: width 0.4s ease;
}

.track.warning .fill {
  background: var(--warn);
}

.track.critical .fill {
  background: var(--crit);
}

.value {
  font-size: 0.75rem;
  text-align: right;
  color: var(--fg);
}

.resets {
  font-size: 0.7rem;
  text-align: right;
  color: var(--fg-faint);
}
</style>
