# 0031. A shared ticket counts the receivers it serves, and the sender says who connected

**Status:** Accepted **Date:** 2026-10-29 **Approved by:** @sanamhub

## Context

A ticket is a read capability for one piece of content, and sharing it with several people is what it
is for. A send stops after one receiver has taken the whole transfer unless it is given `--keep`, and
that wait has no upper bound other than the expiry: a ticket left in a chat, or forwarded on, serves
anyone who finds it for as long as the sender is running. Nothing on the sender's side said who had
taken the content, so a user could not tell a transfer that was expected from one that was not.

The receive side knows when a receive has taken every entry: it closes its content connection with
the completion code. That says what one connection did, not who was served, and it is not a bound:
a receive to standard output, a range receive, or any client that takes every byte and closes the
normal way never sends the code, so a count of codes lets those clients take the content without
ever reaching a limit. The ticket's own text names no receiver, and it names no receiver's endpoint
either: it names the sender. Whatever bounds the number of receivers has to be counted by the
sender, at the point where it decides whether a request is served.

## Decision

1. `--keep N` offers N places. A place belongs to an endpoint id and is taken by the first request
   the policy lets in from it, for any blob of the transfer, the manifest included. How that
   connection ends — whole, part, a failure, a quiet close — does not give the place back. Once all
   N places are taken, the policy refuses every request from an endpoint id that holds none, in the
   code that decides whether a request is served and before any byte is written. The send stops when
   N places are taken and every connection of the endpoints holding them has closed, or at Ctrl+C,
   or at the ticket's expiry. A holder that stays connected doing nothing is waited for no longer
   than the grace the one-time send already uses (30 seconds), and a request or a byte from a
   holder pushes that wait back, so a transfer that is still moving is not cut off. Plain `--keep`
   keeps its meaning: until Ctrl+C or the expiry, with no count. Without `--keep`, a send still
   stops after one complete transfer, and a one-time send is unchanged: it ends on a completion
   code from a connection that asked for every root.
2. A place is counted by endpoint id, and a ticket does not name an endpoint id: it names the
   sender. Each `sluice recv` binds a fresh endpoint key, deliberately, so that two runs cannot be
   linked to one person; the cost is that a person who runs `recv` again after a failure takes a
   second place. A fingerprint names one run, not a person. A receiver that reconnects under the
   same endpoint id is one receiver: its second connection is served without another place.
3. A refusal is a line of its own, made at most once per endpoint id and at most three times in
   all, so a host that keeps reconnecting cannot flood the terminal. `sluice inspect` reads the
   manifest, which is a request like any other, so against a send with N places it takes one.
4. The sender prints one line when a receiver takes a place, and one when a connection of a holder
   closes, whether it took the whole transfer or part of it. The line names the receiver by the
   first ten characters of its endpoint id in the base32 a ticket is written in, says whether its
   path is direct or relayed, and, for the closing line, whether it took the whole transfer or part
   of it. With `--json` the same events carry the full id. Both go through the same escaping
   boundary as every other line a name reaches.
5. The ticket's text does not change: nothing goes on the wire for this, and an older receiver and a
   newer sender still work together. The completion code already existed and its meaning is
   unchanged.
6. Binding a ticket to one receiver was considered and is not done. A ticket shared with several
   people is the point of the format, and a binding would break exactly that; a count, and a line
   that names who took it, bounds the exposure without taking the sharing away.

## Consequences

- A sender that expects several receivers can now say how many endpoint ids it will serve, and sees
  each of them take a place and finish. A receiver nobody expected is visible while it is connected,
  and Ctrl-C stops the send.
- The count is a bound on endpoint ids served, not on connections or on completed transfers: a
  client that asks for the manifest and goes away has spent a place. A connection that asks for
  nothing spends none and prints nothing.
- `--keep N` says nothing about the wire, so a receiver cannot tell whether a send will still be
  there: one that arrives after the limit was reached has its request refused with the permission
  reason, and its connection reaches nothing it can read.
- A person who runs `recv` again after a failure is a second endpoint id to the sender, and takes a
  second place. The line that names them names one run, not one person; nothing in the format links
  two runs, which is the property the fresh key is there for.
- The provider now holds one channel of connection reports, and a connection is remembered for a
  moment after it closes so its report is credited to it and not to the next connection.
