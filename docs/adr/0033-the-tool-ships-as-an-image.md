# 0033. The tool ships as a container image beside the relay

**Status:** Accepted **Date:** 2026-10-29 **Approved by:** @sanamhub **Relates to:** ADR 0003, ADR 0013, ADR 0028

## Context

The 0.1.0 release ships the `sluice` binary for six targets and the `relay` image, and nothing
else. A user who runs a service in a container can pull the relay, but has to download and unpack
the binary on the host to send or receive a folder. That is two ways to get the same program, and
the container has no supported way to hold the identity that makes a ticket work again.

The relay image's decisions already fit a second image: two base images pinned by digest, a
distroless runtime with no shell, uid 65532, one build per native architecture, syft SBOMs, and a
push path that attests each architecture before it writes the version index. The tool needs one
thing the relay does not: a writable home, because its identity, queue and partial stores outlive a
container.

## Decision

1. **`ghcr.io/sanamhub/sluice` is built and released** for `linux/amd64` and `linux/arm64`, from
   `docker/cli/Dockerfile` with the repository root as its context. The binary is compiled from the
   tagged tree with the release lock file, not installed from a registry.
2. **It runs as uid 65532 with no shell**, on the same distroless cc image as the relay, and its
   entry point is `sluice`.
3. **The home is a volume.** `SLUICE_HOME` is `/home/sluice` and the image declares that path a
   volume. A container that mounts the same volume is the same endpoint; one without a volume gets a
   fresh identity. The home belongs to 65532, so a bind mount has to be given to that user.
4. **It follows the relay's pipeline.** `images.yml` builds it on every pull request that touches
   the sources, runs `--version`, `id` and `doctor` in the image, and sends a file between two
   containers on an internal network with no route out. `release.yml` builds it per architecture,
   waits for the GitHub release, pushes each architecture with provenance and a per-architecture
   SBOM, attests them, and only then writes the version index. `push-image.sh` and the tag scheme
   are the ones the relay uses.
5. **The two images are separate packages.** `sluice-relay` and `sluice` each have their own
   visibility, tags and rollback. Both must build before either is pushed, because the publishing
   jobs start only after both architectures of both images have built; after that each package is
   pushed on its own, and a failure in one does not roll the other back.

## Consequences

- A user can send or receive without installing the binary on the host, and can keep one identity
  across containers with a named volume.
- The image is another artifact to build, smoke test, attest, make public and delete on a rollback.
  The release workflow has two publication jobs, one per image, and each waits in the release
  environment.
- The binary is built twice: once by dist for the archives and once for the image. The two are the
  same commit and lock file but not the same bytes, so an archive's attestation does not cover the
  image, and the image has its own.
- The default bridge does not give a direct path to a peer on another machine. The image works
  there through a relay; `--network host` on Linux is what a direct path needs. The image README
  says so.
- Revisit this if the tool ever needs a writable path outside the home, or if the image has to run
  on a host without glibc.
