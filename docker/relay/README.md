# relay

The `relay` image runs `iroh-relay` 1.4.0 with both halves on: address discovery over QUIC, and the
relay over HTTPS that carries the traffic of two peers that cannot reach each other directly.
**This is the image with the bandwidth bill.** Every relayed byte of a transfer passes through the
host that runs it, in and out again (ADR 0013). For address discovery alone, run
[`rendezvous`](../rendezvous/README.md), which carries no transfer bytes.

It is the upstream binary, built the way `rendezvous` builds it: from crates.io with `--locked`,
at the version `Cargo.lock` resolves for `iroh`, on the same two base images pinned by the same
digests. It runs as uid 65532 on `gcr.io/distroless/cc-debian13:nonroot`, which has no shell, and
the metrics server is off.

The relay runs closed (`docs/design.md`, **Relays run closed**). Only the endpoint ids on its
allowlist may relay through it, and the image ships with that list empty, so it lets nobody in until
the operator adds an id.

## Ports

| Port | What |
|---|---|
| TCP 443 | the relay, over HTTPS and WebSocket |
| TCP 80 | plain HTTP, serving only the captive portal check `/generate_204` that iroh clients make |
| UDP 7842 | address discovery over QUIC |

Checked by running the image: those three listeners and no other, and `/generate_204` answers 204.

## Build and run

```
docker build -t sluice-relay docker/relay
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
should use the relay. The two below are examples, the public halves of keys made for the check and
thrown away. Copy [relay.toml](relay.toml) next to the certificates, add the ids, mount it
over the one in the image, and restart:

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
- with `open.toml`, the third key connected and relayed a datagram.

## Open mode

[open.toml](open.toml) lets anyone relay, and each client may send at most 2 MiB/s into the relay,
with bursts of up to 8 MiB. It is used only when the operator names it on the command line:

```
docker run -d --name relay -p 443:443 -p 80:80 -p 7842:7842/udp \
  -v "$PWD/certs:/etc/relay/certs:ro" sluice-relay --config-path /etc/relay/open.toml
```

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
