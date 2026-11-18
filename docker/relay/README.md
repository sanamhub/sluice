# relay

The `relay` image runs `sluice-relay`, our binary around `iroh-relay` 1.4.1. It serves the relay at
`/relay` and the short-code **meeting place** at `GET /meet` on one HTTPS listener, the plain-HTTP
captive-portal check on port 80, QUIC address discovery on UDP 7842, and metrics only when the
config asks for them. It starts through [`sluice-relay-guard`](../guard/README.md), which refuses a
config that would open the relay by mistake (ADR 0019, ADR 0020, ADR 0021).

The image runs closed (`docs/design.md`, **Relays run closed**). Only the endpoint ids on its
allowlist may relay through it, and the image ships with that list empty. An operator adds the id
`sluice id` prints and restarts.

## Two words that are not one thing

- A **meeting point** is the ADR 0028 relay recipe: a relay with a small per-client byte rate, so
  two peers can coordinate a hole punch through it and then transfer directly. It is the relay.
- A **meeting place** is ADR 0041's `/meet`: two live WebSocket connections that run SPAKE2 on a
  short code, and the relay forwards a few frames between them. It stores nothing. It is configured
  by `[meet]` and is not a second relay.

Both may be on in one process; neither replaces the other.

## The meeting place

`[meet]` in `relay.toml`:

```toml
[meet]
enable = true
rate_per_ip = 30
max_plates = 256
ttl_seconds = 600
```

`rate_per_ip` is claims and joins per minute from one IP, 1..=600; `max_plates` is the number held
at once, 1..=9999; `ttl_seconds` is a plate's life, 60..=600. The guard refuses `enable = true`
without all three, or with one outside those ranges. A config without `[meet]` serves `/relay` only
and answers `/meet` with `404`.

The server never parses a code, a word, a ticket, a SPAKE2 message or a sealed value. A refusal is
logged by its error code only; the nameplate is not in the line. `/meet` and `/relay` share the
host, port and certificate, so the browser tab of ADR 0040 can reach the same URL with `wss`.

`cert_mode = "LetsEncrypt"` is refused when `[meet]` is enabled: ACME is not carried into
`sluice-relay` yet. The shipped recipes use `cert_mode = "Manual"`.

## The guard

The entry point is [`sluice-relay-guard`](../guard/README.md). It opens the config, checks it, copies
the checked bytes into a sealed `memfd`, puts that copy on descriptor 0, and replaces itself with
`sluice-relay --config-path=/proc/self/fd/0` (ADR 0021). The server reads a file that cannot be
written, renamed over or truncated after the check, and is PID 1 with the container's signals.

**Overriding the entry point skips the guard.** `docker run --entrypoint /usr/local/bin/sluice-relay`
runs the server with whatever it is given. Name `sluice-relay-guard` instead, or leave the entry
point alone.

## Ports

| Port | What |
|---|---|
| TCP 443 | `/relay` and `/meet`, over HTTPS and WebSocket |
| TCP 80 | plain HTTP, serving only `/generate_204` when TLS is on |
| UDP 7842 | address discovery over QUIC |
| TCP 9090 | metrics, only when `enable_metrics = true` |

## Build and run

Build from the repository root, which holds `crates/sluice-meet` as well as both workspaces:

```
docker build -f docker/relay/Dockerfile -t sluice-relay .
```

For a test, make a self-signed certificate that says `CA:FALSE`:

```
mkdir certs && cd certs
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout default.key -out default.crt -days 30 -subj '/CN=relay.example.net' \
  -addext 'subjectAltName=DNS:relay.example.net' \
  -addext 'basicConstraints=critical,CA:FALSE' \
  -addext 'keyUsage=digitalSignature' -addext 'extendedKeyUsage=serverAuth'
chmod 644 default.key
cd ..
docker run -d --name relay -p 443:443 -p 80:80 -p 7842:7842/udp \
  -v "$PWD/certs:/etc/relay/certs:ro" sluice-relay
```

`relay.toml` is the closed recipe and ships empty. [meeting-point.toml](meeting-point.toml) adds the
ADR 0028 per-client rate. [open.toml](open.toml) lets anyone relay at 2 MiB/s; it is used only with
`--open`:

```
docker run -d --name relay -p 443:443 -p 80:80 -p 7842:7842/udp \
  -v "$PWD/certs:/etc/relay/certs:ro" \
  -v "$PWD/docker/relay/open.toml:/etc/relay/relay.toml:ro" sluice-relay \
  --open --config-path /etc/relay/relay.toml
```

## Access

`[access] allowlist` holds 64 lower-case hex characters, the endpoint ids `sluice id` prints. The
guard refuses anything else. `access = "everyone"` needs `--open` and a per-client byte limit under
`[limits.client.rx]`. Shared tokens, an HTTP check and a denylist are the other forms iroh-relay
1.4.1 has; the guard's README lists the rules.

`RUST_LOG` at `debug` or `trace` makes iroh-relay print its whole server config, tokens included.
The image sets `RUST_LOG=info`; keep it there on a relay with tokens. The meeting place logs no
code, nameplate, ticket, SPAKE2 message or sealed value at any level.

## Certificates and ACME

The shipped recipes use `cert_mode = "Manual"` and read `default.crt` and `default.key` from
`cert_dir`. The server reads them once at start, so restart the container after a renewal. The
meeting place is served from the same certificate.

`cert_mode = "LetsEncrypt"` is not carried by `sluice-relay` yet; with `[meet]` enabled the guard
refuses it with a line that names the limit. A config with no `[tls]` is served as plain HTTP and is
refused unless `--behind-proxy` says a proxy in front ends TLS.

## Smoke test

`docker/smoke-test.sh relay <image>` checks that the image starts with a read-only root as the
config it was given, that PID 1 is `sluice-relay` with the sealed copy on descriptor 0, that
`/generate_204` answers 204, that `/meet` claims and joins a nameplate on the relay's own port and
certificate, that metrics are off, and that `docker stop` returns quickly. It also checks each
refusal the guard makes, `[meet]`'s limits and the ACME refusal included.
