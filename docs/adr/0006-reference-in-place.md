# 0006. Reference files in place; the store holds trees, not data

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub

## Context

A content-addressed store normally imports data before serving it. For a transfer tool that doubles
disk use and halves throughput: sending a 100 GB file would first copy 100 GB.

Serving a file in place means it can change underneath a transfer, and a store reachable from both a
CLI process and a daemon can be opened twice.

## Decision

- Sending hashes the file where it lives. The store keeps the outboard tree and metadata, about 0.4% of
  the file size.
- `--copy` imports into the store, for removable or volatile sources.
- Receiving writes directly to the destination as a sparse `.part` file with an outboard sidecar, then
  `fsync` and rename.
- Size, mtime and inode (file id on Windows) are recorded at import and checked before every range
  read. A mismatch fails the transfer with a `Verify` error that names the file.
- One process writes to a store. It holds an exclusive lock with its pid. If a live process holds the
  lock, the CLI talks to it over the control socket instead of opening the store.

## Consequences

- The disk cost of sending is the tree, not the content.
- A file edited mid-send fails loudly instead of arriving half old and half new.
- The CLI runs in one of two modes, in process or delegating, depending on the lock.
