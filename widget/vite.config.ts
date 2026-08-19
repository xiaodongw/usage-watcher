import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  // Tauri serves the built bundle from a file:// style origin, so every asset
  // reference has to be relative rather than rooted at /.
  base: "./",
  server: {
    port: 5173,
    // WSL: bind all interfaces so a browser on the Windows side can reach the
    // dev server through localhost forwarding.
    host: true,
    strictPort: true,
  },
  build: {
    target: "es2022",
    outDir: "dist",
  },
});
