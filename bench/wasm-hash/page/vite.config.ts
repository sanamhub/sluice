import { defineConfig } from "vite";

export default defineConfig({
  // Relative, so the built page works from any path on any static host.
  base: "./",
  worker: {
    // The worker dynamic-imports the wasm-bindgen glue, which an IIFE worker cannot split out.
    format: "es",
  },
  // No server option sets COOP or COEP. ADR 0014's claim is that the page needs neither.
});
