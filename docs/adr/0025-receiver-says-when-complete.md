# 0025. A receiver says when the transfer is complete

**Status:** Accepted **Date:** 2026-10-27 **Approved by:** @sanamhub

## Context

A one-download sender stopped in one of two ways: every content root had been served across all
connections in this process, or the connection that asked for the last root closed. Both are
claims about this machine, not about the receiver's.

The first is a byte count. A blob split into chunk-group requests was counted as delivered when
the *sum* of the payloads it sent reached the blob's size, so two requests for the first half
counted as the whole file. It also counted bytes acknowledged by QUIC, which lets a sender end
while the last of them are still in flight to the application.

The second never happened at all for a resumed receive. A receive cut off at 300 MB and rerun
against a new `send` asks only for the bytes it is missing, so the new sender process serves 700 MB
of a 1 GB blob and no single request ever covers the whole root. The sender sat until its 24-hour
ticket expiry, and a receive that arrived correctly, verified and installed, left a process running
that had nothing left to do.

Counting bytes can also not tell a range request from a whole one, or a client that keeps asking
for the same range from one that is making progress.

## Decision

1. The receiver says so. Once it has verified and installed every file, a receive closes its static
   content connection with the QUIC application close code `0x534C0001`. The code, the moment it is
   sent, and the fact that a sender may stop on it are in SPEC section 3.
2. A one-download sender stops serving when it sees that code, through its normal shutdown, and
   does not wait for a byte count or a closed connection first.
3. A receive that failed, a receive of a byte range, and a receive that writes part of a transfer
   to standard output never send the code. A sender that never sees it still stops on the fallback.
4. The fallback counts the union of the ranges served for a root in this process, not a sum of
   payloads, and counts a request that aborted by the bytes it did send. Two requests for the same
   half are one half.
5. The code is not a credential. Any holder of a ticket can already end a one-download sender by
   taking every byte, so sending the code without the bytes grants nothing new: it ends a sender
   whose ticket the sender already gave out. `docs/threat-model.md` says so.

## Consequences

- A resumed receive ends its sender promptly, however many processes served the file.
- The fallback remains for clients that are not Sluice, and its only failure is serving longer.
- The sender trusts one unauthenticated bit about the receiver's disk. It changes nothing the
  sender promised: the ticket still names the content, and a receiver that lost the file asks for a
  new ticket. This is the same trust a serving loop that stops after one download already places in
  a stranger.
- The end of a transfer is observable by the sender a moment after the last file is installed,
  which is also what a progress line reports.
