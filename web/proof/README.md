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

Put the relay's config and the file a send serves in a folder outside this repository, so a run leaves
nothing in the tree. This recipe calls that folder `$SCRATCH`.

The relay. `--dev` fills in a bind address only when the config has none, and its own default is
IPv6-only on Windows, so the config names IPv4:

```toml
# $SCRATCH/relay.toml
http_bind_addr = "0.0.0.0:3340"
enable_metrics = false
```

```sh
iroh-relay --dev -c "$SCRATCH/relay.toml"
```

The sender, with a file of at least a megabyte so the ticket names the file's own blob rather than a
pack:

```sh
sluice send "$SCRATCH/payload.bin" --relay http://127.0.0.1:3340/
```

The page reads `relay`, `sender`, `root` and `size` from its fragment, so the ticket's parts have
to be extracted, and `sluice` has no command that prints them. Write the ticket to a file and pipe it
in, because what the command prints is the ticket by another spelling:

```sh
cargo xtask ticket-parts < "$SCRATCH/ticket.txt"
# the values below are examples, not the ones a run makes
# sender 1111111111111111111111111111111111111111111111111111111111111111
# relay http://127.0.0.1:3340/
# root 2222222222222222222222222222222222222222222222222222222222222222
# size 2097152
```

Serve the page, from `web/` alone so nothing else in the tree is reachable:

```sh
cd web
python -m http.server 8099 --bind 127.0.0.1
chrome --headless=new --no-sandbox --remote-debugging-port=9222 \
  "http://127.0.0.1:8099/proof/index.html#relay=http://127.0.0.1:3340/&sender=<64 hex>&root=<64 hex>&size=<the ticket's size>"
```

The parts are in the fragment, after the `#`, and not in the query string. A fragment is not part of
a request, so the server the page came from, and any proxy or log between them, never sees the
sender or the root. The browser's command line holds the parts, though, so anything that lists the
processes of the host reads them for as long as the run lasts, and the page clears the fragment from
its own address bar and history entry once it has read them.

`--dump-dom` returns before a fetch of two megabytes finishes, so the result is read from the live
page over the DevTools protocol rather than from a dump. The body says `sequence 64 bytes`, then the
file's root, then the file's length, its SHA-256 and how long it took.

The page's script is `main.js`, a file beside the page rather than a block inside it: the page is
served with a policy that allows scripts only from its own origin, and a policy that also allowed an
inline block would allow any injected one.

## What it assumes

The page reads one root out of the sequence, the one after the manifest root, so the sender's content
is one file. A folder ticket names several roots and needs the manifest read to know which are files
and what they are called; that is the second step, not this page.

## What it proved

A 2 MiB file, 1,654 ms, `sha256 dac7494485cf5bce3989b90f0135d5cc2f68ae532a3e9b7fad4b9ce2f4dba88c`,
which is the sender's file's own digest. The sender's log said `connected through a relay`, and every
block was verified against the file's BLAKE3 root before the page saw it.
