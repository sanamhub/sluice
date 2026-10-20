# sluice-wasm-hash

Measures how fast one WebAssembly thread hashes with BLAKE3 in a browser. ADR 0014 builds the browser
engine with `blake3/wasm32_simd` and no threads, which holds only if that is faster than a relay an
operator would run. M0 measures it in Chrome, Firefox and Safari, on desktop and on iOS. This is the
page that does the measuring. The maintainer runs it on the devices and decides from the numbers.

The page reports each rate next to 500 Mbit/s, the browser target in
[`docs/performance.md`](../../docs/performance.md), and next to 1 Gbit/s. It decides nothing itself.

Nothing here is wired into a product crate. `sluice-wasm` comes later and reuses what is learnt.

| Path | What it is |
|---|---|
| `src/` | The Rust crate: a hasher fed from JavaScript, and the test-vector input held in WASM memory |
| `page/` | The Vite and TypeScript page, with Biome and Playwright. Node is needed for this directory only |
| `../../xtask/` | `cargo xtask wasm-hash`, which builds both WASM variants for the page |

## What is measured

Two builds of the same crate: `simd` (`wasm32_simd` and `+simd128`, the build ADR 0014 names) and
`scalar` (neither, which shows what SIMD is worth and what a browser without it would get). Each runs
three cases in a dedicated Worker, as the engine will. The input is the BLAKE3 test-vector pattern,
byte `i` being `i % 251`, so every root can be checked.

| Case | Size | What it includes |
|---|---|---|
| In memory | 1 GiB, 1 MiB chunks | The hash rate alone, on bytes already in WASM memory |
| Copied in | 1 GiB, 1 MiB chunks | The same, fed from a JavaScript `ArrayBuffer`, so it pays the copy into WASM memory that the engine pays |
| From a file | 256 MiB | A `Blob` read through `stream()`, as a dropped file would be, in whatever chunks the stream gives |

Each case runs one warm-up that is not counted, then five runs that are. The result gives the median
and the range in MB/s and Gbit/s (MB is 10^6 bytes), and the median as a multiple of 500 Mbit/s and of
1 Gbit/s. Every run, warm-up included, is in the JSON.

Before measuring, each build hashes 1025 and 102400 bytes of the pattern and compares the roots with
the official BLAKE3 test vectors. A mismatch stops the page with the error. Every timed run's root is
compared with a root computed apart from the timing, and the two builds must agree on every root.

A browser without WebAssembly SIMD, found with `WebAssembly.validate`, gets the `scalar` build only,
and the result says so. The page sets no COOP or COEP header and records `crossOriginIsolated`, which
must be `false`. If it is `true`, the page warns that the numbers are not what ADR 0014 assumes.

If a buffer cannot be allocated, the case retries at half the size, down to 64 MiB, and the result
records the size used and the sizes that failed. A browser can also kill a tab for memory without
throwing, and nothing can catch that. On a phone, start lower with `?mem_mib=256&file_mib=128`.

The page sends nothing anywhere. It loads nothing from a CDN.

## Build

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
cargo xtask wasm-hash
cd bench/wasm-hash/page
npm ci
npm run build
```

The `wasm-bindgen` CLI version has to equal the crate's. The task reads the crate's version from
`cargo metadata` and stops with the install command if they differ.

`cargo xtask wasm-hash` writes the two builds and `sizes.json` to `page/src/generated/`, which git
ignores, and prints each `.wasm` file's raw and brotli size. The budget in `docs/performance.md` is
1.5 MB brotli for the whole WASM build, and the sizes go into the result JSON.

## Run

```
cd bench/wasm-hash/page
npm run preview -- --host
```

`--host` serves the built page on the LAN. Vite prints a `Network:` URL. Open it on the device, tap
Run, and keep the tab in the foreground.

- **iPhone.** Open the `Network:` URL in Safari on the same Wi-Fi as the computer. A guest network that
  isolates clients will not work. Use the IP address, because Vite refuses host names it does not
  know.
- **Plain HTTP.** The page is served over HTTP and needs nothing else: WebAssembly and Workers work in
  an insecure context. `navigator.clipboard` does not, so Copy JSON falls back to selecting the text
  and copying it, which can fail on some browsers. Download JSON does not need the clipboard.
- **Query options.** `?mem_mib=` and `?file_mib=` set the sizes (defaults 1024 and 256, at most 4095),
  `?runs=` sets the counted runs (default 5), and `?autorun=1` starts without the button.

`npm run dev` serves unbuilt modules, so measure with the preview.
Close other tabs and applications first, and note the power mode. Throughput on a laptop on battery is
a different number.

## Tests

The crate's unit tests run with the rest of the workspace (`cargo nextest run`). They check the pattern's
root at 0, 1, 1024, 1025 and 102400 bytes against the official BLAKE3 vectors, and that chunked and
one-shot hashing agree for chunk sizes that do not divide the length.

The page has Playwright tests, in `page/tests/`:

```
cd bench/wasm-hash/page
npx playwright install chromium firefox webkit
npx playwright test
```

Each test runs the page headless at 64 MiB and checks that both builds finish with matching roots, that
the result has the right shape, and that the page asks for nothing but its own files. Playwright's
WebKit is not Safari, and the project is named `webkit-not-safari` for that reason. The numbers from
these runs, in CI or here, are not results. The only results are runs on the devices ADR 0014 names.

The workflow `.github/workflows/wasm-hash.yml` runs the build, Biome, `npm audit --audit-level=high` and
these tests, and only for changes under `bench/wasm-hash/`, `xtask/` or the workflow.
