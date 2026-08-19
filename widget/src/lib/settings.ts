import { ref, watch, type Ref } from "vue";

/**
 * Where to find the daemon, decided at runtime rather than at build time.
 *
 * The desktop build could get away with `VITE_UWD_URL`, baked in by whoever
 * ran `npm run build`. A phone cannot: the same signed binary has to reach a
 * daemon whose Tailscale address the user only knows after installing it, and
 * there is no shell to set an environment variable in. So the value is stored,
 * editable, and the env var becomes the default rather than the answer.
 */
export interface Settings {
  url: string;
  token: string;
}

const KEY = "usage-watcher.daemon";

export const DEFAULTS: Settings = {
  url: (import.meta.env.VITE_UWD_URL ?? "http://127.0.0.1:7878") as string,
  token: (import.meta.env.VITE_UWD_TOKEN ?? "") as string,
};

function load(): Settings {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { ...DEFAULTS };
    const parsed = JSON.parse(raw) as Partial<Settings>;
    return {
      url: typeof parsed.url === "string" && parsed.url ? parsed.url : DEFAULTS.url,
      token: typeof parsed.token === "string" ? parsed.token : DEFAULTS.token,
    };
  } catch {
    // A private-mode browser can throw on localStorage access, and a corrupt
    // entry should cost the stored value, not the whole app.
    return { ...DEFAULTS };
  }
}

/**
 * The single shared settings object.
 *
 * Module-level rather than per-component: two views editing two copies of the
 * daemon address is the kind of bug that only shows up on the device.
 */
export const settings: Ref<Settings> = ref(load());

watch(
  settings,
  (v) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(v));
    } catch {
      // Nothing to do — the value still applies for this session.
    }
  },
  { deep: true },
);

/** Trailing slashes are the most common paste error, and they double up in URLs. */
export function baseUrl(s: Settings = settings.value): string {
  return s.url.trim().replace(/\/+$/, "");
}

/** `EventSource` cannot set headers, so the token travels as a query param. */
export function daemonUrl(path: string, s: Settings = settings.value): string {
  const base = baseUrl(s);
  const token = s.token.trim();
  return token ? `${base}${path}?token=${encodeURIComponent(token)}` : `${base}${path}`;
}

/** True when the user has never touched the address — used to pick a first screen. */
export function isDefault(s: Settings = settings.value): boolean {
  return s.url === DEFAULTS.url && s.token === DEFAULTS.token;
}
