<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { api } from "../lib/api";
import { applyProviders } from "../lib/providers";
import { openExternal } from "../lib/shell";
import type { AuthMethod } from "../types/AuthMethod";
import type { Phase } from "../types/Phase";
import type { ProviderInfo } from "../types/ProviderInfo";

/**
 * One screen for all three ways in, because the user picked a provider, not a
 * protocol. Which of the three is in play comes from the manifest, so this
 * component has never heard of Claude or OpenRouter.
 */
const props = defineProps<{ provider: ProviderInfo; method: AuthMethod }>();
const emit = defineEmits<{ done: []; cancel: [] }>();

const phase = ref<Phase | null>(null);
const session = ref<string | null>(null);
const error = ref<string | null>(null);
const busy = ref(false);
const code = ref("");
const token = ref("");
/** True once we have opened the browser at least once, to reword the button. */
const opened = ref(false);

let poll: number | undefined;

onMounted(start);
onUnmounted(stopPolling);

function stopPolling() {
  if (poll !== undefined) window.clearInterval(poll);
  poll = undefined;
}

async function start() {
  error.value = null;
  busy.value = true;
  try {
    // Recorded before the sign-in, not after. A login that fails must still
    // leave the provider in the list — visible, retryable, removable — rather
    // than vanishing along with whatever went wrong.
    applyProviders(await api.add(props.provider.id, props.method.auth));

    if (props.method.kind === "borrow") {
      // Nothing to do: the vendor CLI's file is the credential, and the poller
      // is already reading it.
      emit("done");
      return;
    }
    if (props.method.kind === "browser") {
      await beginBrowserLogin();
    }
  } catch (e) {
    error.value = message(e);
  } finally {
    busy.value = false;
  }
}

async function beginBrowserLogin() {
  const started = await api.startLogin(props.provider.id);
  session.value = started.session;
  phase.value = started.phase;

  if (started.phase.phase === "waiting") {
    opened.value = await openExternal(started.phase.authorize_url);
  }
  watchProgress();
}

/**
 * Poll the daemon for the outcome.
 *
 * Belt to the SSE braces: the `providers` event tells the *list* that something
 * changed, but this screen needs the reason a login failed, and it must work
 * even if the event stream happens to be reconnecting at that moment.
 */
function watchProgress() {
  stopPolling();
  poll = window.setInterval(async () => {
    try {
      const status = await api.loginStatus(props.provider.id);
      phase.value = status.phase;
      session.value = status.session;
      if (status.phase.phase === "done") {
        stopPolling();
        emit("done");
      } else if (status.phase.phase === "failed") {
        stopPolling();
      }
    } catch {
      // A 404 means the session was replaced or the daemon restarted; either
      // way there is nothing left to wait for.
      stopPolling();
    }
  }, 1500);
}

async function submitCode() {
  if (!session.value || !code.value.trim()) return;
  busy.value = true;
  error.value = null;
  try {
    const res = await api.submitCode(props.provider.id, session.value, code.value.trim());
    phase.value = res.phase;
    code.value = "";
    watchProgress();
  } catch (e) {
    error.value = message(e);
  } finally {
    busy.value = false;
  }
}

async function saveToken() {
  if (!token.value.trim()) return;
  busy.value = true;
  error.value = null;
  try {
    applyProviders(await api.setToken(props.provider.id, token.value.trim()));
    token.value = "";
    emit("done");
  } catch (e) {
    error.value = message(e);
  } finally {
    busy.value = false;
  }
}

function reopen() {
  if (phase.value?.phase === "waiting") void openExternal(phase.value.authorize_url);
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
</script>

<template>
  <section class="view">
    <header class="bar">
      <button class="link" title="Back" @click="emit('cancel')">‹</button>
      <strong>{{ provider.label }}</strong>
    </header>

    <div class="body">
      <p v-if="error" class="error">{{ error }}</p>

      <!-- Paste a key. Label, placeholder and help all come from the
           provider's own manifest, so the wording matches its console. -->
      <form v-if="method.kind === 'paste' && method.token" @submit.prevent="saveToken">
        <p class="help">{{ method.token.help }}</p>
        <p v-if="method.token.url" class="help">
          <button type="button" class="link inline" @click="openExternal(method.token!.url!)">
            {{ method.token.url }}
          </button>
        </p>
        <label>
          <span>{{ method.token.label }}</span>
          <input
            v-model="token"
            type="password"
            autocapitalize="off"
            autocorrect="off"
            spellcheck="false"
            :placeholder="method.token.placeholder"
          />
        </label>
        <div class="actions">
          <button type="submit" class="primary" :disabled="busy || !token.trim()">Save</button>
        </div>
      </form>

      <!-- Browser sign-in. -->
      <template v-else-if="method.kind === 'browser'">
        <p v-if="!phase || phase.phase === 'opening'" class="help">Starting sign-in…</p>

        <template v-else-if="phase.phase === 'waiting'">
          <p class="help">
            {{ opened ? "Finish signing in, in the browser window that just opened." :
               "Open this address to sign in:" }}
          </p>
          <!-- Always shown, never only auto-opened. Under WSL, over SSH, and in
               a container there may be no browser to launch, and a URL you can
               copy is the difference between a flow that works and one that
               silently does nothing. -->
          <p class="url">{{ phase.authorize_url }}</p>
          <div class="actions">
            <button class="primary" @click="reopen">
              {{ opened ? "Open again" : "Open browser" }}
            </button>
          </div>

          <form v-if="phase.needs_code" class="code" @submit.prevent="submitCode">
            <p class="help">
              This provider shows a code instead of redirecting back. Paste it here.
            </p>
            <label>
              <span>Code</span>
              <input
                v-model="code"
                type="text"
                autocapitalize="off"
                autocorrect="off"
                spellcheck="false"
                placeholder="paste the code from the page"
              />
            </label>
            <div class="actions">
              <button type="submit" class="primary" :disabled="busy || !code.trim()">
                Finish
              </button>
            </div>
          </form>
          <p v-else class="waiting">Waiting for the browser…</p>
        </template>

        <template v-else-if="phase.phase === 'failed'">
          <p class="error">{{ phase.message }}</p>
          <div class="actions">
            <button class="primary" :disabled="busy" @click="start">Try again</button>
          </div>
        </template>

        <p v-else class="help">Signed in.</p>
      </template>
    </div>
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

.body {
  padding: 0.75rem;
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
}

form {
  display: flex;
  flex-direction: column;
  gap: 0.55rem;
}

.code {
  margin-top: 0.4rem;
  padding-top: 0.6rem;
  border-top: 1px solid var(--rule);
}

label {
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
  font-size: 0.7rem;
  color: var(--fg-dim);
}

input {
  /* 16px: anything smaller makes iOS Safari zoom the page on focus, and the
     panel never zooms back out. */
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

.help,
.waiting {
  margin: 0;
  font-size: 0.7rem;
  line-height: 1.5;
  color: var(--fg-dim);
}

.waiting {
  color: var(--fg-faint);
}

.url {
  margin: 0;
  font-size: 0.62rem;
  line-height: 1.4;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  color: var(--fg-faint);
  background: var(--track);
  border-radius: 4px;
  padding: 0.35rem 0.4rem;
  /* Long OAuth URLs must wrap rather than widen the window. */
  overflow-wrap: anywhere;
  max-height: 5.5rem;
  overflow-y: auto;
  user-select: all;
}

.error {
  margin: 0;
  font-size: 0.7rem;
  line-height: 1.5;
  color: var(--crit);
}

.actions {
  display: flex;
  gap: 0.5rem;
}

button {
  font: inherit;
  font-size: 0.72rem;
  cursor: pointer;
  border-radius: 5px;
  min-height: 2rem;
  padding: 0 0.7rem;
}

.primary {
  background: var(--track);
  color: var(--fg);
  border: 1px solid var(--rule);
}

.primary:hover:not(:disabled) {
  border-color: var(--fg-faint);
}

button:disabled {
  opacity: 0.5;
  cursor: default;
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

.inline {
  font-size: 0.68rem;
  text-decoration: underline;
  padding: 0;
  min-height: 0;
  text-align: left;
  overflow-wrap: anywhere;
}
</style>
