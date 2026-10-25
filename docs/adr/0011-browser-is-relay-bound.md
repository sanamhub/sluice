# 0011. The browser is always relayed, and relay mode is never chosen silently

**Status:** Accepted **Date:** 2026-10-19 **Approved by:** @sanamhub

## Context

A browser tab cannot open a UDP socket. iroh's WASM build reaches peers over a WebSocket to a relay,
and iroh has no WebRTC transport, so every browser session is relayed for its whole life.

WebRTC data channels can hole-punch from a tab. Adding them would mean ICE, a TURN service and a
second connectivity stack beside iroh's. On the same laptop, a WebRTC data channel implementation
measured about 20 MB/s against iroh's 135 to 177 MB/s
([qrdrop#6](https://github.com/stan-ely/qrdrop/issues/6)).

iroh's `RelayMode::Default` uses n0's public relays. Sluice depends on no service it does not run,
so that default cannot be inherited. A relay URL that leaks is free bandwidth for anyone who finds
it.

## Decision

1. Relay mode is `Disabled` until an operator names a relay, then `Custom`. `Default` needs a flag
   and prints a warning.
2. The README, the web UI and `sluice doctor` all say that the browser is always relayed. Nothing
   implies a direct browser path.
3. Browser throughput is published with the relay's CPU, NIC and egress, because it measures the
   relay.
4. The relay image runs with authorization on. `open` mode needs a flag and has a per-client byte
   rate limit.
5. `sluice doctor` and the web UI's first screen name the relay in use, or say there is none and
   what that rules out.

Rejected: a WebRTC transport, for the reasons above. Revisit if iroh ships a browser transport that
hole-punches.

## Consequences

- Someone who uses only the CLI may never run a relay. Someone who wants the web UI must, and learns
  that from the quickstart.
- Relay egress is the real cost of offering a browser client, so per-key quotas exist in every
  authorization mode and show up in `doctor`.
- Dropping the browser would not affect the native product.
