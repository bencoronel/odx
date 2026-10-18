import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  server: { strictPort: true },
  test: { environment: "jsdom", include: ["src/**/*.test.tsx", "src/**/*.test.ts"], setupFiles: ["src/test-setup.ts"] },
});
