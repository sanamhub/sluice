# 0031. A shared ticket counts the receivers it serves, and the sender says who connected

**Status:** Accepted **Date:** 2026-10-29 **Approved by:** @sanamhub

## Context

A ticket is a read capability for one piece of content, and sharing it with several people is what it
is for. A send stops after one receiver has taken the whole transfer unless it is given `--keep`, and
that wait has no upper bound other than the expiry: a ticket left in a chat, or forwarded on, serves
anyone who finds it for as long as the sender is running. Nothing on the sender's side said who had
taken the content, so a user could not tell a transfer that was expected from one that was not.

The receive side already knows exactly who took the whole transfer. A receive that took every entry
closes its content connection with the completion code, and the sender already believes that code
only from a connection that asked for every content root, so a partial receive does not end a send.
The ticket's own text names no receiver, so whatever bounds the number of receivers has to be counted
by the sender while it serves.

## Decision

1. `--keep N` serves at most N distinct receivers that have each taken the whole transfer, and stops
   the send when the last of them finishes. Plain `--keep` keeps its meaning: until Ctrl+C or the
   expiry, with no count. Without `--keep`, a send still stops after one complete transfer.
2. A receiver is counted by its endpoint id, which is what its ticket names anyway: two connections
   from one endpoint are one receiver, so a reconnect does not use up the limit.
3. Only a believed completion counts: a connection that asked for nothing counts as nothing, and a
   receive that took part of a ticket never sends the code, so it counts as nothing either. A
   receiver that is already transferring when the limit is reached is waited for, at the longest for
   the grace period; one that connects afterwards is not served, because the limit is what the send
   was asked to spend.
4. The sender prints one line when a receiver's content connection opens, and one when it closes. The
   line names the receiver by the first ten characters of its endpoint id in the base32 a ticket is
   written in, says whether its path is direct or relayed, and, for the closing line, whether it took
   the whole transfer or part of it. With `--json` the same two events carry the full id. Both go
   through the same escaping boundary as every other line a name reaches.
5. The ticket's text does not change: nothing goes on the wire for this, and an older receiver and a
   newer sender still work together. The completion code already existed and its meaning is
   unchanged.
6. Binding a ticket to one receiver was considered and is not done. A ticket shared with several
   people is the point of the format, and a binding would break exactly that; a count, and a line
   that names who took it, bounds the exposure without taking the sharing away.

## Consequences

- A sender that expects several receivers can now say how many it will serve, and sees each of them
  connect and finish. A receiver nobody expected is visible while it is connected, and Ctrl-C stops
  the send.
- `--keep N` says nothing about the wire, so a receiver cannot tell whether a send will still be
  there: one that arrives after the limit was reached fails to connect as it would if the sender had
  been stopped.
- The count is a bound on receivers that finished, not on connections: a peer that connects and asks
  for nothing costs a line and no number.
- The provider now holds one channel of connection reports, and a connection is remembered for a
  moment after it closes so its report is credited to it and not to the next connection. A peer that
  opens many connections makes the sender print a line for each, which is the point.
