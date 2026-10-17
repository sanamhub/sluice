# 0009. Svelte 5 for a single UI shared by browser and desktop

**Status:** Accepted

## Context

The product needs a browser client and a desktop app, and writing two UIs doubles the work. Avalonia and
.NET MAUI are good desktop toolkits but cannot share code with a browser, and MAUI has no Linux target.
Flutter forces a third language and still cannot share with the browser. Electron costs bundle size and
idle memory for what is a drop zone and a progress bar.

The at-rest budget allows 60 KB of JavaScript on first load. React's runtime alone is roughly 45 KB
minified and compressed, against 2 to 3 KB for Svelte 5.

## Decision

One TypeScript application built with Svelte 5, shipped twice: as a PWA with the WASM core, and inside a
Tauri 2 shell with the native Rust core compiled in. Vite for build, Biome for lint and format, Vitest
and Playwright for tests.

## Consequences

- One UI codebase covers browser, Windows, macOS, Linux, and later iOS and Android.
- The first-load budget becomes achievable rather than aspirational.
- Tauri's backend language is Rust, which is already ours, so the desktop app needs no sidecar process.
