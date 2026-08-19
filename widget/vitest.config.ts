import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  test: {
    // Component tests mount into a real DOM; the pure formatting tests do not
    // care either way.
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
});
