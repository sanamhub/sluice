# The rendezvous server: iroh-relay with the relay half off, so it tells peers their own addresses
# and moves no transfer bytes (ADR 0013). It is the upstream binary and nothing of ours.
#
# Build from this directory: docker build -t sluice-rendezvous docker/rendezvous

FROM rust:1.99.1-slim-trixie@sha256:4df2ae8cfdfebdfc13a65da7f253b73d68983dc0339f2719bc17a279c913b13d AS build

# The release Cargo.lock resolves for iroh-relay, so the server and the probe's iroh come from the
# same release. Move them together.
ARG IROH_RELAY_VERSION=1.3.0

# --locked uses the lockfile the crate was published with. The binary needs the server feature.
RUN cargo install iroh-relay --version "=${IROH_RELAY_VERSION}" --locked --features server --root /out

# distroless has no shell, no package manager and no root login. cc is the glibc and libgcc layer
# the dynamically linked binary needs.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

COPY --from=build /out/bin/iroh-relay /usr/local/bin/iroh-relay
COPY manual.toml /etc/rendezvous/

USER 65532:65532

# The server logs errors only unless RUST_LOG is set, and this is the one line that says it started.
ENV RUST_LOG=info

EXPOSE 7842/udp

ENTRYPOINT ["/usr/local/bin/iroh-relay"]

# --dev is required with the relay half off, see README.md. It has no other effect here, because
# there is no HTTP server for it to switch to plain HTTP.
CMD ["--dev", "--config-path", "/etc/rendezvous/manual.toml"]
