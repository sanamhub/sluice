# The sluice tool: the commands, the manifests and the pipe a user runs, with no server of its own
# (ADR 0033). It is the release's own binary built from this tree, and not a package from a
# registry, so the image runs the commit the tag names.
#
# The build context is the repository root, not docker/, because the binary is compiled from the
# workspace:
#   docker build -f docker/cli/Dockerfile -t sluice .
# The root .dockerignore lets in only the manifests and sources the build reads.
#
# The base images, the digest pins and the user are the relay image's (docker/relay/Dockerfile).
# What differs is below: the build compiles sluice instead of installing iroh-relay, the entry
# point is the tool itself rather than a guard, and the image declares the home it writes.

# rust 1.99.1 slim trixie, the toolchain rust-toolchain.toml names, pinned by digest.
FROM rust:1.99.1-slim-trixie@sha256:4df2ae8cfdfebdfc13a65da7f253b73d68983dc0339f2719bc17a279c913b13d AS build

WORKDIR /src

# The manifests of every workspace member, because Cargo resolves the whole workspace before it
# builds one package. The sources of the members this image does not build are here for the same
# reason: Cargo reads each member's target before it reads the lock file.
COPY Cargo.toml Cargo.lock ./
COPY crates/sluice-cli/Cargo.toml crates/sluice-cli/
COPY crates/sluice-core/Cargo.toml crates/sluice-core/
COPY crates/sluice-live/Cargo.toml crates/sluice-live/
COPY bench/probe/Cargo.toml bench/probe/
COPY bench/harness/Cargo.toml bench/harness/
COPY bench/wasm-hash/Cargo.toml bench/wasm-hash/
COPY xtask/Cargo.toml xtask/

COPY crates/sluice-cli/build.rs crates/sluice-cli/
COPY crates/sluice-cli/src crates/sluice-cli/src
COPY crates/sluice-core/src crates/sluice-core/src
COPY crates/sluice-live/src crates/sluice-live/src
COPY bench/probe/build.rs bench/probe/
COPY bench/probe/src bench/probe/src
COPY bench/harness/src bench/harness/src
COPY bench/wasm-hash/src bench/wasm-hash/src
COPY xtask/src xtask/src

# --locked uses the release Cargo.lock. Release and not dist's fat-LTO profile: the archives are
# still the dist builds, and this image only needs the same commit's behaviour.
RUN cargo build --locked --release --package sluice-cli --bin sluice

# The home sluice creates belongs to the user the image runs as. It is made here because the
# runtime image has no shell to make it.
RUN install -d -m 0700 -o 65532 -g 65532 /out/home

# Distroless has no shell, no package manager and no root login, and cc is the glibc and libgcc
# layer the dynamically linked binary needs.
# gcr.io/distroless/cc-debian13:nonroot, the relay's runtime, pinned by digest.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

COPY --from=build /src/target/release/sluice /usr/local/bin/sluice
COPY --from=build --chown=65532:65532 /out/home /home/sluice

# $SLUICE_HOME pins the home to the one path the volume covers. With no volume a container gets a
# fresh identity, and with the same volume every container that mounts it is the same endpoint.
ENV SLUICE_HOME=/home/sluice
VOLUME ["/home/sluice"]

USER 65532:65532

# No ports are exposed: a send binds an ephemeral UDP port, which --port fixes in advance.
ENTRYPOINT ["sluice"]
