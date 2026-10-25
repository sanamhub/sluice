// The sluice landing page and a 404. No docs framework (the docs stay in the repository), no
// client framework, no server. Built to dist/ and served by a Cloudflare Worker with no code of
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
});
