# 0035. A delta tells the sender which parts of the candidate you hold

**Status:** Proposed **Date:** 2026-11-12 **Corrects:** the consequence of ADR 0005 that the requester reveals nothing about its local files

## Context

ADR 0005 keeps delta a hint and sends signatures from the provider to the requester. It lists as a
consequence that "the requester reveals nothing about its local files". The engine that builds
the feature shows that consequence is not true, and the cause is in the design and not in a mistake
of the code.

A requester that rebuilds a file from a candidate fetches the rest. The ranges it fetches, and the
`NeedRanges` message of `SPEC.md` section 7 that names them, say which parts of the target it did
not need. The parts it left out are the parts it could rebuild: it holds those chunks. A provider
that is hostile can send a list that is not the file's own but a set of guesses, one chunk hash for
each position, and read from the answer which guesses the requester holds. For a candidate that is
a private document, each confirmed guess is a bit about it. The numbers: a list of one million
entries, one guess each, is up to one million bits about one file, on a connection the requester
opened to that provider on purpose.

Not sending `NeedRanges` does not help: the static protocol's own range requests carry the same
information, and a requester that fetched the whole file would not be using a delta. The fix is not
in the wire. It is in saying what is disclosed, to whom, and about what, and in making the user
choose it.

## Decision

1. **The requester discloses which parts of the candidate it holds, to the sender it fetches from,
   when it sends `NeedRanges`, and about the one file it was given.** `SPEC.md` section 7 and
   `docs/threat-model.md` say so. The sentence of ADR 0005 that the requester reveals nothing is
   corrected by this ADR; ADR 0005 itself is not edited.
2. **The candidate is only a path the user named.** `sluice recv --delta-from PATH` names a file, or
   a folder whose files are matched by the ticket's own paths. No implicit candidate is ever
   chosen: not an output of the same name that exists, not a file with the same hash in a cache,
   not a file found by scanning. A receive without the flag asks for no list and sends no ranges.
3. **A file with no candidate is not asked about.** No list is requested and no `NeedRanges` is sent
   for it, so what the sender learns is limited to files the user offered.
4. **The help text of the flag says it.** A user reads that the sender learns which ranges are
   needed before the flag does anything.
5. **Nothing about the candidate leaves except the ranges.** No signature, no hash and no length of
   the candidate is sent: the requester sends the 32 bytes of the root it asks about and one
   `NeedRanges`, and a test reads both from the sender's side.

## Consequences

- A user who must not tell a sender what they hold fetches plainly. That costs bandwidth and nothing
  else.
- A sender can confirm guesses about a candidate. The cost to it is one list per file asked about
  and the requester's connection, so the leak is bounded by the requester's own choice of files and
  by how many entries a list may carry.
- ADR 0005's decision stands: the hint decides nothing about correctness, and the assembled file is
  verified against the root before it is used.
- Revisit if the static protocol gains a way to request ranges without naming them, or if a delta is
  added that is initiated by the provider, which would change who learns what.
