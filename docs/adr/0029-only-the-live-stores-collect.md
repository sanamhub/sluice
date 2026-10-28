# 0029. Only the live stores run their collector, and every store is closed on the way out

**Status:** Accepted **Date:** 2026-10-28 **Approved by:** @sanamhub

## Context

0027 opened every store with a collector and had every place that needs a blob hold a temporary
tag. The pairing is right for a store whose blobs are throwaway, and wrong for the others.

A resume store is the one store whose blobs must outlive the command. A run that keeps its store
after a network or storage failure drops every tag it holds before it exits, because a tag cannot
outlive the process, so a sweep after that would free exactly the bytes a second run continues
from. Between two runs there is no process and no tag at all: the store's bytes are held by the
folder and its marker, which is the record a later run trusts. A collector in that store can only
lose work.

A range, a write to standard output and the manifest store each write into a private folder that is
removed when the command ends. A collector there buys nothing and adds two ways to fail: a fetch
whose tag is dropped a moment too soon, and a sweep that runs between a pack arriving and being
copied out.

The stores also turned out not to be closed on every way out. A command that returned early, or
that was cancelled, dropped its store instead of closing it: the store's actor kept the database
open, and a resume in the same process then waited on a folder that nothing was going to release.

## Decision

1. Two stores run the collector, because their blobs are throwaway and are written once: the live
   send store and the live receive store.
2. Every other store opens without it and keeps every blob until the store is done or discarded:
   the static send store, the static receive store and the resume store it becomes, the range
   store, the standard output store, and the manifest store. This is the behaviour before 0027.
3. The one function that opens a store takes the choice as an argument, `Collector::On` or
   `Collector::Off`, so a new store cannot be opened without saying whether its blobs may be freed.
4. The interval stays 10 seconds, for the reason 0027 gives: a few segments at the rates a live
   stream is sealed at, and far longer than a reader needs to have a request answered. A unit test
   sweeps sooner so it does not wait for the first sweep.
5. On the two live stores each blob is held by a temporary tag until it is no longer needed:

   | Where | Blob | Held by | Until |
   |---|---|---|---|
   | live send | each sealed segment | a temporary tag in the send | the send stops serving it, which is after the close is signed and the provider's wait ends |
   | live receive | each segment | a temporary tag made before its fetch | the segment is written to the output file |

   The live sender holds every segment until it stops rather than until the segment is written,
   because a reader may ask for any sealed segment while the send is serving.

   On the stores that do not collect, the tags whose only job was to protect a blob from the
   collector are gone. The tags at the places that add bytes stay: `temp_tag()` is the call that
   does not create a permanent named tag, and a named tag would outlive every command.
6. Every store is closed on every way out of the command that opened it. A `Store` closes itself
   where the command can await it, and one a cancelled command could not close is handed to the
   next command in the process, which closes it before it opens its own. The store a send serves
   from is closed by the router shutdown, which closes the blob protocol.

## Consequences

- A receive that was interrupted, by a failure or by the user, can be resumed in the same process:
  the database of its store is free by the time the second run asks for it.
- A store a cancelled command left behind keeps one actor alive until the process opens another
  store or exits. That costs a thread and an open database, not disk.
- A live stream is still bounded by one sweep of what it sealed, on both sides; nothing else is
  collected, so a static send's content stays in the home store until the store is next opened with
  the collector, which never happens.
- A resume store that is never resumed keeps its bytes until the sweep of temporary folders or the
  user removes it, as it did before 0027.
