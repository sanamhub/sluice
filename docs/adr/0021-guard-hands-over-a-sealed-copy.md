# 0021. The guard hands iroh-relay a sealed copy, and a relay without TLS needs a flag

**Status:** Accepted **Date:** 2026-10-23 **Approved by:** @sanamhub **Relates to:** ADR 0019, ADR
0020 (this ADR extends it and replaces its decision 4)

## Context

ADR 0020 decision 4 hands iroh-relay the open config file as standard input, with
`--config-path=/dev/stdin`. The approach has two gaps. Line numbers are in
`iroh-relay-1.4.0/src/main.rs`.

- **A path that does not resolve runs open.** iroh-relay tests the path with `exists()`, which is
  false on any error, and then runs `Config::default()` (lines 548-552): relay on, access for
  everyone, no limit. `/dev/stdin` is a link the container runtime makes, and some runtimes do not.
  The guard now passes `/proc/self/fd/0` and checks, before exec, that it names the checked file by
  device and inode.
- **An edit in place gets in, and an empty file is open.** iroh-relay opens the path again and reads
  it from the start (lines 559-567). That reaches the same inode, with whatever it holds at that
  moment. ADR 0020 accepted that an edit in place could get in. It did not see the worst case: a
  write that truncates the file first, as `cat >` or a generator script does, leaves iroh-relay an
  empty file, and `toml::from_str("")` is `Config::default()`. A host file bind-mounted into the
  container is edited this way, because a rename would break the mount.

ADR 0020 also kept a config with no `[tls]` allowed, for a relay behind a reverse proxy that ends
TLS. The guard cannot tell that relay from one where TLS was forgotten. In both, a shared token or a
bearer token crosses the network in clear text.

## Decision

1. **The guard hands over a sealed copy of the bytes it checked.**
   - It copies them into a `memfd`, then seals it against writes, growth, shrinking and further
     seals.
   - It puts that copy on descriptor 0 itself, and checks that `/proc/self/fd/0` has the copy's
     device and inode.
   - Only then does it exec iroh-relay with `--config-path=/proc/self/fd/0`.
   - If any step fails, the guard exits 69 and starts nothing.
2. **A relay with no `[tls]` needs `--behind-proxy`.** In closed and open mode, a config without
   `[tls]` is refused unless the guard is given `--behind-proxy`. `--behind-proxy` with a `[tls]`
   section is refused, and so is `--behind-proxy` in rendezvous mode.

Two alternatives were rejected:
- **Keep the file and state the window.** An empty file is the open default the guard exists to
  stop, so a window of a few milliseconds is still an open relay.
- **Copy to a private file in `/tmp`.** ADR 0020 rejected it for `--read-only` runs, and a `memfd`
  needs no writable folder.

## Consequences

- iroh-relay reads exactly the bytes the guard checked. ADR 0020's consequence that an edit in place
  can still get in no longer holds.
- The guard needs `memfd_create` and its seals, in Linux since 3.17. A runtime whose seccomp profile
  blocks them gets exit 69, not an open relay. A sealed `memfd` reopened through `/proc/self/fd` was
  checked in WSL on 2026-10-23: it is a regular file, reads back the bytes, and refuses a write and
  a truncate. Inside Docker's default seccomp profile it is not yet checked.
- An operator behind a proxy adds one flag. The shipped configs all have `[tls]`, so the images'
  default commands do not change.
- Revisit together with ADR 0019 and ADR 0020, if iroh-relay reads its config from a descriptor it
  is given, or refuses an empty config.
