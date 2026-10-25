// The two families the page sets, read from the pinned fontsource packages so the files, weights
// and unicode ranges always match the package version. Astro writes the @font-face rules, copies
// the files under /_astro and generates metric-matched fallbacks, so text does not move when a
// face arrives. Both families are self-hosted: the page requests no other origin.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fontProviders } from "astro/config";

const require = createRequire(import.meta.url);

/**
 * One subset of a fontsource package as a variant for Astro's font API.
 * @param {string} pkg e.g. "@fontsource/ibm-plex-mono"
 * @param {string} css the package's stylesheet for that weight, e.g. "400.css" or "wght.css"
 * @param {string} subset e.g. "latin"
 * @param {string} weight e.g. "400" or "400 700"
 * @param {string} style "normal" or "italic"
 */
function variant(pkg, css, subset, weight, style) {
    const text = readFileSync(require.resolve(`${pkg}/${css}`), "utf8");
    // Match the subset as a whole name segment: "-latin-" is a substring of "-latin-ext-", and the
    // extended block comes first in the package stylesheet.
    const wanted = new RegExp(`-${subset}-(wght|\\d{3})-normal\\.woff2$`);
    for (const block of text.split("@font-face").slice(1)) {
        const file = /url\(\.\/(files\/[^)]+\.woff2)\)/.exec(block)?.[1];
        if (!file || !wanted.test(file)) continue;
        const range = /unicode-range:\s*([^;]+);/.exec(block)?.[1];
        if (!range) continue;
        return {
            src: [`${pkg}/${file}`],
            weight,
            style,
            display: "swap",
            unicodeRange: range.split(",").map((part) => part.trim()),
        };
    }
    throw new Error(`fonts.mjs: ${pkg}/${css} has no ${subset} subset`);
}

/**
 * @param {string} name
 * @param {string} cssVariable
 * @param {string} pkg
 * @param {ReturnType<typeof variant>[]} variants
 * @param {string[]} fallbacks
 */
const family = (name, cssVariable, variants, fallbacks) => ({
    provider: fontProviders.local(),
    name,
    cssVariable,
    fallbacks,
    optimizedFallbacks: true,
    options: { variants },
});

export const fonts = [
    family(
        "Instrument Sans",
        "--font-sans",
        [
            variant(
                "@fontsource-variable/instrument-sans",
                "wght.css",
                "latin",
                "400 700",
                "normal",
            ),
        ],
        ["sans-serif"],
    ),
    family(
        "IBM Plex Mono",
        "--font-mono",
        [
            variant(
                "@fontsource/ibm-plex-mono",
                "400.css",
                "latin",
                "400",
                "normal",
            ),
            variant(
                "@fontsource/ibm-plex-mono",
                "500.css",
                "latin",
                "500",
                "normal",
            ),
        ],
        ["monospace"],
    ),
];
