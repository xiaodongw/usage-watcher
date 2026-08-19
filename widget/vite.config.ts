import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

/**
 * Set by `tauri android dev` / `tauri ios dev` to the address the *device* must
 * dial to reach this machine. A phone cannot resolve `localhost` to your
 * laptop, so both the dev server and HMR have to be told the real host.
 */
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [vue()],
  // Tauri serves the built bundle from a file:// style origin, so every asset
  // reference has to be relative rather than rooted at /.
  base: "./",
  server: {
    port: 5173,
    // WSL: bind all interfaces so a browser on the Windows side can reach the
    // dev server through localhost forwarding. Same setting serves a phone on
    // the LAN, which is why it is unconditional.
    host: host || true,
    strictPort: true,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
  },
  build: {
    target: "es2022",
    outDir: "dist",
  },
});
