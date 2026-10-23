# relay

The `relay` image runs `iroh-relay` 1.4.0 with both halves on: address discovery over QUIC, and the
relay over HTTPS that carries the traffic of two peers that cannot reach each other directly.
**This is the image with the bandwidth bill.** Every relayed byte of a transfer passes through the
host that runs it, in and out again (ADR 0013). For address discovery alone, run
[`rendezvous`](../rendezvous/README.md), which carries no transfer bytes.

`iroh-relay` is the upstream binary, built the way `rendezvous` builds it: from crates.io with
`--locked`, at the version `Cargo.lock` resolves for `iroh`, on the same two base images pinned by
the same digests. It runs as uid 65532 on `gcr.io/distroless/cc-debian13:nonroot`, which has no
shell, and the metrics server is off. It starts through [the guard](#the-guard), the one program of
ours in the image, which refuses a config that would open the relay by mistake.

The relay runs closed (`docs/design.md`, **Relays run closed**). Only the endpoint ids on its
allowlist may relay through it, and the image ships with that list empty, so it lets nobody in until
the operator adds an id.

## The guard

The entry point is [`sluice-relay-guard`](../guard/README.md) (ADR 0019), and the default command
is `--config-path /etc/relay/relay.toml`. It opens the config, checks it, copies the checked bytes
into a sealed `memfd`, puts that copy on descriptor 0, and then replaces itself with
`iroh-relay --config-path=/proc/self/fd/0` (ADR 0021). The relay so reads a file that cannot be
written, renamed over or truncated after the check, and it is PID 1 and gets the container's
signals.

Checked on 2026-10-23 in WSL with Docker 29.1.3, by [smoke-test.sh](../smoke-test.sh): PID 1, read
from the host, is `/usr/local/bin/iroh-relay --config-path=/proc/self/fd/0`, and its descriptor 0
is a `memfd:` whose bytes are the shipped `relay.toml`. The relay presented the certificate that
was mounted, on HTTPS 443, logged `QUIC server listening`, and had port 9090 closed. Those three
are what only that config asks for: with no config, iroh-relay 1.4.0 serves plain HTTP on 80, which
answers `/generate_204` all the same, and metrics on 9090, and no HTTPS or QUIC.

`iroh-relay` 1.4.0 runs open, with no byte limit and nothing in its log, when `access` is left out,
when a key is misspelled (it ignores keys it does not know), and when the config file is missing.
The guard refuses each of these, and any config that lets in ids nobody listed, unless the
operator passes `--open` and the config sets a per-client byte limit. A refusal is one line on
stderr that names the key or the rule, and the container exits with 64. Two of them, as the image
prints them:

| Started with | stderr |
|---|---|
| a mounted `relay.toml` that says `[acess]` | ``sluice-relay-guard: line 1: unknown key `acess`: iroh-relay 1.4.0 has no such setting, and would ignore it; check the spelling against docker/relay/README.md`` |
| `--config-path /etc/relay/open.toml` | ``sluice-relay-guard: `access` is "everyone", which lets anyone relay: list the ids that may relay in `[access] allowlist`, or start with --open to run it open on purpose`` |

It also refuses `--dev`, every argument but `--open` and `--config-path`, `--open` with a closed
config, and an allowlist entry that is not 64 hex digits. Since ADR 0020 it refuses
`dangerous_http_only = true`, a config with no `enable_metrics` line, an `access.http.url` with a
user name, password, query or fragment in it, and `enable_relay = false`, which is the rendezvous
image's config. The [guard's README](../guard/README.md) lists every rule. No message prints a
value from the config, since a token may be one.

**Overriding the entry point skips the guard.** `docker run --entrypoint /usr/local/bin/iroh-relay`
runs the server with whatever it is given, open default included. Name `sluice-relay-guard` instead,
or leave the entry point alone.

## Environment variables

`iroh-relay` 1.4.0 reads four variables that change what the config says (`src/main.rs`, lines
41-47 and where each is read). A `docker run -e` sets them past the file the guard checked.

| Variable | What it changes |
|---|---|
| `IROH_RELAY_ACCESS_TOKEN` | replaces the whole `access.shared_token` list with this one token, when `access` is shared tokens (main.rs:238). The guard reads it as iroh-relay does: set, it passes an empty list; set but empty, it is refused |
| `IROH_RELAY_HTTP_BEARER_TOKEN` | replaces `access.http.bearer_token`, the token sent to the HTTP access check (main.rs:230) |
| `IROH_RELAY_ACME_URL` | replaces the LetsEncrypt directory with another ACME server, for `cert_mode = "LetsEncrypt"` (main.rs:653) |
| `IROH_RELAY_ACME_CA` | adds a CA that the ACME server's own TLS certificate may chain to, for `cert_mode = "LetsEncrypt"` (main.rs:665) |

A token belongs in one of the first two rather than in the file, so that a copy of the config
holds no secret.

**`RUST_LOG` at `debug` or `trace` prints the config, tokens included.** At start iroh-relay logs
the whole server config at `debug` (main.rs:606). Its `RelayConfig` derives `Debug` with the access
check in it (`src/server.rs:126` and `147`), and that check derives `Debug` over every shared
token and the HTTP check's bearer token. This was read from the source and not run. The image sets
`RUST_LOG=info`. Keep it at `info` or above on a relay with tokens, or treat its log as a secret.

## Ports

| Port | What |
|---|---|
| TCP 443 | the relay, over HTTPS and WebSocket |
| TCP 80 | plain HTTP, serving only the captive portal check `/generate_204` that iroh clients make |
| UDP 7842 | address discovery over QUIC |

Checked by running the image: those three listeners and no other, and `/generate_204` answers 204.

## Build and run

```
docker build -f docker/relay/Dockerfile -t sluice-relay docker
```

The default command runs [relay.toml](relay.toml), which reads `default.crt` and `default.key` from
`/etc/relay/certs`. For a test, make a self-signed certificate as for `rendezvous`. It has to say
`CA:FALSE`, or the client refuses it as a server certificate:

```
mkdir certs && cd certs
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout default.key -out default.crt -days 30 -subj '/CN=relay.example.net' \
  -addext 'subjectAltName=DNS:relay.example.net' \
  -addext 'basicConstraints=critical,CA:FALSE' \
  -addext 'keyUsage=digitalSignature' -addext 'extendedKeyUsage=serverAuth'
sudo chown 65532:65532 default.key && chmod 600 default.key
cd ..
docker run -d --name relay -p 443:443 -p 80:80 -p 7842:7842/udp \
  -v "$PWD/certs:/etc/relay/certs:ro" sluice-relay
```

The key belongs to uid 65532, the user the server runs as, and no one else can read it. The server
reads the certificate once at start, so restart the container after a renewal.

## Let an endpoint in

The allowlist holds endpoint ids, the 64 hex characters that `sluice id` prints on each machine that
should use the relay, and the guard refuses an entry that is not 64 hex digits. The two below are
examples, the public halves of keys made for the check and thrown away. Copy
[relay.toml](relay.toml) next to the certificates, add the ids, mount it over the one in the image,
and restart:

```toml
[access]
allowlist = [
  "68852edc24884118b7c22e87f75094674a9b3268ba83b3005e4da869de4ed964",
  "f0b7855c2e023a888cb42a8934999b57949acf835ad8755bfd61cb5a8548e1c0",
]
```

```
docker rm -f relay
docker run -d --name relay -p 443:443 -p 80:80 -p 7842:7842/udp \
  -v "$PWD/certs:/etc/relay/certs:ro" -v "$PWD/relay.toml:/etc/relay/relay.toml:ro" sluice-relay
```

An endpoint off the list gets `The relay denied our authentication (not authorized)` when it
connects, and the server logs a warning with the client's address. The list controls relaying only.
Address discovery on UDP 7842 answers anyone.

**How this was checked**, on 2026-10-22 in WSL with Docker 29.1.3, with a throwaway client built on
`iroh-relay` 1.4.0's own `ClientBuilder` and three random keys:

- with the image as shipped, the first key was refused with the line above;
- with the first two keys on the list, the first sent a datagram to the second through the relay
  and the second received it, and the third key was refused;
- with `--open --config-path /etc/relay/open.toml`, the third key connected and relayed a datagram.

It was run again through the guard, with new keys, and came out the same, with `iroh-relay` as the
container's PID 1. On 2026-10-23 it was run a third time with the config handed over and the
container's root read-only, and came out the same again, with `iroh-relay` started as
`--config-path=/dev/stdin` as the guard did then; ADR 0021 moved that to a sealed copy read through
`/proc/self/fd/0`. [smoke-test.sh](../smoke-test.sh) checks the guard's refusals, that both images
start as the config they were given with a read-only root, and what PID 1 is, and CI runs it on
every build of either image.

## Open mode

[open.toml](open.toml) lets anyone relay, and each client may send at most 2 MiB/s into the relay,
with bursts of up to 8 MiB. It is used only when the operator names it on the command line, with
`--open` to say it is meant:

```
docker run -d --name relay -p 443:443 -p 80:80 -p 7842:7842/udp \
  -v "$PWD/certs:/etc/relay/certs:ro" sluice-relay --open --config-path /etc/relay/open.toml
```

Without `--open` the guard refuses `open.toml`, and with `--open` it refuses `relay.toml`, so the
flag and the config always agree. An open config of the operator's own must set
`[limits.client.rx] bytes_per_second`, or the guard refuses it.

What it costs: anyone who finds the URL, in a ticket, a log or a screenshot, can use the operator's
bandwidth up to that rate, as many clients at once as care to. `iroh-relay` 1.4.0 has no working
limit on connections (`accept_conn_limit` and `accept_conn_burst` are documented upstream as not
implemented, ADR 0013), so the per-client byte limit is the only limit there is. The limit on
connections and on total bandwidth has to come from the host, such as a firewall or the provider's
traffic cap.

## Certificates

The image takes `Manual` certificates, as `rendezvous` does. With the relay half on, `iroh-relay`
1.4.0 can also fetch a LetsEncrypt certificate itself: set `cert_mode = "LetsEncrypt"`, `hostname`
and `contact` in `[tls]`, and keep `cert_dir` on a mounted volume, where it caches the certificate.
Its challenge is TLS-ALPN-01, answered on the HTTPS listener (`src/server/http_server.rs:1095`), so
TCP 443 must be reachable from the internet. TCP 80 is not used for the challenge. This mode was
read from the source and not run, because there was no public host name.

With a certificate from outside, the `certbot` recipe of the
[`rendezvous` README](../rendezvous/README.md#certificates) works with one change: `--standalone`
needs TCP 80, which the relay holds, so add `--pre-hook 'docker stop relay'` and end the deploy hook
with `docker start relay`. This recipe was not run either.

## A config with no `[tls]`

The guard lets a config with no `[tls]` table start (ADR 0020), for a relay behind a reverse proxy.
Then iroh-relay serves plain HTTP only, and **the proxy in front must end TLS**: a client reaches
the proxy over HTTPS, and the proxy reaches the relay over HTTP. Whatever the clients present
crosses that second hop in clear text, shared tokens included. Keep it on one host or one private
network, and never on a path another party can read. The guard refuses `dangerous_http_only =
true`, which turns HTTPS off on a relay that has a `[tls]` table, but it cannot see what is in
front of one that has none.
