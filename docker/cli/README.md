# cli

The `sluice` image runs the same binary as the release archives, for `linux/amd64` and
`linux/arm64`. It is the tool and nothing else: no relay, no server and no shell. A container can
send a folder that is mounted into it, receive into another, and keep its endpoint id on a volume.

```
docker pull ghcr.io/sanamhub/sluice:0.1.0
```

The image runs as uid 65532 on `gcr.io/distroless/cc-debian13:nonroot`, pinned by digest, so it has
no shell and no package manager. The entry point is `sluice`, and the base has the glibc and libgcc
the binary needs. It is built from the commit the tag names. How to verify the tag and its
attestations is in [`docs/install.md`](../../docs/install.md#the-sluice-image).

## The home and the identity

`SLUICE_HOME` is `/home/sluice`, and the image declares that path as a volume. The identity key,
the queue and the temporary store live there.

```
docker volume create sluice-home
docker run --rm -v sluice-home:/home/sluice ghcr.io/sanamhub/sluice:0.1.0 id
```

Every container that mounts `sluice-home` at `/home/sluice` prints the same id, so a ticket made in
one container can be received in another, and a resume finds the same home. A container without the
volume writes a new home on its own layer, gets a new id, and cannot use tickets or partial stores
made under the old one. Remove the volume and the identity is gone:

```
docker volume rm sluice-home
```

The volume belongs to uid 65532. A named volume takes that owner from the image; a bind mount keeps
the host's owner and needs `chown 65532:65532` or a mode that lets 65532 write.

## Send and receive

Mount the files at a path and name that path:

```
docker run --rm -it --network host -v sluice-home:/home/sluice \
  -v "$PWD:/data" ghcr.io/sanamhub/sluice:0.1.0 send /data/report.pdf
```

It prints a ticket and serves until one complete download, as the binary does. On the other
machine:

```
docker run --rm -it --network host -v sluice-home:/home/sluice \
  -v "$PWD/out:/out" ghcr.io/sanamhub/sluice:0.1.0 recv <ticket> --out /out
```

The mounted folder has to be readable by uid 65532 to send from it, and writable to receive into
it. The `-it` is what lets Ctrl+C stop a sender that is still serving; without a terminal it serves
until the one download arrives.

`--network host` is what gives the container the host's addresses on Linux, so a direct path works
as it does for the binary on the host. On Docker's default bridge the container has a private
address, which a peer on another machine cannot dial, so the transfer is carried by a relay passed
with `--relay`. It still works, at the relay's rate. Two containers on the same user-defined
network can reach each other directly, which is what the smoke test does with no route out.

A UDP port is chosen at start. To name it for a firewall rule, add `--port 7842` to `send` and
publish that port:

```
docker run --rm -it --network host -v sluice-home:/home/sluice -v "$PWD:/data" \
  ghcr.io/sanamhub/sluice:0.1.0 send /data/report.pdf --port 7842
```

## Read-only root

The only path the image writes is the home. With a volume mounted there, the root can be read-only:

```
docker run --rm --read-only -v sluice-home:/home/sluice ghcr.io/sanamhub/sluice:0.1.0 doctor
```

`doctor` only reads. It reports the home, its permissions, the lock and the identity, as
[`crates/sluice-cli/README.md`](../../crates/sluice-cli/README.md) describes.

## Difference from the relay image

The two images share the base images and their digest pins, the non-root user and the release
pipeline. They differ where the program does:

- The build compiles `sluice-cli` from this tree instead of installing `iroh-relay` and the guard.
- The entry point is the tool. There is no config to check and no guard, because every command
  validates its own arguments and ticket.
- The home is a declared volume. The relay keeps its state in its config and mounts its
  certificates; it has nothing to remember between runs.
- No port is exposed. A relay listens on fixed ports; a send binds an ephemeral UDP port unless
  `--port` names one.

## Build

From the repository root:

```
docker build -f docker/cli/Dockerfile -t sluice .
```

The root `.dockerignore` keeps the context to the manifests and sources the build reads. The
distroless base has no shell, so run the binary directly:

```
docker run --rm sluice --version
```
