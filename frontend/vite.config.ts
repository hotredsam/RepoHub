import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The Vite dev server proxies API + WebSocket calls to the Rust backend
// so the frontend can use same-origin "/api" and "/ws" paths.
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/api": { target: "http://127.0.0.1:8787", changeOrigin: true },
      "/ws": { target: "ws://127.0.0.1:8787", ws: true },
    },
  },
});
