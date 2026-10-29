// The sluice landing page and a 404. No docs framework (the docs stay in the repository), no
// client framework, no server. The page reads without script; scripts add the theme button, the
// install tabs and the hero's scene. Built to dist/ and served by a Cloudflare Worker with no code of
// its own (wrangler.jsonc, ADR 0022).
import { defineConfig } from "astro/config";
import { fonts } from "./fonts.mjs";
import { SITE_URL } from "./src/site";

export default defineConfig({
    site: SITE_URL,
    output: "static",
    trailingSlash: "ignore",
    build: {
        format: "directory",
        // The whole stylesheet is a few KB, so inlining it saves a request before first paint.
        inlineStylesheets: "always",
    },
    fonts,
    // Whitespace that holds a line break is kept as a space, so a sentence that wraps in the source
    // before a link keeps the space before it.
    compressHTML: false,
    vite: {
        // Astro inlines a script under this size. The CSP allows scripts from this origin only, so
        // every script is a file, never inline.
        build: {
            assetsInlineLimit: 0,
            // The hero's three.js chunk is large on purpose and loads only when the scene can run.
            chunkSizeWarningLimit: 600,
        },
    },
});
