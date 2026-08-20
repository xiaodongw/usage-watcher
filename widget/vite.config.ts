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
  // Cargo's own errors are the ones worth reading during `tauri dev`, and Vite
  // wipes the scrollback on every restart otherwise.
  clearScreen: false,
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
    watch: {
      // Never watch the Rust side. `src-tauri/target` is written continuously
      // while Cargo builds, and on Windows a DLL that is mid-write is locked —
      // the watcher then dies with EBUSY and takes `tauri dev` down with it.
      // Nothing under here is a frontend source file, so there is nothing lost.
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: "es2022",
    outDir: "dist",
  },
});
