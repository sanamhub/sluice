# 0032. The rendezvous image is no longer built or released

**Status:** Accepted **Date:** 2026-10-29 **Approved by:** @sanamhub **Supersedes:** ADR 0020's
rendezvous mode **Relates to:** ADR 0013, ADR 0019, ADR 0020, ADR 0021, ADR 0028

## Context

ADR 0028 answered the question ADR 0013 left open: iroh coordinates a hole punch through a relay
connection, so address discovery alone connects far fewer pairs than a meeting point does. It
retired the `rendezvous` image and the guard's `--rendezvous` mode in principle, but left both in
the tree until the release pipeline next changed, so that the pipeline and the tree moved in one
step.

The pipeline has now changed. The image built and published a second, weaker tier that ADR 0028
rejected; its documentation still told readers to run it; and the release workflow built, smoke
tested, saved and pushed it beside the relay on every tag. Keeping it any longer would make the
first release ship a server the design no longer recommends.

The relay image covers the case. As a meeting point it carries the coordination that hole punching
needs, and its per-client limit keeps that cheap; as a full relay it carries the transfer too. No
case needs a server that answers address discovery and nothing else.

## Decision

1. **The `rendezvous` image is removed.** `docker/rendezvous/` is gone, and so are its entries in
   `docker/.dockerignore`, the image matrices and push test of `images.yml`, the image builds,
   outputs, artifacts, publication matrix and index steps of `release.yml`, and
   `docker/smoke-test.sh`.
2. **The guard has no rendezvous mode.** `--rendezvous` is an unknown argument. A config that says
   `enable_relay = false` is refused, because the image exists to carry traffic; a deleted line can
   no longer turn a discovery-only server into a relay, because there is no such server.
3. **The documentation describes one image.** The relay README, the guard README, the threat model,
   the design, the release runbook and the bench READMEs no longer tell a reader to build, run or
   pull a second image. The bench harness still measures a rendezvous server for research: it runs
   `iroh-relay` with the relay half off in its own process and does not build or pull an image.
4. **This supersedes the rendezvous mode of ADR 0020** and completes the removal ADR 0028
   announced. The rest of ADR 0020 stands. ADR 0013 was already superseded by ADR 0028.

`docker/guard` keeps its refusal of a config whose relay half is off. That check used to tell the
operator to add `--rendezvous`; it now says that the image carries traffic and the setting must be
true or absent.

## Consequences

- One image to build, smoke test, sign, attest, publish and explain. The release workflow has one
  image build per architecture and one publication matrix, and its `host` job waits for one image.
- An operator who ran the old image runs `relay` with `meeting-point.toml` instead. ADR 0028 gives
  the two recipes, and `docker/relay/README.md` shows the meeting-point limits.
- The guard's fuzz target no longer checks a rendezvous config, and the fuzz seeds drop
  `docker/rendezvous/manual.toml`. A near-miss config with `enable_relay = false` stays, now as a
  check that the relay image refuses a relay half that is off.
- A reader of ADR 0013, ADR 0019 or ADR 0020 finds the history there and the current state here,
  linked from the index. The old files are not edited.
