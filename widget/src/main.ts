import { createApp } from "vue";
import App from "./App.vue";
import { setAutoUrl } from "./lib/settings";
import { discoverDaemonUrl } from "./lib/shell";
import "./style.css";

/**
 * Ask the shell where the daemon is before painting anything.
 *
 * The desktop build starts a daemon in-process, on the configured port or an
 * ephemeral one if something else already holds it, so the address is not a
 * constant and cannot be baked in at build time. Resolved before mounting
 * rather than after: a first render against the wrong port would open an
 * `EventSource` to nothing and flash "cannot reach uwd" on every launch.
 *
 * Not top-level `await` — that forces the whole bundle to an ESM target the
 * older WebViews on the mobile side do not all have.
 */
discoverDaemonUrl()
  .then((url) => {
    if (url) setAutoUrl(url);
  })
  .catch(() => {
    // No shell, or an older one without the command. The conventional port is
    // the right guess, and the settings screen can override it.
  })
  .finally(() => createApp(App).mount("#app"));
