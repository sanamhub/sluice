# 0020. The guard closes three more open defaults, pins the file it checked, and guards rendezvous

**Status:** Accepted **Date:** 2026-10-23 **Approved by:** @sanamhub **Relates to:** ADR 0011, ADR
0013, ADR 0019 (this ADR extends it)

## Context

The guard of ADR 0019 accepts no config that iroh-relay 1.4.0 runs open. Four more places end up
less closed than a config or a deployment reads, though. Line numbers are in
`iroh-relay-1.4.0/src/main.rs`.

- **HTTPS can be turned off in the file.** `[tls] dangerous_http_only = true` drops HTTPS (lines
  703-721), which is what `--dev` does, and the guard refuses `--dev`. A shared token then crosses
  the network in clear text.
- **Metrics are on unless the file says otherwise.** `enable_metrics` defaults to true (lines 145
  and 384-386), and the metrics server listens on port 9090 with no authentication. This is the same
  pattern as a missing `access`.
- **The HTTP access URL is logged.** A failed access call logs the request error at `warn` (line
  331), and reqwest's error text holds the full URL. A password or a query in that URL is then in
  the container log.
- **iroh-relay opens the file again.** It reads the path itself after the guard has checked it
  (lines 549-567). A swap by rename between the two, such as a Kubernetes ConfigMap update, starts a
  file nobody checked.

The `rendezvous` image has its own gap. It runs upstream iroh-relay with `enable_relay = false`, and
`enable_relay` defaults to true (lines 107 and 376-378). If an operator deletes that line, the
rendezvous box becomes an open relay with no byte limit. ADR 0019 left this image upstream only.

## Decision

The guard gains these rules, on top of those in ADR 0019:

1. **`dangerous_http_only = true` is refused.** A config with no `[tls]` at all stays allowed, for a
   relay behind a reverse proxy that ends TLS.
2. **`enable_metrics` must be written out.** True and false both pass. Leaving it out is refused.
3. **The HTTP access URL may hold no user, password or query.** A secret goes in `bearer_token`,
   which is sent as a header and not logged.
4. **The relay reads the bytes the guard checked.** The guard opens the file once and checks what it
   read. It then passes that same open file to iroh-relay as standard input, with
   `--config-path=/dev/stdin`.
5. **The rendezvous image starts through the same guard, with `--rendezvous`.** That mode requires
   `enable_relay = false` written out, and refuses `--open`, `access` and `[limits]`, since a server
   that relays nothing has no use for them. It parses with the same strict schema.

Two alternatives were rejected:
- **Copying the checked bytes to a private file in `/tmp`** closes an edit in place too. It needs a
  writable `/tmp`, so it breaks a container run with `--read-only`.
- **A ceiling on the open byte rate.** The operator already chose the number when they passed
  `--open`, and a ceiling would be a second number to argue about.

## Consequences

- **An edit in place,** in the instant between the guard's read and the relay's, can still get in,
  because standard input pins the file and not its contents. A rename swap cannot.
- iroh-relay then has no standard input of its own. It reads none today.
- **Both images now hold code of ours.** The consequence in ADR 0019 that says the rendezvous image
  stays upstream only no longer holds.
- **A config that relied on a default now needs one more line:** `enable_metrics`, and for
  rendezvous `enable_relay = false`. The guard's message names the key.
- Revisit together with ADR 0019, if iroh-relay refuses unknown keys and its defaults turn closed.
