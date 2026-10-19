# 0009. One Svelte 5 UI for the browser and the desktop

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub

## Context

Sluice needs a browser client and a desktop app. Two UI codebases would double the work for what is
mostly a drop zone, a ticket field and a progress list.

The first-load budget is 60 KB of JavaScript (`docs/performance.md`). React's runtime is roughly 45 KB
compressed; Svelte 5's is 2 to 3 KB. Flutter adds a language and cannot share code with the browser
build. Electron adds a browser engine per install and idle memory the budget does not allow.

## Decision

One TypeScript application in Svelte 5, built twice: as a PWA with the WASM engine, and inside a Tauri
2 shell with the native engine compiled in. Vite builds it, Biome lints and formats it, Vitest and
Playwright test it.

## Consequences

- One UI covers the browser, Windows, macOS and Linux, and later iOS and Android through Tauri.
- Tauri's backend is Rust, so the desktop app runs the engine in process with no sidecar.
- Svelte's ecosystem is smaller than React's, which matters little for an app this size.
