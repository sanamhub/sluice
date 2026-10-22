# sluice-relay-guard

The entry point of both images, [`relay`](../relay/README.md) and
[`rendezvous`](../rendezvous/README.md) (ADR 0019, ADR 0020). It checks the config that
`iroh-relay` 1.4.0 is about to start with, refuses one that would open more than the operator
wrote, and then replaces itself with `/usr/local/bin/iroh-relay --config-path=/dev/stdin`, so the
server runs as PID 1 and gets the container's signals.

```
sluice-relay-guard [--open | --rendezvous] --config-path <file>
```

| Mode | Flag | Starts a config that |
|---|---|---|
| closed | none | relays only for an allowlist, an HTTP check or shared tokens |
| open | `--open` | lets anyone relay, with `limits.client.rx.bytes_per_second` set |
| rendezvous | `--rendezvous` | says `enable_relay = false`, and sets no `access` and no `[limits]` |

## Why it exists

`iroh-relay` 1.4.0 opens the relay to anyone, with no byte limit and nothing in its log, in three
ways (`src/main.rs` of that crate):

- a missing `access` setting is read as everyone (`#[serde(default)]`, main.rs:157);
- a key it does not know is ignored, so `[acess]` is read as no access setting at all;
- a config path that does not exist, or no `--config-path`, runs its open default
  (main.rs:542-554).

`enable_relay` defaults to true (main.rs:107), so a rendezvous server whose `enable_relay = false`
line is deleted becomes an open relay. `enable_metrics` defaults to true (main.rs:145), with no
login on the metrics port.

## What it refuses

Every refusal is one line on stderr, starting `sluice-relay-guard:`, and exits with 64, `EX_USAGE`.
No message prints a value from the config: a `shared_token` entry, `access.http.bearer_token` and
the query of `access.http.url` can be secrets.

| Refused | Why |
|---|---|
| no `--config-path`, a path that does not exist, a folder | iroh-relay runs open without a file |
| `--dev`, and any argument but `--open`, `--rendezvous` and `--config-path` | `--dev` turns TLS off and runs on any config |
| `--open` and `--rendezvous` together | a rendezvous server has nothing to open |
| a key iroh-relay 1.4.0 does not have, at any level | iroh-relay would ignore it |
| text that is not TOML, or a value of the wrong type | named by line and key, never by value |
| `access` missing, `"everyone"` or a denylist, without `--open` | it lets in ids nobody listed |
| `--open` without `limits.client.rx.bytes_per_second` | ADR 0011: open needs a per-client byte limit |
| `--open` with an allowlist, an HTTP check or shared tokens | the flag must mean what it says |
| an allowlist entry that is not 64 hex digits | the README says to paste what `sluice id` prints |
| `[tls] dangerous_http_only = true` (ADR 0020) | it turns HTTPS off, as `--dev` does; no `[tls]` at all still starts |
| no `enable_metrics` line (ADR 0020) | iroh-relay then serves metrics on port 9090 with no login; `true` and `false` both start |
| an `access.http.url` that is not an absolute http or https URL, or holds a user name, password, query or fragment (ADR 0020) | iroh-relay logs the URL when a check fails; a secret goes in `bearer_token`, a header |
| `--rendezvous` without `enable_relay = false` written out (ADR 0020) | iroh-relay turns the relay half on by default |
| `--rendezvous` with `access` or `[limits]` | they govern only a relay half, so the config was written for a relay |
| `enable_relay = false` without `--rendezvous` | it is a rendezvous config, and the flag must say so |

An empty allowlist starts: it is what the relay image ships, and it lets nobody in. If iroh-relay
itself cannot be started, the guard exits with 69, `EX_UNAVAILABLE`.

## The relay reads the file the guard checked

The guard opens the config once, checks that the open file is a regular file, reads it, and hands
that same open file to iroh-relay as its standard input (ADR 0020). iroh-relay is told to read
`/dev/stdin`, which on Linux opens that file again from its start. A rename over the path between
the two reads, such as a Kubernetes ConfigMap update, so cannot start a file nobody checked. An
edit in place, in the instant between them, still could: standard input pins the file, not its
bytes.

iroh-relay 1.4.0 reads nothing else from standard input: no line of its `src/` reads `stdin`. In
the image, `/dev/stdin` is the runtime's link to `/proc/self/fd/0`, and for the relay that is the
config file (checked in WSL with Docker 29.1.3, see [the relay README](../relay/README.md)).

The guard starts the relay only on Unix. Elsewhere `/dev/stdin` does not exist, iroh-relay would
find no config there and run its open default, so the guard stops after the check with exit 69.

## `--dev` in rendezvous mode

The guard refuses `--dev` on its command line, and passes it to iroh-relay itself in rendezvous
mode, after checking that the relay half is off. iroh-relay 1.4.0 hands the address-discovery QUIC
server a certificate only on the path `--dev` takes (main.rs:593-600 and 703-721), and without it
refuses a config with the relay half off: `Failed to bind QAD listener` and `TLS not configured`
(see [the rendezvous README](../rendezvous/README.md)). Its other effects, plain HTTP for the
relay and a default HTTP port, have nothing to act on when no relay runs.

## What it must never do

- Change the config, or start iroh-relay with anything but the open file it checked.
- Print a value from the config.
- Accept a key because a newer iroh-relay might have it. The schema is 1.4.0's and no other.
- Pass `--dev` to a server whose relay half is on.

## Moving iroh-relay on

`schema.rs` mirrors `Config` and every type it holds in `iroh-relay` 1.4.0's `src/main.rs`, and
names the lines of each. A new release may add a key, rename one, or change what a default means,
so before moving `IROH_RELAY_VERSION`:

1. Read the new release's `src/main.rs`, from `struct Cli` down to `build_relay_config`, and diff
   it against 1.4.0's. Check that `--dev` still does nothing with the relay half off but hand the
   QUIC server its certificate.
2. Bring `schema.rs` in line, field by field, with the new line numbers, and copy the release's own
   example configs into `testdata/iroh-relay-<version>/`.
3. Move `IROH_RELAY_VERSION` in `src/lib.rs`, the `ARG` in both Dockerfiles, and `iroh-relay` in
   the root `Cargo.lock` together. Two tests fail until all three agree, and a third until every
   crate in this project's `Cargo.lock` is one the root lock has at the same version.

## Building and testing

It is its own project with its own `Cargo.lock`, outside the root workspace, so each image builds
from `docker/` with only the files it needs: each Dockerfile has a `Dockerfile.dockerignore` beside
it that lists them. ci.yml runs its gates; to run them by hand from the repository root:

```
cargo fmt --manifest-path docker/guard/Cargo.toml --check
cargo clippy --manifest-path docker/guard/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path docker/guard/Cargo.toml --locked
cargo deny --manifest-path docker/guard/Cargo.toml check
```

The fuzz target `relay_config` in `fuzz/` reads any bytes as a config, in each mode. `cargo xtask
fuzz-seeds relay_config <dir>` writes its seeds: the three shipped configs and the iroh-relay
examples.
