# The rendezvous server: iroh-relay with the relay half off, so it tells peers their own addresses
# and moves no transfer bytes (ADR 0013). iroh-relay is the upstream binary. It starts through
# sluice-relay-guard in `--rendezvous` mode, which refuses a config that would turn the relay half
# on, or open anything else by default (ADR 0020, ../guard/README.md).
#
# Build from docker/, which both images share for the guard's sources:
#   docker build -f docker/rendezvous/Dockerfile -t sluice-rendezvous docker
# BuildKit reads Dockerfile.dockerignore, beside this file, and lets in only manual.toml and
# the guard's manifest, lock file and sources. The legacy builder reads only .dockerignore in
# docker/, which lets in the same for both images. Either way the COPY lines below name only what
# the build needs, so a stray folder in docker/ never reaches the build stage.

FROM rust:1.99.1-slim-trixie@sha256:4df2ae8cfdfebdfc13a65da7f253b73d68983dc0339f2719bc17a279c913b13d AS build

# The release Cargo.lock resolves for iroh-relay, so the server and the probe's iroh come from the
# same release, and the guard's schema is that release's: a test of the guard fails if this line
# names another. Move it with docker/relay, and review the guard's schema first.
ARG IROH_RELAY_VERSION=1.4.1

# --locked uses the lockfile the crate was published with. The binary needs the server feature.
# The same line as docker/relay's, so the two images share this layer.
RUN cargo install iroh-relay --version "=${IROH_RELAY_VERSION}" --locked --features server --root /out

# After iroh-relay, so a change to the guard does not rebuild the server. --locked uses the guard's
# own Cargo.lock, whose versions are the root lock's.
COPY guard/Cargo.toml guard/Cargo.lock /src/guard/
COPY guard/src /src/guard/src
RUN cargo install --path /src/guard --locked --root /out

# distroless has no shell, no package manager and no root login. cc is the glibc and libgcc layer
# the dynamically linked binaries need.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

COPY --from=build /out/bin/iroh-relay /usr/local/bin/iroh-relay
COPY --from=build /out/bin/sluice-relay-guard /usr/local/bin/sluice-relay-guard
COPY rendezvous/manual.toml /etc/rendezvous/

USER 65532:65532

# The server logs errors only unless RUST_LOG is set, and this is the one line that says it started.
ENV RUST_LOG=info

EXPOSE 7842/udp

# The guard checks the config, then replaces itself with iroh-relay, which so runs as PID 1. An
# --entrypoint that names iroh-relay skips every check (README.md).
ENTRYPOINT ["/usr/local/bin/sluice-relay-guard"]

# iroh-relay stops on SIGINT and not on SIGTERM (main.rs:610-614), so this is what makes
# `docker stop` return in about a second instead of waiting out its grace period and killing the
# process. It reaches the relay through the exec.
STOPSIGNAL SIGINT

# The guard passes iroh-relay the --dev it needs with the relay half off (README.md), and refuses
# --dev from the command line.
CMD ["--rendezvous", "--config-path", "/etc/rendezvous/manual.toml"]
