# 0027. Every blob store runs its collector, and a tag holds each blob while it is used

**Status:** Accepted **Date:** 2026-10-28 **Approved by:** @sanamhub

## Context

`iroh-blobs` keeps a blob on disk only while a name holds it: a tag, a temporary tag, or a tag a
batch holds. A store opened with `FsStore::load` starts no collector, so nothing is ever deleted and
the tags mean nothing. That was how every store was opened. Two consequences followed.

A live receive fetched each segment into its store and wrote it into the output file, and the
segment's bytes stayed in the store for the rest of the process: a two-hour stream used two hours of
disk for a file that was one hour long. A live send sealed each segment with `add_slice(..).await`,
which creates a named tag, and named tags are permanent: every send added segments to the home store
for ever. Static sends and receives did the same for their manifests and packs, and a static send's
content was the only thing a temporary tag held.

The stores are ours and short-lived or per-send, but the home store outlives every command, so a
collector that deletes what no tag holds has to be paired with a tag for everything still in use.

## Decision

1. Every store this tree opens is opened with the collector on: the per-transfer store beside a
   receive, the home store a send exports into, the stores a live send and a live receive keep their
   segments in, the store a range or a write to standard output fetches through, and the store a
   receive reads the manifest into. One function opens them, so a store cannot be added without one.
2. The collector's interval is 10 seconds. It is the longest a segment can stay on disk after the
   tag that held it is dropped, which is what bounds a live stream, and it is several times the time
   a reader needs to have a queued request answered. A shorter interval scans more often for little
   space; a longer one lets a stream keep more of what it has already written.
3. Every place a blob is still needed holds a temporary tag for it until the need ends:

   | Where | Blob | Held by | Until |
   |---|---|---|---|
   | static send | each content blob, each pack, the manifest, the hash sequence | a temporary tag in `Outgoing` | the send stops |
   | live send | each sealed segment | a temporary tag in the send | the send stops serving it, which is after the close is signed and the provider stops |
   | static receive | each content blob and pack | a temporary tag in the receive | the tree has been written |
   | live receive | each segment | a temporary tag made before its fetch | the segment is written to the output |
   | range, standard output | the blob the range reads | a temporary tag made before its fetch | the bytes are copied out |
   | manifest store | the hash sequence and the manifest | a temporary tag made before each fetch | both are decoded |

   The live sender holds every segment until it stops rather than until the segment is written,
   because a reader may ask for any sealed segment while the send is serving, and a reader that is
   slower than the sender is not a reason to lose one.
4. A kept store after a failed receive keeps its blobs, and its collector stops with the process, so
   nothing is collected between two runs. The resume takes its own tags from the same roots before
   it reads anything.
5. A named tag is no longer used for any blob: a named tag is permanent, and permanent names are what
   made a live send grow the home store for ever.

## Consequences

- A live stream keeps about one interval of segments beside the file it is writing, on both sides,
  however long the stream runs. The same bound makes a killed live receive leave at most that much
  in a temporary folder the sweep removes.
- A static send's manifest, packs and content go when the send stops, so a machine that sends many
  folders no longer accumulates them.
- The bytes a live reader asks for after the sender has stopped may already be collected if the
  reader is more than the interval behind. The sender holds the tags until it stops, so the window
  opens only after that, and a reader still on the wire then is slower than the interval. A stream
  that serves a reader more than ten seconds behind its own stop would need a longer interval or a
  tag handed over to the reader, and neither is worth it at these sizes.
- A store whose collector fails logs and stops collecting, which costs space but not correctness.