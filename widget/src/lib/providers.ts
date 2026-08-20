import { computed, ref, shallowRef } from "vue";
import { api } from "./api";
import type { ProviderInfo } from "../types/ProviderInfo";
import type { ProvidersView } from "../types/ProvidersView";

/**
 * The configured-provider list, shared by every view that shows it.
 *
 * Module-level rather than per-component for the same reason the daemon
 * settings are: the panel, the provider list and the add screen are three views
 * of one thing, and three copies would disagree the moment a login finished in
 * one of them.
 *
 * Kept fresh from two directions. The screens that change something use the
 * response of the call that changed it, so there is no refetch and no flicker.
 * Everything else arrives on the daemon's `providers` event — which is what
 * makes a browser login land on screen, since that completes long after the
 * request that started it was answered.
 */
const view = shallowRef<ProvidersView | null>(null);
const error = ref<string | null>(null);
const loading = ref(false);

export const providersView = view;
export const providersError = error;
export const providersLoading = loading;

/** What the user has added. Empty until the first load completes. */
export const configured = computed(() => view.value?.configured ?? []);

/** Everything that could be added, whether or not it already has been. */
export const catalogue = computed(() => view.value?.catalogue ?? []);

/** The catalogue minus what is already there — what the "+" screen offers. */
export const addable = computed<ProviderInfo[]>(() => {
  const have = new Set(configured.value.map((p) => p.id));
  return catalogue.value.filter((p) => !have.has(p.id));
});

/** True once we know the answer and the answer is "nothing". */
export const isEmpty = computed(() => view.value !== null && configured.value.length === 0);

export function applyProviders(next: ProvidersView) {
  view.value = next;
  error.value = null;
}

export async function refreshProviders(): Promise<void> {
  loading.value = true;
  try {
    applyProviders(await api.providers());
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

/** Run a mutating call and adopt its response, surfacing failures as text. */
export async function mutate(run: () => Promise<ProvidersView>): Promise<string | null> {
  try {
    applyProviders(await run());
    return null;
  } catch (e) {
    return e instanceof Error ? e.message : String(e);
  }
}
