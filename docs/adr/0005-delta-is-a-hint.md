# 0005. Delta is a hint layer and never decides correctness

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub

## Context

Resending a large file that changed slightly should not move the whole file. Finding the unchanged
parts needs content-defined chunk boundaries, which move with insertions. Verified streaming needs
fixed boundaries, which is why the BLAKE3 tree uses 16 KiB groups. One chunker cannot do both.

The usual rsync-style flow has the receiver send signatures of its own files to the sender. That tells
the sender what the receiver has on disk.

## Decision

- The BLAKE3 tree stays the only structure for integrity and addressing.
- A FastCDC index is added purely as a hint.
- The provider sends chunk signatures for content the requester is already allowed to read. The
  requester matches them against its own files and fetches only what it cannot rebuild.
- The assembled result is verified against the real root. If that fails, the requester discards it and
  fetches normally.

## Consequences

- A wrong hint costs wasted work, never a corrupt file.
- The requester reveals nothing about its local files.
- Chunk signatures add traffic before the transfer starts, so delta is opt-in per transfer, through the
  `DELTA` capability.
