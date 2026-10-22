# The relay server: iroh-relay with both halves on, address discovery and the relay that carries
# traffic when two peers cannot reach each other (ADR 0013). iroh-relay is the upstream binary,
# built the way docker/rendezvous builds it, from the same base images and version. It starts
# through sluice-relay-guard, which refuses a config that would open the relay by mistake
# (ADR 0019, ADR 0020, ../guard/README.md).
#
# Build from docker/, which both images share for the guard's sources:
#   docker build -f docker/relay/Dockerfile -t sluice-relay docker
# Dockerfile.dockerignore, beside this file, lets in only the two configs and the guard's
# manifest, lock file and sources.

FROM rust:1.99.1-slim-trixie@sha256:4df2ae8cfdfebdfc13a65da7f253b73d68983dc0339f2719bc17a279c913b13d AS build

# The release Cargo.lock resolves for iroh-relay, so the server and the probe's iroh come from the
# same release, and the guard's schema is that release's: a test of the guard fails if this line
# names another. Move it with docker/rendezvous, and review the guard's schema first.
ARG IROH_RELAY_VERSION=1.4.0

# --locked uses the lockfile the crate was published with. The binary needs the server feature.
RUN cargo install iroh-relay --version "=${IROH_RELAY_VERSION}" --locked --features server --root /out

# After iroh-relay, so a change to the guard does not rebuild the server. --locked uses the guard's
# own Cargo.lock, whose versions are the root lock's.
COPY guard /src/guard
RUN cargo install --path /src/guard --locked --root /out

# distroless has no shell, no package manager and no root login. cc is the glibc and libgcc layer
# the dynamically linked binaries need.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

COPY --from=build /out/bin/iroh-relay /usr/local/bin/iroh-relay
COPY --from=build /out/bin/sluice-relay-guard /usr/local/bin/sluice-relay-guard
COPY relay/relay.toml relay/open.toml /etc/relay/

USER 65532:65532

# The server logs errors only unless RUST_LOG is set, and this prints the lines that say it started.
ENV RUST_LOG=info

# 443 is the relay over HTTPS, 80 the plain HTTP captive portal check that iroh clients make, and
# 7842 address discovery over QUIC.
EXPOSE 443/tcp 80/tcp 7842/udp

# The guard checks the config and then replaces itself with iroh-relay, which so runs as PID 1. An
# --entrypoint that names iroh-relay skips every check (README.md).
ENTRYPOINT ["/usr/local/bin/sluice-relay-guard"]

# The closed config: only the endpoint ids on its allowlist may use the relay, and it ships empty.
# Open mode is `--open --config-path /etc/relay/open.toml`, and only that (README.md).
CMD ["--config-path", "/etc/relay/relay.toml"]
