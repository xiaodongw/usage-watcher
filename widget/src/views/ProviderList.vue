<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import { api } from "../lib/api";
import { configured, mutate, providersError } from "../lib/providers";
import type { ConfiguredProvider } from "../types/ConfiguredProvider";
import type { AuthPreference } from "../types/AuthPreference";

const emit = defineEmits<{ close: []; add: []; settings: []; signIn: [id: string] }>();

/** Which row is awaiting confirmation. One at a time, inline. */
const confirming = ref<string | null>(null);
const busy = ref<string | null>(null);
const error = ref<string | null>(null);

/**
 * The order being shown, while it differs from the daemon's.
 *
 * `null` almost always: the list follows `configured`, which the daemon sends
 * in the order it stores. It is only set while a drag is in flight and until
 * the write that follows it comes back, so a `providers` event arriving from
 * another window mid-drag cannot yank the rows out from under the pointer.
 */
const local = ref<ConfiguredProvider[] | null>(null);
const rows = computed(() => local.value ?? configured.value);

const list = ref<HTMLElement | null>(null);

/** The row being dragged, how far it has been pulled, and where it would land. */
const dragId = ref<string | null>(null);
const dragY = ref(0);
const dropAt = ref(-1);

/**
 * Row midpoints, measured once when the drag starts.
 *
 * Frozen on purpose. The obvious implementation reorders the array on every
 * pointer move, but then the layout it is measuring against is the layout it
 * just changed — and with rows of different heights (a "not signed in" note
 * makes one taller) that feedback loop oscillates: the row swaps down, the
 * shorter neighbour moves up under the pointer, and the next move swaps it
 * straight back. Measuring a layout that is not moving cannot do that. The
 * cost is that the other rows do not part to make room; an insertion line
 * shows where the row will land instead.
 */
let midpoints: number[] = [];
let grabbedAt = 0;
let from = -1;

function startDrag(event: PointerEvent, index: number) {
  // Left button only; touch and pen report 0 here too.
  if (event.button !== 0) return;
  event.preventDefault();
  const handle = event.currentTarget as HTMLElement;
  // Without capture the drag dies the moment the pointer leaves the handle,
  // which at 20px square is immediately.
  handle.setPointerCapture(event.pointerId);

  const boxes = Array.from(list.value?.querySelectorAll("li.row") ?? []);
  midpoints = boxes.map((el) => {
    const r = el.getBoundingClientRect();
    return r.top + r.height / 2;
  });
  local.value = rows.value.slice();
  from = index;
  dragId.value = rows.value[index].id;
  grabbedAt = event.clientY;
  dragY.value = 0;
  dropAt.value = index;
  // A row half-way through a delete confirmation should not be draggable-with-
  // a-question-open; the confirmation refers to a position that is moving.
  confirming.value = null;
  window.addEventListener("keydown", cancelOnEscape);
}

function onDrag(event: PointerEvent) {
  if (dragId.value === null) return;
  dragY.value = event.clientY - grabbedAt;
  // How many rows the pointer has passed the middle of. That is the index the
  // row would be inserted *before*, so it ranges 0..rows.length.
  dropAt.value = midpoints.filter((m) => event.clientY > m).length;
}

function endDrag() {
  if (dragId.value === null) return;
  // Read before `reset`, which clears both. `dropAt` counts midpoints passed,
  // so it is an insertion point in the *unmoved* list: dropping below your own
  // row means one of the midpoints counted was your own.
  const start = from;
  const to = dropAt.value > start ? dropAt.value - 1 : dropAt.value;
  const moved = local.value ?? [];
  reset();
  if (to === start || to < 0) {
    local.value = null;
    return;
  }
  void commit(move(moved, start, to));
}

function move(rows: ConfiguredProvider[], from: number, to: number): ConfiguredProvider[] {
  const next = rows.slice();
  const [row] = next.splice(from, 1);
  next.splice(to, 0, row);
  return next;
}

/** Escape, a lost pointer, or an unmount: put it back where it was. */
function cancelDrag() {
  if (dragId.value === null) return;
  reset();
  local.value = null;
}

function cancelOnEscape(event: KeyboardEvent) {
  if (event.key === "Escape") cancelDrag();
}

function reset() {
  dragId.value = null;
  dropAt.value = -1;
  dragY.value = 0;
  from = -1;
  window.removeEventListener("keydown", cancelOnEscape);
}

onBeforeUnmount(() => window.removeEventListener("keydown", cancelOnEscape));

/**
 * Move a row one place with the keyboard.
 *
 * A control you can only operate by dragging is one a keyboard cannot reach at
 * all, and the handle is a button precisely so it can be tabbed to.
 */
function nudge(index: number, delta: number) {
  const to = index + delta;
  if (to < 0 || to >= rows.value.length) return;
  void commit(move(rows.value, index, to));
}

async function commit(next: ConfiguredProvider[]) {
  // Shown before it is saved: the drop has to feel instant, and the daemon's
  // answer is the same list. `local` is cleared either way afterwards, so a
  // rejected write snaps back to whatever the daemon actually has.
  local.value = next;
  const failed = await mutate(() => api.reorder(next.map((p) => p.id)));
  error.value = failed;
  local.value = null;
}

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

    <p v-if="rows.length === 0" class="empty">
      Nothing is being watched yet. Press <strong>+</strong> to add a provider.
    </p>

    <ul ref="list" class="rows">
      <li
        v-for="(p, i) in rows"
        :key="p.id"
        class="row"
        :class="{
          lifted: dragId === p.id,
          'drop-above': dragId !== null && dropAt === i,
          'drop-below': dragId !== null && dropAt === rows.length && i === rows.length - 1,
        }"
        :style="dragId === p.id ? { transform: `translateY(${dragY}px)` } : undefined"
      >
        <button
          class="grip"
          :title="`Reorder ${p.label}`"
          :aria-label="`Reorder ${p.label}. Use the arrow keys to move it.`"
          @pointerdown="startDrag($event, i)"
          @pointermove="onDrag"
          @pointerup="endDrag"
          @pointercancel="cancelDrag"
          @keydown.up.prevent="nudge(i, -1)"
          @keydown.down.prevent="nudge(i, 1)"
        >
          <!-- Drawn rather than typed. The conventional glyph for this is
               U+2833, and a machine whose font lacks it would render the whole
               feature as a column of tofu. -->
          <svg viewBox="0 0 10 16" aria-hidden="true">
            <circle v-for="y in [4, 8, 12]" :key="`l${y}`" cx="3" :cy="y" r="1.15" />
            <circle v-for="y in [4, 8, 12]" :key="`r${y}`" cx="7" :cy="y" r="1.15" />
          </svg>
        </button>
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
          :aria-label="`Remove ${p.label}`"
          @click="askRemove(p)"
        >
          <!-- Drawn, like the grip above, and for the same reason: this was
               U+1F5D1 until a machine without an emoji font rendered the whole
               column as tofu. -->
          <svg viewBox="0 0 14 16" aria-hidden="true">
            <path
              d="M2 4h10v10a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 2 14V4Zm3.5 2.5v7m3-7v7"
              fill="none"
              stroke="currentColor"
              stroke-width="1.3"
              stroke-linecap="round"
            />
            <path
              d="M0.5 3h13M5 3V1.5A1 1 0 0 1 6 .5h2a1 1 0 0 1 1 1V3"
              fill="none"
              stroke="currentColor"
              stroke-width="1.3"
              stroke-linecap="round"
            />
          </svg>
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
  grid-template-columns: auto auto 1fr auto auto;
  align-items: center;
  gap: 0.5rem;
  padding: 0.55rem 0.75rem;
  border-bottom: 1px solid var(--rule);
  background: var(--bg);
}

/* The row under the pointer. `transform` rather than anything that reflows:
   the drop position is measured against the layout as it was when the drag
   started, so the layout must not move while it is being measured. */
.row.lifted {
  position: relative;
  z-index: 1;
  opacity: 0.85;
  border-radius: 6px;
  box-shadow: 0 4px 14px rgb(0 0 0 / 35%);
}

/* Where it would land. Since the other rows do not part to make room, this
   line is the only thing that answers "where am I dropping this?" — so it is
   drawn on the boundary itself rather than as a highlight on a neighbour,
   which would read as "swap with that one". */
.row.drop-above::before,
.row.drop-below::after {
  content: "";
  position: absolute;
  left: 0.5rem;
  right: 0.5rem;
  height: 2px;
  border-radius: 1px;
  background: var(--fg-dim);
}

.row.drop-above,
.row.drop-below {
  position: relative;
}

.row.drop-above::before {
  top: -1px;
}

.row.drop-below::after {
  bottom: -1px;
}

.grip {
  display: flex;
  align-items: center;
  padding: 0 0.1rem;
  background: none;
  border: none;
  color: var(--fg-faint);
  cursor: grab;
  /* Or the first drag on a touchscreen scrolls the panel instead. */
  touch-action: none;
}

.grip:hover,
.grip:focus-visible {
  color: var(--fg-dim);
}

.grip:active {
  cursor: grabbing;
}

.grip svg {
  width: 10px;
  height: 16px;
  fill: currentColor;
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
  display: flex;
  align-items: center;
  background: none;
  border: none;
  color: var(--fg-faint);
  padding: 0 0.2rem;
}

.trash svg {
  width: 14px;
  height: 16px;
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
