# The relay server: iroh-relay's `/relay` and the short-code meeting place at `/meet` on one TLS
# listener (ADR 0041). It starts through sluice-relay-guard, which refuses a config that would open
# the relay by mistake (ADR 0019, ADR 0020, ../guard/README.md).
#
# Build from the repository root, which holds `crates/sluice-meet` as well as the two workspaces:
#   docker build -f docker/relay/Dockerfile -t sluice-relay .
# BuildKit reads Dockerfile.dockerignore, beside this file, and lets in only what the build needs.
# The legacy builder reads the root .dockerignore, which lets in the same files.

FROM rust:1.99.1-slim-trixie@sha256:4df2ae8cfdfebdfc13a65da7f253b73d68983dc0339f2719bc17a279c913b13d AS build

# The release the root Cargo.lock resolves for iroh-relay. The relay workspace pins the same
# version, and a test of the guard fails if this line names another.
ARG IROH_RELAY_VERSION=1.4.1

# The guard first, so its own lock and sources are one layer.
COPY docker/guard/Cargo.toml docker/guard/Cargo.lock /src/docker/guard/
COPY docker/guard/src /src/docker/guard/src
RUN cargo install --path /src/docker/guard --locked --root /out

# The meeting-place library is a path dependency of the relay workspace. Its manifest does not
# inherit the root workspace, so this folder is the whole dependency.
COPY crates/sluice-meet/Cargo.toml /src/crates/sluice-meet/
COPY crates/sluice-meet/words.txt /src/crates/sluice-meet/
COPY crates/sluice-meet/src /src/crates/sluice-meet/src

# The relay workspace, with its own lock. The tests are not copied: the image builds the binary.
COPY docker/relay-server/Cargo.toml docker/relay-server/Cargo.lock /src/docker/relay-server/
COPY docker/relay-server/src /src/docker/relay-server/src
RUN cargo build --manifest-path /src/docker/relay-server/Cargo.toml --locked --release \
    --target-dir /out

# distroless has no shell, no package manager and no root login. cc is the glibc and libgcc layer
# the dynamically linked binaries need.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

COPY --from=build /out/release/sluice-relay /usr/local/bin/sluice-relay
COPY --from=build /out/bin/sluice-relay-guard /usr/local/bin/sluice-relay-guard
COPY docker/relay/relay.toml docker/relay/open.toml docker/relay/meeting-point.toml /etc/relay/

USER 65532:65532

# The server logs errors only unless RUST_LOG is set, and this prints the lines that say it started.
ENV RUST_LOG=info

# 443 is the relay over HTTPS, 80 the plain HTTP captive portal check that iroh clients make, and
# 7842 address discovery over QUIC.
EXPOSE 443/tcp 80/tcp 7842/udp

# The guard checks the config and then replaces itself with sluice-relay, which so runs as PID 1.
ENTRYPOINT ["/usr/local/bin/sluice-relay-guard"]

# The relay stops on SIGINT and SIGTERM, and the Dockerfile sends SIGINT so `docker stop` returns
# without waiting out its grace period.
STOPSIGNAL SIGINT

# The closed config: only the endpoint ids on its allowlist may use the relay, and it ships empty.
# Open mode is `--open --config-path /etc/relay/open.toml`, and only that (README.md).
CMD ["--config-path", "/etc/relay/relay.toml"]
