# rendezvous

The `rendezvous` image runs `iroh-relay` 1.3.0 with the relay half turned off. Peers behind NATs ask it
for their own public address over QUIC and it answers. It carries no transfer bytes, so the smallest VPS
runs it (ADR 0013). It is the upstream binary, built from crates.io with `--locked`, and nothing of ours.
The version is the one `Cargo.lock` resolves for `iroh`, so the probe and the server come from one
release.

The `relay` image, which carries traffic, is not here. It needs the authorization design of ADR 0011.

## Build and run

```
docker build -t sluice-rendezvous docker/rendezvous
```

The image is 61.2 MB. It runs as uid 65532 on `gcr.io/distroless/cc-debian13:nonroot`, which has no
shell. Both base images are pinned by digest in the [Dockerfile](Dockerfile).

The default command runs [manual.toml](manual.toml), which reads `default.crt` and `default.key` from
`/etc/rendezvous/certs`. For a test, make a self-signed certificate. It has to say `CA:FALSE`:
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

The key belongs to uid 65532, the user the server runs as, and no one else can read it. Point a probe at it with the CA file:

```
probe serve --rendezvous https://rendezvous.example.net --rendezvous-ca certs/default.crt
```

The URL's port, if there is one, is the UDP port of address discovery, and it defaults to 7842. A
certificate from a public CA needs no `--rendezvous-ca`. That is the expected result and was not tested
here, because no public certificate was available.

## Certificates

The image takes `Manual` certificates only. `iroh-relay` 1.3.0 can fetch a LetsEncrypt certificate
itself, but only with the relay half on, because the challenge is answered on the relay's HTTPS
listener (below). A rendezvous that runs a relay listener is not the server ADR 0013 describes, so the
certificate comes from outside the container.

On the VPS, with nothing listening on TCP 80, `certbot` can fetch one with its own HTTP server and copy
it into place after each renewal:

```
sudo certbot certonly --standalone -d rendezvous.example.net   --deploy-hook 'install -o 65532 -g 65532 -m 600 "$RENEWED_LINEAGE/privkey.pem" /srv/rendezvous/certs/default.key &&
                 install -m 644 "$RENEWED_LINEAGE/fullchain.pem" /srv/rendezvous/certs/default.crt &&
                 docker restart rendezvous'
```

Mount `/srv/rendezvous/certs` as `/etc/rendezvous/certs`. The server reads the files once at start, so
the hook restarts it. This recipe was not run here, because there was no public host name.

## What iroh-relay 1.3.0 does with the relay half off

Paths are in the `iroh-relay` 1.3.0 crate, `src/`. "Run" means it was started from this image and
observed.

**Ports.** One: UDP 7842, the QUIC address discovery port (`defaults.rs:7`, ALPN `/iroh-qad/0` at
`quic.rs:10`). Run: with `manual.toml` the container's namespace has no TCP listener and one UDP socket
on 7842. A third listener would be the metrics server, which defaults to on at `[::]:9090` with no
authentication (`main.rs:344-346`, `main.rs:778`). The config sets `enable_metrics = false`, so it is
off, as the run shows. This default was read from the source and not run.

**HTTPS server.** It does not run. `Server::spawn` builds the HTTP and HTTPS listeners only inside
`match config.relay` (`server.rs:712`), and the `None` arm returns no listener (`server.rs:833`). There
is no plain HTTP port and no captive portal check either. That is why the built-in LetsEncrypt mode
cannot run with the relay half off: its TLS-ALPN-01 challenge is answered on the HTTPS listener
(`server.rs:747-788`).

**The binary refuses this config without `--dev`.** `main.rs:762` builds the relay config only if
`enable_relay` is true, and drops the TLS config with it. In the normal path the QUIC server is given no
certificate of its own (`main.rs:694-697`), so `Server::spawn` finds none (`server.rs:846-848`). Run: the
upstream binary with `enable_relay = false`, `enable_quic_addr_discovery = true` and a `Manual` certificate
exits at once with `Failed to bind QAD listener` and `TLS not configured`. The `--dev` branch is the only
one that hands the certificate to the QUIC server (`main.rs:706`). It accepts `Manual` only
(`main.rs:709-711`), and its other effect, plain HTTP for the relay, has nothing to switch here. The image
passes `--dev` for this reason. It is an upstream bug, not a setting, and a later release may fix it.

**Logs.** To stderr. The filter is `EnvFilter::from_default_env()` (`main.rs:570`), so with `RUST_LOG`
unset the server prints errors only. The image sets `RUST_LOG=info`, which prints one line:
`QUIC server listening on bind_addr=0.0.0.0:7842` (`quic.rs:119`). Each address-discovery connection is
logged at `debug` (`quic.rs:212`) with the client's address, `qad-conn{remote_addr=172.17.0.1:43145}`.
Run with `RUST_LOG=iroh_relay=debug`.

**What a client sees if it uses the relay half anyway.** Run, with the probe from this repository
(`iroh` 1.3.0) and a rendezvous URL as its only relay:

- The address-discovery round trip works: 0.37 s from binding to the server's answer on loopback.
- iroh adds the URL as its home relay, so the ticket carries `{"Relay":"https://127.0.0.1:7842/"}`.
- The relay WebSocket dial gets `Connection refused` on the TCP port of the URL, because nothing
  listens. iroh logs `Failed to connect to relay server: unable to connect: Connection refused` and retries
  with an exponential backoff from 10 ms, capped at 16 s before jitter, and no limit on attempts
  (`iroh` `src/socket/transports/relay/actor.rs:326-355` and `:392-398`). It keeps trying until the
  endpoint closes. Run for 75 s: 13 dials, with delays of 13 ms, 37 ms, 79 ms, 102 ms, 277 ms, 551 ms,
  1.0 s, 2.3 s, 4.3 s, 8.2 s, 10.6 s, 27.7 s and 20.9 s. Jitter takes a delay past the cap, so
  an idle endpoint makes one failed TCP connect every 16 to 32 s.
- The HTTPS latency probes to `/ping` fail the same way, at 200, 300 and 400 ms into the report.
- Neither the ticket nor the connection waited for it: the dial took 1.6 ms on loopback.

With a firewall that drops TCP instead of refusing it, each relay dial would wait for the 10 s connect
timeout (`actor.rs:87`) in the relay actor. Nothing in a connection waits on that actor. This was read
from the source and not run.

## The handshake is X25519, not post-quantum

The upstream binary installs `ring` as its TLS provider (`main.rs:579` and `main.rs:617`), and `ring`
has no `X25519MLKEM768`. Address discovery therefore negotiates X25519. We do not patch it. The
handshake carries the peer's observed address and moves no transfer data, and ADR 0018 covers the
transfer connections.

## Limits

- One QUIC address discovery server cannot tell whether a NAT maps a source port differently per
  destination. That needs a second server to compare against, and iroh reports it only with two relays in
  the map (`net_report/report.rs`, `mapping_varies_by_dest`).
- `accept_conn_limit` and `accept_conn_burst` are not implemented upstream (ADR 0013), and the
  address-discovery server has no connection limit of its own.
