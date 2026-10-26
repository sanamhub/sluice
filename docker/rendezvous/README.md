# rendezvous

The `rendezvous` image runs `iroh-relay` 1.4.1 with the relay half turned off. Peers behind NATs ask
it for their own public address over QUIC and it answers. It carries no transfer bytes, so the
smallest VPS runs it (ADR 0013). `iroh-relay` is the upstream binary, built from crates.io with
`--locked`. The version is the one `Cargo.lock` resolves for `iroh`, so the probe and the server
come from one release.

The `relay` image, which carries traffic and runs closed, is in [docker/relay](../relay/README.md).

## The guard

The entry point is [`sluice-relay-guard`](../guard/README.md), the relay image's guard, and the
default command is `--rendezvous --config-path /etc/rendezvous/manual.toml` (ADR 0020).
`enable_relay` defaults to true in `iroh-relay` 1.4.1, so a config with that line deleted would
turn this server into an open relay with no byte limit. In `--rendezvous` mode the guard refuses a
config that does not say `enable_relay = false`, and one that sets `access` or `[limits]`, which
only a relay reads. It applies every other rule of the relay's guard too: no unknown key, no
`dangerous_http_only = true`, and `enable_metrics` written out. A refusal is one line on stderr,
and the container exits with 64.

The guard then starts `iroh-relay --dev --config-path=/proc/self/fd/0`, with the sealed copy of the
bytes it checked on descriptor 0 (ADR 0021). `--dev` is the guard's, passed only in this mode and
only after the check: this config needs it
([below](#what-iroh-relay-140-does-with-the-relay-half-off)), and the guard refuses `--dev` given on
its command line.

**Overriding the entry point skips the guard.** `docker run --entrypoint /usr/local/bin/iroh-relay`
runs the server with whatever it is given.

## Environment variables

`iroh-relay` 1.4.1 reads four variables that change what the config says (`src/main.rs`, lines
41-47). With the relay half off, none of them changes what this server does:

| Variable | What it changes | Here |
|---|---|---|
| `IROH_RELAY_ACCESS_TOKEN` | replaces the `access.shared_token` list of a relay with one token | no relay and no `access`; the guard still refuses it set but empty |
| `IROH_RELAY_HTTP_BEARER_TOKEN` | replaces the bearer token of a relay's HTTP access check | no relay and no `access` |
| `IROH_RELAY_ACME_URL` | replaces the LetsEncrypt directory, for `cert_mode = "LetsEncrypt"` | `Manual` only: iroh-relay refuses LetsEncrypt with `--dev` (main.rs:714-716) |
| `IROH_RELAY_ACME_CA` | adds a CA the ACME server's certificate may chain to, for LetsEncrypt | as above |

**`RUST_LOG` at `debug` or `trace` prints the config.** iroh-relay logs the whole server config at
`debug` at start (main.rs:606). This config holds no token, but a relay's does, so the
[relay README](../relay/README.md#environment-variables) says to keep `RUST_LOG` at `info` or
above there. The image sets `RUST_LOG=info`.

## Build and run

```
docker build -f docker/rendezvous/Dockerfile -t sluice-rendezvous docker
```

The build context is `docker/`, for the guard's sources, and
[Dockerfile.dockerignore](Dockerfile.dockerignore) lets in only `manual.toml` and the guard's
manifest, lock file and sources.

The image is 63.1 MB, as `docker image ls` showed it on 2026-10-23, with the guard in it. It runs
as uid 65532 on `gcr.io/distroless/cc-debian13:nonroot`, which has no shell, and starts with a
read-only root. Both base images are pinned by digest in the [Dockerfile](Dockerfile).
[smoke-test.sh](../smoke-test.sh) checks that it starts and that the guard refuses a config
without `enable_relay = false`, and CI runs it on every build.

The default command runs [manual.toml](manual.toml), which reads `default.crt` and `default.key`
from `/etc/rendezvous/certs`. For a test, make a self-signed certificate. It has to say `CA:FALSE`:
`openssl req -x509` without it makes a CA certificate, and the client refuses that as a server
certificate with `CaUsedAsEndEntity` (seen with the probe, and discovery then times out after 10 s).

```
mkdir certs && cd certs
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout default.key -out default.crt -days 30 -subj '/CN=rendezvous.example.net' \
  -addext 'subjectAltName=DNS:rendezvous.example.net' \
  -addext 'basicConstraints=critical,CA:FALSE' \
  -addext 'keyUsage=digitalSignature' -addext 'extendedKeyUsage=serverAuth'
sudo chown 65532:65532 default.key && chmod 600 default.key
cd ..
docker run -d --name rendezvous -p 7842:7842/udp \
  -v "$PWD/certs:/etc/rendezvous/certs:ro" sluice-rendezvous
```

The key belongs to uid 65532, the user the server runs as, and no one else can read it. Point a
probe at it with the CA file:

```
probe serve --rendezvous https://rendezvous.example.net --rendezvous-ca certs/default.crt
```

The URL's port, if there is one, is the UDP port of address discovery, and it defaults to 7842. A
certificate from a public CA needs no `--rendezvous-ca`. That is the expected result and was not
tested here, because no public certificate was available.

## Certificates

The image takes `Manual` certificates only. `iroh-relay` 1.4.1 can fetch a LetsEncrypt certificate
itself, but only with the relay half on, because the challenge is answered on the relay's HTTPS
listener (below). A rendezvous that runs a relay listener is not the server ADR 0013 describes, so
the certificate comes from outside the container.

On the VPS, with nothing listening on TCP 80, `certbot` can fetch one with its own HTTP server and
copy it into place after each renewal:

```
sudo certbot certonly --standalone -d rendezvous.example.net --deploy-hook '
  install -o 65532 -g 65532 -m 600 "$RENEWED_LINEAGE/privkey.pem" \
    /srv/rendezvous/certs/default.key &&
  install -m 644 "$RENEWED_LINEAGE/fullchain.pem" /srv/rendezvous/certs/default.crt &&
  docker restart rendezvous'
```

Mount `/srv/rendezvous/certs` as `/etc/rendezvous/certs`. The server reads the files once at start,
so the hook restarts it. This recipe was not run here, because there was no public host name.

## What iroh-relay 1.4.1 does with the relay half off

Paths are in the `iroh-relay` 1.4.1 crate, `src/`. "Run" means it was started from this image and
observed. The start with `--dev` and the refusal without it were run on 1.4.1. The other runs were
made on 1.3.0, and the code they cite is the same in 1.4.1, with the line numbers moved to match.

**Ports.** One: UDP 7842, the QUIC address discovery port (`defaults.rs:7`, ALPN `/iroh-qad/0` at
`quic.rs:10`). Run: with `manual.toml` the container's namespace has no TCP listener and one UDP
socket on 7842. A third listener would be the metrics server, which defaults to on at `[::]:9090`
with no authentication (`main.rs:349-351`, `main.rs:783`). The config sets `enable_metrics = false`,
so it is off, as the run shows. This default was read from the source and not run.

**HTTPS server.** It does not run. `Server::spawn` builds the HTTP and HTTPS listeners only inside
`match config.relay` (`server.rs:713`), and the `None` arm returns no listener (`server.rs:834`).
There is no plain HTTP port and no captive portal check either. That is why the built-in LetsEncrypt
mode cannot run with the relay half off: its TLS-ALPN-01 challenge is answered on the HTTPS listener
(`server.rs:748-789`).

**The binary refuses this config without `--dev`.** `main.rs:767` builds the relay config only if
`enable_relay` is true, and drops the TLS config with it. In the normal path the QUIC server is
given no certificate of its own (`main.rs:699-702`), so `Server::spawn` finds none
(`server.rs:847-849`). Run: the upstream binary with `enable_relay = false`,
`enable_quic_addr_discovery = true` and a `Manual` certificate exits at once with `Failed to bind
QAD listener` and `TLS not configured`. The `--dev` branch is the only one that hands the
certificate to the QUIC server (`main.rs:711`). It accepts `Manual` only (`main.rs:714-716`), and
its other effect, plain HTTP for the relay, has nothing to switch here. The guard passes `--dev` for
this reason, in `--rendezvous` mode alone. It is an upstream bug, not a setting, and a later
release may fix it.

**Logs.** To stderr. The filter is `EnvFilter::from_default_env()` (`main.rs:575`), so with
`RUST_LOG` unset the server prints errors only. The image sets `RUST_LOG=info`, which prints one
line: `QUIC server listening on bind_addr=0.0.0.0:7842` (`quic.rs:119`). Each address-discovery
connection is logged at `debug` (`quic.rs:212`) with the client's address,
`qad-conn{remote_addr=172.17.0.1:43145}`. Run with `RUST_LOG=iroh_relay=debug`.

**What a client sees if it uses the relay half anyway.** Run, with the probe from this repository
(`iroh` 1.3.0 at the time) and a rendezvous URL as its only relay:

- The address-discovery round trip works: 0.37 s from binding to the server's answer on loopback.
- iroh adds the URL as its home relay, so the ticket carries `{"Relay":"https://127.0.0.1:7842/"}`.
- The relay WebSocket dial gets `Connection refused` on the TCP port of the URL, because nothing
  listens. iroh logs `Failed to connect to relay server: unable to connect: Connection refused` and
  retries with an exponential backoff from 10 ms, capped at 16 s before jitter, and no limit on
  attempts (`iroh` `src/socket/transports/relay/actor.rs:326-355` and `:392-398`). It keeps trying
  until the endpoint closes. Run for 75 s: 13 dials, with delays of 13 ms, 37 ms, 79 ms, 102 ms, 277
  ms, 551 ms, 1.0 s, 2.3 s, 4.3 s, 8.2 s, 10.6 s, 27.7 s and 20.9 s. Jitter takes a delay past the
  cap, so an idle endpoint makes one failed TCP connect every 16 to 32 s.
- The HTTPS latency probes to `/ping` fail the same way, at 200, 300 and 400 ms into the report.
- Neither the ticket nor the connection waited for it: the dial took 1.6 ms on loopback.

With a firewall that drops TCP instead of refusing it, each relay dial would wait for the 10 s
connect timeout (`actor.rs:87`) in the relay actor. Nothing in a connection waits on that actor.
This was read from the source and not run.

## The handshake is X25519, not post-quantum

The upstream binary installs `ring` as its TLS provider (`main.rs:584` and `main.rs:622`), and
`ring` has no `X25519MLKEM768`. Address discovery therefore negotiates X25519. We do not patch it.
The handshake carries the peer's observed address and moves no transfer data, and ADR 0018 covers
the transfer connections.

## Limits

- One QUIC address discovery server cannot tell whether a NAT maps a source port differently per
  destination. That needs a second server to compare against, and iroh reports it only with two
  relays in the map (`net_report/report.rs`, `mapping_varies_by_dest`).
- `accept_conn_limit` and `accept_conn_burst` are not implemented upstream (ADR 0013), and the
  address-discovery server has no connection limit of its own.
