# 0030. Each send keeps its copies in a store of its own, and the home keeps none of them

**Status:** Accepted **Date:** 2026-10-28 **Approved by:** @sanamhub

## Context

A send imports a regular file where it lives and copies nothing of it, so the home store held only
what a send had to copy: the bytes of standard input, the packs of small files, the manifest and the
hash sequence, and the segments of a live stream. That is still a lot, and it was written into
`home/blobs`, which every send opened and nothing emptied.

Two consequences followed. A machine that sends many folders kept growing `home/blobs`, because the
copies were added under tags and the store was opened without a collector. And a live send's
segments stayed after it exited: the router closed the store when the provider stopped, and the
send's tags were released after that, so the collector never ran on the segments it should have
freed. 0029 records the second one as a correctness problem of the collector's placement; it is also
a problem of where the bytes live.

## Decision

1. Every send opens its store in a private folder of its own under the home's temporary area, made
   the way the home's folders are made: mode 0700 on Unix and the home's private access list on
   Windows. This covers a static send, standard input, the packs and manifests, and a live send.
   Regular files are still imported by reference where they are, and are not copied.
2. The folder is removed after the provider has stopped and the store is closed, on every way out
   the command can await: success, failure, expiry and Ctrl-C. A folder a killed send left is
   removed by the sweep of temporary folders the next command runs, and folder and store go
   together, in that order, so a folder is never removed under a database that is still open.
3. A live send keeps its collector inside that store, so a segment it releases can be freed while
   the send runs, and the folder going away bounds what it leaves after it.
4. `home/blobs` is no longer opened by any send, and no longer created. A leftover one is removed the
   first time a send runs: it holds nothing a user can name.
5. Two sends in one home never share a store, and neither removes the other's: each folder has a
   name of its own and a lock that says a running command holds it.

## Corrections to 0029

0029 is not edited; this is what it gets wrong.

1. 0029 says a static send's content stays in the home store until a store is next opened with the
   collector on, which is what a live send opening the same store would do. There is no store the
   two share any more: a static send's copies are in its own store and are gone with it, and no
   later send collects them, because it never sees them.
2. 0029 says a live stream is bounded by one sweep on both sides. On the sending side it is not: the
   sender holds every sealed segment until it stops, because a reader may ask for any of them until
   then, so the collector inside the live store frees nothing while the stream runs. What bounds a
   live send now is its folder, which is removed when the send ends, and the collector is what
   bounds it when a send is interrupted after the collector has been handed a released segment.
3. 0027 claimed that "a machine that sends many folders no longer accumulates them", and 0029
   reversed that by keeping a static send's content in the home store. This restores it: a home
   that sends accumulates nothing.

## Consequences

- A home's disk use after a send is what it was before the send, whatever the send copied.
- A send that is killed, or a publish that is cancelled, may leave its folder behind. The sweep of
  temporary folders removes it after a day, and no other command reads it.
- A store is opened and closed per send, which costs one temporary folder and one database. Cold
  start of a command that does not send is unchanged.
- A send that is served for hours keeps a copy of everything it sealed, as it did before; that copy
  is in its own folder and does not survive the send.
- The home's layout loses `blobs`. A home made by an older build keeps it until a send removes it,
  and an older build reading a home without it makes it again and finds it empty.
