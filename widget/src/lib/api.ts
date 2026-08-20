import { authHeaders, baseUrl } from "./settings";
import type { AuthPreference } from "../types/AuthPreference";
import type { LoginStarted } from "../types/LoginStarted";
import type { ProvidersView } from "../types/ProvidersView";

/**
 * The daemon's write API — adding providers, signing in, removing them.
 *
 * Every one of these runs on the daemon rather than in this process, which
 * looks like a detour until you remember where the credentials are. In the
 * arrangement this project was built for the daemon is inside WSL, holding the
 * tokens beside the vendor CLIs, and this UI is a Windows app; a login driven
 * from here would store the credential on the wrong side of that boundary. The
 * same indirection is what lets a phone drive a login on your desktop.
 */

/** The `{ error }` body the daemon sends, surfaced as a real Error. */
async function call<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${baseUrl()}${path}`, {
      ...init,
      headers: {
        ...(init?.body ? { "content-type": "application/json" } : {}),
        ...authHeaders(),
        ...(init?.headers ?? {}),
      },
    });
  } catch (e) {
    // A network-level failure here is almost always "the daemon is not
    // running", which is worth saying plainly rather than as "Failed to fetch".
    throw new Error(`Cannot reach uwd at ${baseUrl()}`, { cause: e });
  }

  const body = await res.text();
  if (!res.ok) {
    let message = body || `${res.status} ${res.statusText}`;
    try {
      const parsed = JSON.parse(body) as { error?: string };
      if (parsed.error) message = parsed.error;
    } catch {
      // Not JSON — a proxy error page, say. The raw text is still the best
      // thing we have to show.
    }
    throw new Error(message);
  }
  return body ? (JSON.parse(body) as T) : (undefined as T);
}

const json = (body: unknown): RequestInit => ({
  method: "POST",
  body: JSON.stringify(body),
});

export const api = {
  providers: () => call<ProvidersView>("/providers"),

  add: (id: string, auth: AuthPreference) =>
    call<ProvidersView>("/providers", json({ id, auth })),

  remove: (id: string) =>
    call<ProvidersView>(`/providers/${encodeURIComponent(id)}`, { method: "DELETE" }),

  /** Begins a browser login. The returned phase carries the URL to open. */
  startLogin: (id: string) =>
    call<LoginStarted>(`/providers/${encodeURIComponent(id)}/login`, { method: "POST" }),

  loginStatus: (id: string) =>
    call<LoginStarted>(`/providers/${encodeURIComponent(id)}/login`),

  /** Abandon a sign-in, freeing the loopback port it is holding. */
  cancelLogin: (id: string) =>
    call<{ cancelled: boolean }>(`/providers/${encodeURIComponent(id)}/login`, {
      method: "DELETE",
    }),

  /** For providers that display a code instead of redirecting back. */
  submitCode: (id: string, session: string, code: string) =>
    call<LoginStarted>(
      `/providers/${encodeURIComponent(id)}/login/code`,
      json({ session, code }),
    ),

  setToken: (id: string, token: string) =>
    call<ProvidersView>(`/providers/${encodeURIComponent(id)}/token`, json({ token })),

  logout: (id: string) =>
    call<ProvidersView>(`/providers/${encodeURIComponent(id)}/logout`, { method: "POST" }),
};
