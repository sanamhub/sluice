# sluice-relay-guard

The entry point of both images, [`relay`](../relay/README.md) and
[`rendezvous`](../rendezvous/README.md) (ADR 0019, ADR 0020, ADR 0021). It checks the config that
`iroh-relay` 1.4.0 is about to start with, refuses one that would open more than the operator
wrote, and then replaces itself with `/usr/local/bin/iroh-relay --config-path=/proc/self/fd/0`,
with a sealed copy of the bytes it checked on descriptor 0, so the server runs as PID 1 and gets
the container's signals.

```
sluice-relay-guard [--open | --rendezvous | --behind-proxy] --config-path <file>
```

| Mode | Flag | Starts a config that |
|---|---|---|
| closed | none | relays only for an allowlist, an HTTP check or shared tokens |
| open | `--open` | lets anyone relay, with `limits.client.rx.bytes_per_second` set |
| rendezvous | `--rendezvous` | says `enable_relay = false`, and sets no `access` and no `[limits]` |

`--behind-proxy` is not a mode: it is added to closed or open mode when the config has no `[tls]`
table and a proxy in front ends TLS (ADR 0021). It is refused with `[tls]`, and with
`--rendezvous`.

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
the query of `access.http.url` can be secrets. A key the guard does not know is named only when it
is 1 to 32 lower-case ASCII letters or `_`, which every iroh-relay 1.4.0 setting is; any other is
"a key", with its line, since it may be a token pasted in the wrong place. A hex or base64url key
has digits or capitals, or is longer, so it is not printed. A token of lower-case letters alone, 32
characters or fewer, would be: no rule can tell it from a setting. An unknown argument is named
only when it is `--` and 1 to 32 lower-case letters or `-`; any other is "an argument", with its
position, so a token on the command line stays out of the log. A `--config-path` value is printed
only when it starts with `/` or `./`, is at most 200 characters, and uses only `[A-Za-z0-9._/-]`;
any other is "the path given to --config-path". Every control character in a message, from a path or
an argument, is escaped, so a refusal is one line.

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
| a relay config with no `[tls]` table, without `--behind-proxy` | iroh-relay serves plain HTTP, and a shared token or bearer token would cross the network in clear text (ADR 0021) |
| `--behind-proxy` with a `[tls]` table | iroh-relay ends TLS itself, so the flag and the config disagree |
| `--behind-proxy` with `--rendezvous` | a rendezvous server relays nothing and answers no token |
| an allowlist or denylist entry that is not 64 lower-case hex digits of an ed25519 public key | iroh-relay refuses to start on an id it cannot read, and the README says to paste what `sluice id` prints |
| an empty `access.shared_token` list, or an empty token in it, unless `IROH_RELAY_ACCESS_TOKEN` is set | iroh-relay refuses both at start, and the variable replaces the list |
| `IROH_RELAY_ACCESS_TOKEN` set but empty, in every mode | iroh-relay would take the empty value as the only token |
| `[tls] dangerous_http_only = true` (ADR 0020) | it turns HTTPS off, as `--dev` does; a config with no `[tls]` needs `--behind-proxy` instead |
| no `enable_metrics` line (ADR 0020) | iroh-relay then serves metrics on port 9090 with no login; `true` and `false` both start |
| an `access.http.url` that is not an absolute http or https URL, or holds a user name, password, query or fragment (ADR 0020) | iroh-relay logs the URL when a check fails; a secret goes in `bearer_token`, a header |
| `--rendezvous` without `enable_relay = false` written out (ADR 0020) | iroh-relay turns the relay half on by default |
| `--rendezvous` with `access` or `[limits]` | they govern only a relay half, so the config was written for a relay |
| `enable_relay = false` without `--rendezvous` | it is a rendezvous config, and the flag must say so |

An empty allowlist starts: it is what the relay image ships, and it lets nobody in. If iroh-relay
itself cannot be started, the guard exits with 69, `EX_UNAVAILABLE`.

## The relay reads a sealed copy of the bytes the guard checked

The guard opens the config once, checks that it is a regular file, reads it, and copies those bytes
into a `memfd` (ADR 0021). It seals the copy against a write, a growth, a shrink and any further
seal, puts it on descriptor 0 with `dup2`, and closes the file it opened. Before it replaces itself
it checks that `/proc/self/fd/0` names the copy by device and inode, and reads the bytes back
through it. Only then does it exec iroh-relay, told to read `/proc/self/fd/0`.

A rename over the path cannot reach the relay, and neither can an edit in place, which ADR 0020
accepted a window for. Nor can a write that truncates the file before it writes, as `cat >` or a
generator script does, which would leave iroh-relay an empty file and `toml::from_str("")` is its
open default.

**The path must resolve, or the relay runs open.** iroh-relay tests its config path with
`exists()`, which is false on any error, and then runs its open default: relay on, everyone, no
limit (main.rs:542-567). The check before the exec proves that `/proc` is mounted and gives this
process's own descriptors. If it does not, the guard prints one line naming the step that failed
and exits with 69, and starts nothing. `/proc/self/fd/0` is a path of the kernel's, and not
`/dev/stdin`, which is a link the container runtime makes and some runtimes do not.

The guard starts the relay only on Linux, where `memfd_create` and its seals exist (Linux 3.17).
Elsewhere it stops after the check with exit 69.

iroh-relay 1.4.0 reads nothing from standard input: no line of its `src/` reads `stdin`.

## `--dev` in rendezvous mode

The guard refuses `--dev` on its command line, and passes it to iroh-relay itself in rendezvous
mode, after checking that the relay half is off. iroh-relay 1.4.0 hands the address-discovery QUIC
server a certificate only on the path `--dev` takes (main.rs:593-600 and 703-721), and without it
refuses a config with the relay half off: `Failed to bind QAD listener` and `TLS not configured`
(see [the rendezvous README](../rendezvous/README.md)). Its other effects, plain HTTP for the
relay and a default HTTP port, have nothing to act on when no relay runs: with `enable_relay =
false`, `Server::spawn` builds no HTTP or HTTPS listener (`server.rs:713-835`), so `--dev` here only
hands the certificate to QUIC. That is why the guard accepts it in this mode and no other.

## What it must never do

- Change the config, or start iroh-relay with anything but the sealed copy of the bytes it checked.
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
it that lists them, which BuildKit reads. The legacy builder (`DOCKER_BUILDKIT=0`) reads only
`docker/.dockerignore`, so that holds the union of the two lists, and each Dockerfile copies only
`guard/Cargo.toml`, `guard/Cargo.lock` and `guard/src`. Tests check both.

ci.yml runs its gates; to run them by hand from the repository root:

```
cargo fmt --manifest-path docker/guard/Cargo.toml --check
cargo clippy --manifest-path docker/guard/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path docker/guard/Cargo.toml --locked
cargo deny --manifest-path docker/guard/Cargo.toml check
```

Two fuzz targets are in `fuzz/`. `relay_config` reads any bytes as a config, in each mode, with and
without `--behind-proxy`, and as a command line split at each NUL. It asserts that no refusal holds
a control character or prints a key or an argument outside the rules above, and that every
backticked span of a config refusal is fixed text of the guard's or a dotted setting name of 1 to 32
lower-case letters or `_`. `relay_differential` reads the same bytes through the guard and through a
loose mirror of how iroh-relay 1.4.0 reads them, and asserts that whatever the guard lets start is
what iroh-relay would run: closed means a closed `access`, rendezvous means the relay half is off,
`enable_metrics` was written, `dangerous_http_only` is not true, an open config has a per-client
byte limit, a relay config's `[tls]` agrees with `--behind-proxy`, and an HTTP check's URL holds no
user name, password, query or fragment. Its first byte says whether `IROH_RELAY_ACCESS_TOKEN` is
unset, set, or set but empty, which the guard must refuse, and the rest is the config.

`cargo xtask fuzz-seeds relay_config <dir>` writes the seeds of the first: the three shipped
configs, the iroh-relay examples, and six configs one line from a rule, such as an open config
without a limit. For `relay_differential` it writes each of them twice, with `0x00` and `0x01` in
front: every shipped config starts with `#`, and as the first byte it would leave a comment cut in
two, so no rule would run. With only these seeds, deleting `check_rate`, `check_metrics` or
`check_tls` in a scratch copy makes the target fail on its first run.
