# 0011. The browser is a relayed peer, and relay mode is never chosen silently

**Status:** Accepted

**Supersedes the browser transport claim in plan v7 section 10.**

## Context

Plan versions up to v7 described the browser client as using "WebTransport where a path allows it,
WebRTC for hole punching, WebSocket where UDP is blocked". Checking that against the stack we actually
depend on, two of those three do not exist.

A browser tab cannot open a UDP socket. iroh's WASM build therefore connects over WebSocket to a relay,
and every browser connection is relayed for its whole life. iroh carries no WebRTC transport, and
WebTransport becoming Baseline in Safari 26.4 does not help, because the relay protocol is WebSocket.

Separately, iroh's `RelayMode::Default` points at n0's hardcoded free public relays. Locked decision 1
forbids depending on a third-party service, so we cannot inherit that default without contradicting the
premise of the project. iroh relays also gained authentication, and since June 2026 managed relays are
authenticated by default, because a relay URL that leaks is a free bandwidth faucet.

## Decision

1. Relay mode is `RelayMode::Disabled` until an operator names a relay, and `RelayMode::Custom` after
   that. `RelayMode::Default` is reachable only through an explicit flag that prints a warning.
2. The browser is documented, in the README and in the product, as always relayed. No UI element and no
   benchmark implies a direct browser path.
3. Browser throughput numbers are published with the relay's CPU, NIC, and egress, because they measure
   a VPS rather than a link.
4. The relay container ships with authorization on. `open` mode requires an explicit flag and is rate
   limited even then. Two relays in two regions is the documented production shape, since iroh clients
   fail over between them without help.
5. `sluice doctor` and the web UI's first run both report the relay in use, or report that there is none
   and say what that rules out: symmetric-NAT pairs, and the browser client entirely.

## Consequences

- An operator who installs only the CLI may never need to run anything. An operator who wants the web UI
  must run a relay, and learns that from the quickstart rather than from a support thread.
- Relay egress becomes the real operating cost of offering a browser client. Per-key bandwidth and
  connection quotas exist in every authorization mode, and the quota shows up in `doctor`.
- Cutting the browser never degrades the native product, because the relay is on the critical path for
  browser sessions only.
- This is a sandbox limit, not a defect we can engineer away. Every browser-based transfer tool shares
  it, and saying so plainly is worth more than a benchmark.
