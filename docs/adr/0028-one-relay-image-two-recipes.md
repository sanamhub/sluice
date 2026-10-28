# 0028. One relay image, run either as a meeting point or as a full relay

**Status:** Accepted **Date:** 2026-10-28 **Approved by:** @sanamhub **Supersedes:** ADR 0013
**Relates to:** ADR 0011, ADR 0019, ADR 0020

## Context

ADR 0013 split the servers into a `rendezvous` image, which answers address discovery and nothing
else, and a `relay` image, which also carries traffic. It recommended `rendezvous` alone for most
users and left one question open: how much hole punching depends on the relay's side channel.

The answer is all of it. An iroh endpoint coordinates a hole punch through its relay connection:
the two peers swap their candidate addresses over it and then send toward each other at the same
time. An iroh maintainer confirmed that iroh has no way to punch a hole without a relay connection
([n0-computer/iroh#4623](https://github.com/n0-computer/iroh/issues/4623)). Address discovery alone
tells each peer its own public address and nobody else's.

With only `rendezvous`, the sender puts its discovered address in the ticket and the receiver dials
it. That succeeds only when the sender's router accepts a packet from an address it never sent to:
a port mapping made with PCP, NAT-PMP or UPnP, or a full-cone NAT. Two ordinary home routers fail.
Port mapping already tells a peer its public address, so the case that `rendezvous` adds is a
full-cone NAT with no port mapping, which is rare. The "about 90%" direct-connection rate in
`docs/design.md` is iroh's figure with a relay connection, and it never applied to `rendezvous`.

`iroh-relay` 1.4.0 can limit what each client sends into the relay (`[limits.client.rx]`,
`bytes_per_second` and `max_burst_bytes`). Coordination messages are a few kilobytes per
connection, so a low limit leaves hole punching working and makes the relay too slow to be worth
using as a free file host.

## Decision

1. **One image, `relay`.** The `rendezvous` image and the guard's `--rendezvous` mode are retired.
   They stay in the tree until the release pipeline that builds and publishes them is next changed,
   and they are removed there. No document recommends them from now on.
2. **Two documented recipes for that image:**
   - **Meeting point.** Closed, with `[limits.client.rx] bytes_per_second = 65536` and
     `max_burst_bytes = 262144`. Peers coordinate through it and the file goes direct. A pair that
     cannot punch a hole still connects, at 64 KiB/s. This is the default recommendation, and it
     fits the smallest VPS.
   - **Full relay.** Closed, with a per-client limit the operator picks from the host's bandwidth.
     Every transfer completes, and the relayed ones cost the host their size twice, once in and
     once out.
3. **A relayed transfer says so.** When a transfer has no direct path 10 seconds after it starts,
   `sluice` prints one line naming the relay and its role, and the transfer goes on. `--json`
   carries the same event. Nothing fails because a path is relayed.

The two limits in the meeting-point recipe are chosen, not measured. The direct-connection
measurement in the roadmap checks that hole punching still succeeds through them, and they change
here, in a new ADR, if it does not.

Rejected:

- **Keep `rendezvous` as a cheaper tier.** It costs the same VPS as a meeting point and connects far
  fewer pairs. A second image to build, sign and explain for that case is not worth it.
- **Fail a transfer that has no direct path unless a flag allows the relay.** It makes the cost of
  a relay visible, but it breaks every transfer from behind a hard NAT, and the one-line notice
  makes the path just as visible.

## Consequences

- `README.md`, `docs/design.md`, `docs/roadmap.md`, `docs/threat-model.md` and the site describe one
  relay with two recipes, and the connectivity tables say which setups connect directly.
- A pair behind two hard NATs needs a full relay for a usable speed. With a meeting point it
  connects, prints the notice, and runs at 64 KiB/s.
- `docker/relay/README.md` shows the meeting-point limits as two lines to add to `relay.toml`. The
  guard already accepts `[limits]` in a closed config, so no code changes for the recipe.
- The release pipeline drops the `rendezvous` image and its smoke test the next time it changes.
