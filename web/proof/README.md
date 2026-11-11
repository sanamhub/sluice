# The first proof

The first step, as a page and a recipe: a tab fetches a file from a native `sluice send` through a
local relay and reports what it got. It is the run `docs/browser.md` records, kept so it can be
repeated without inventing the harness again.

What it is not: a test. Nothing runs this in CI, and the page has no UI, no state and no error
handling beyond one line. The app is what turns this into a program.

## The recipe

The crate builds for the target, which needs a wasm-capable C compiler for `ring`. On a machine with
LLVM, `clang` is enough. Without one, `crates/sluice-wasm/README.md` has the `zig cc` flags that work.

```sh
rustup target add wasm32-unknown-unknown
cargo build -p sluice-wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir web \
  target/wasm32-unknown-unknown/debug/sluice_wasm.wasm
```

The relay. `--dev` fills in a bind address only when the config has none, and its own default is
IPv6-only on Windows, so the config names IPv4:

```toml
# relay.toml
http_bind_addr = "0.0.0.0:3340"
enable_metrics = false
```

```sh
iroh-relay --dev -c relay.toml
```

The sender, with a file of at least a megabyte so the ticket names the file's own blob rather than a
pack:

```sh
sluice send payload.bin --relay http://127.0.0.1:3340/
```

The page reads `relay`, `sender`, `root` and `size` from its query string, so the ticket's parts have
to be extracted, and `sluice` has no command that prints them. Write the ticket to a file and pipe it
in, because what the command prints is the ticket by another spelling:

```sh
cargo xtask ticket-parts < ticket.txt
# sender 7605eeacd2f6bc4abeea76b3e928974cd463e0e9e6af7a283719d6fb00656615
# relay http://127.0.0.1:3340/
# root 6fa9106fef96e35f2ab07a303638db7c48ade9840f7336acf37384795b189adc
# size 2097152
```

Serve the page, with the glue written to `web/` as above:

```sh
python -m http.server 8099 --bind 127.0.0.1
chrome --headless=new --no-sandbox --remote-debugging-port=9222 \
  "http://127.0.0.1:8099/proof/index.html?relay=http://127.0.0.1:3340/&sender=<64 hex>&root=<64 hex>"
```

`--dump-dom` returns before a fetch of two megabytes finishes, so the result is read from the live
page over the DevTools protocol rather than from a dump. The body says `sequence 64 bytes`, then the
file's root, then the file's length, its SHA-256 and how long it took.

## What it assumes

The page reads one root out of the sequence, the one after the manifest root, so the sender's content
is one file. A folder ticket names several roots and needs the manifest read to know which are files
and what they are called; that is the second step, not this page.

## What it proved

A 2 MiB file, 1,654 ms, `sha256 dac7494485cf5bce3989b90f0135d5cc2f68ae532a3e9b7fad4b9ce2f4dba88c`,
which is the sender's file's own digest. The sender's log said `connected through a relay`, and every
block was verified against the file's BLAKE3 root before the page saw it.
