# 0026. The completion code counts only for a connection that asked for every root

**Status:** Accepted **Date:** 2026-10-27 **Approved by:** @sanamhub

## Context

ADR 0025 let a one-download sender stop as soon as any connection on the static content protocol
closed with `0x534C0001`. The code was treated as the receiver's word that the transfer arrived, and
the ADR argued that it granted nothing new because a connection could always end the same sender by
taking every byte.

That argument covers a peer that downloads the ticket's content. It does not cover the sender's
reachability. A sender's identity key is persistent, so its endpoint id is stable across sends, and
it is handed out in every ticket it has ever printed. It also answers mDNS on the local network, so
a host on the same link learns the id without a ticket. Any of them could open a content connection,
close it with the code, and end a send that had served nothing, which the sender reports as a
completed download. The receiver of the real ticket is then cut off mid-transfer, and the operator
sees success.

## Decision

1. The code is believed only from a connection that (a) this process allowed at least one request
   from, and (b) asked for every content root the one-download stop is waiting for. The policy
   records the roots each connection asked for and was allowed, and the completion watcher passes
   the connection it observed to the sender, which checks the record.
2. A code from any other connection is ignored: the sender keeps serving, and the byte-count
   fallback of ADR 0025 continues to decide. Asking for every root and sending the code is still
   enough, which is what a resumed receive ends its sender with.
3. SPEC section 3 states the rule for a sender and `docs/threat-model.md` names the peer it stops.
   The close code, the moment a receiver sends it, and the fallback are unchanged.
4. The policy remembers a bounded number of connections; past that the oldest record is dropped, so
   a peer that opens many connections can only make a sender serve longer, never shorter.

## Consequences

- A stranger who can reach the endpoint can no longer end a send or make it report success. The
  worst it can do is open connections the sender has to answer.
- The rule is about the connection that sends the code, not about the peer: a receiver that was cut
  off and reconnects mid-transfer asks for the remaining roots on the new connection, so its code
  names only those. The sender then falls back to counting bytes, which is what a client that never
  sends the code does.
- A resumed transfer still ends its sender, including one of several roots. A receive asks for the
  first chunk of every content root to prove its size, whether or not its store already holds the
  blob, so the connection that sends the code asked for every root even when the bytes came from
  the process that was killed. This was measured: a tree whose store was nearly complete and a
  broken connection into a fresh sender finished and ended that sender.
- The policy's connection records are the only new state. They hold hashes the transfer already
  allows and are dropped with the provider.
