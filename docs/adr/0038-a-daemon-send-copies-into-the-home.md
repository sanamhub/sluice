# 0038. A daemon send copies the content into the home

**Status:** Proposed **Date:** 2026-11-13 **Relates to:** ADR 0006, ADR 0030, ADR 0037

## Context

A one-shot `sluice send` hashes a file where it lives, keeps only the outboard in its store, and
reads the file again by path each time a receiver asks for a range (ADR 0006). That is safe because
the user who typed the path is the user who owns the files: nothing but that user's own change to the
file can alter what is sent, and the receiver's hash check refuses a changed byte.

A daemon is different. Any program that runs as the user can ask it to send a path inside a root, and
the roots exist so that such a program can read only what the roots hold. A file that is read by path
at serve time can be swapped for a link after the daemon checked it. The bytes then go to the
receiver, and are refused there only after they have left the machine. A check made on the path before
the read cannot close that: the path is resolved again at the read.

## Decision

1. **A daemon send copies every file into the send's own store.** The files are found below the
   handles the daemon proved, never by path, with each open relative to a folder handle and never
   following a link. The walk, the hash and the copy use one open object, which is accepted only if it
   is the object the walk saw. The copy reads no more than the walk saw, so a file that grew, shrank
   or was replaced is refused before any of its bytes are served.
2. **A one-shot send is unchanged.** It still references its files where they live.
3. **The room is checked first.** A send refuses, before it imports, when the volume of the home has
   less free space than the files it copies. The store is removed when the send fails, is cancelled
   during its preparing, or ends.

## Consequences

- A daemon send uses disk space equal to the files it sends, for as long as it serves, in the home's
  temporary area. A terabyte send needs a terabyte free there.
- A daemon send of a file that changes while the walk or the copy runs fails with a clear message and
  sends nothing, where a one-shot send would send whatever the receiver's hash accepts.
- The copy reads every byte once more than a reference does, on a blocking thread.
