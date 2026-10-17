# sluice-delta

**Owns:** the delta hint layer. FastCDC chunk signature lists, matching, and computing the ranges a
receiver still needs. Spec: `SPEC.md` section 7.

**Must never:** influence a verification outcome. Delta is a hint. Every assembled result is verified
against the real BLAKE3 root, and a bad match costs a refetch and nothing more. Signatures flow provider
to requester only, so a requester never discloses its local content.

**Must never:** touch the network, the filesystem, or the clock. Sans-IO, same as `sluice-live`.

**Tests:** `proptest` on range algebra, `fuzz/` on signature list parsing, `delta/` vectors.
