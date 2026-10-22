# 0019. The relay image starts through a guard that refuses an open config

**Status:** Accepted
**Date:** 2026-10-22
**Approved by:** @sanamhub
**Relates to:** ADR 0011, ADR 0013

## Context

ADR 0011 says the relay runs closed, and that open mode needs a flag and a per-client byte limit.
The `relay` image runs the upstream `iroh-relay` 1.4.0 binary, and its config handling makes a
mistake open the relay without a word:

- **A missing `access` setting means everyone.** The field is `#[serde(default)]` and the default
  is `AccessConfig::Everyone`.
- **Unknown keys are ignored.** No part of the config denies unknown fields, so `[acess]` is read as
  no access setting at all.
- **A config path that does not exist means the default config,** which is open with no limits.
  So does no `--config-path` at all.

Each of these gives a relay anyone can use, with no rate limit, and the server logs nothing that
says so. A leaked relay URL is then free bandwidth on the operator's bill.

## Decision

The image's entry point is a small program of ours, `sluice-relay-guard`, that checks the config
and then replaces itself with `iroh-relay`:

1. It requires `--config-path`, and the file must exist.
2. It parses the file with a schema of its own that mirrors `iroh-relay` 1.4.0's config, and refuses
   any key that schema does not know, at every level.
3. A closed config is an allowlist, an HTTP check, or shared tokens, and starts as it is.
4. Everyone, a denylist, or no `access` setting is open. It starts only with `--open`, and only
   when `[limits.client.rx]` sets `bytes_per_second`. `--open` with a closed config is refused, so
   the flag always means what it says.
5. It refuses the `iroh-relay` arguments that bypass the config, such as `--dev`.
6. It names the `iroh-relay` version it was written for, and a check fails when the image builds a
   different one, so a new release cannot add a key the guard does not know.

## Consequences

- **The image now holds code of ours,** with tests and a fuzz target, built from this repository
  with `--locked`. The `rendezvous` image stays upstream only: it relays nothing.
- **Every `iroh-relay` upgrade needs the guard's schema reviewed** against the new release's config,
  which check 6 enforces.
- **An operator's typo now stops the container** with a message that names the key, instead of
  opening the relay.
- Revisit if `iroh-relay` itself refuses unknown keys and has no open default: the guard can then
  shrink to the `--open` rule, or go.
