# The relay server: iroh-relay with both halves on, address discovery and the relay that carries
# traffic when two peers cannot reach each other (ADR 0013). It is the upstream binary and nothing
# of ours, built the way docker/rendezvous builds it, from the same base images and version.
#
# Build from this directory: docker build -t sluice-relay docker/relay

FROM rust:1.99.1-slim-trixie@sha256:4df2ae8cfdfebdfc13a65da7f253b73d68983dc0339f2719bc17a279c913b13d AS build

# The release Cargo.lock resolves for iroh-relay, so the server and the probe's iroh come from the
# same release. Move it with docker/rendezvous.
ARG IROH_RELAY_VERSION=1.4.0

# --locked uses the lockfile the crate was published with. The binary needs the server feature.
RUN cargo install iroh-relay --version "=${IROH_RELAY_VERSION}" --locked --features server --root /out

# distroless has no shell, no package manager and no root login. cc is the glibc and libgcc layer
# the dynamically linked binary needs.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

COPY --from=build /out/bin/iroh-relay /usr/local/bin/iroh-relay
COPY relay.toml open.toml /etc/relay/

USER 65532:65532

# The server logs errors only unless RUST_LOG is set, and this prints the lines that say it started.
ENV RUST_LOG=info

# 443 is the relay over HTTPS, 80 the plain HTTP captive portal check that iroh clients make, and
# 7842 address discovery over QUIC.
EXPOSE 443/tcp 80/tcp 7842/udp

ENTRYPOINT ["/usr/local/bin/iroh-relay"]

# The closed config: only the endpoint ids on its allowlist may use the relay, and it ships empty.
# open.toml is used only when the operator names it on the command line (README.md).
CMD ["--config-path", "/etc/relay/relay.toml"]
