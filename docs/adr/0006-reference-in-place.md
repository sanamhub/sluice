# 0006. Reference user files in place; the store holds outboards, not data

**Status:** Accepted

## Context

A content-addressed store implies importing data into the store. For this product that is wrong: sending
a 100 GB file would double disk usage and halve throughput. No version of the plan before v7 stated an
import mode at all, which is the kind of gap that surfaces in week three of implementation.

## Decision

Default to reference in place. Hashing reads the file where it lives, and the store keeps only the
outboard tree and metadata, about 0.4% of file size at 16 KiB chunk groups or about 0.1% at 64 KiB.
`--copy` imports for volatile or removable sources. Receiving writes directly to the destination as a
sparse `.part` plus an outboard sidecar, then `fsync` and rename.

Because the source can change underneath us, record size, mtime, and inode at import and re-check before
each range read. A mismatch fails the transfer loudly rather than serving mixed content.

The store is single-writer. Take an exclusive lock; if a live process holds it, delegate over the control
socket rather than opening the store twice.

## Consequences

- Disk cost of sending is proportional to the tree, not to the content.
- Users get a clear failure instead of a silently corrupt transfer when a file changes mid-send.
- The CLI has two execution modes, in-process and delegating, decided by the lock.
