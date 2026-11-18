# sluice-relay

The relay image's server, in its own workspace because it needs `iroh-relay`'s `server` feature and
the root workspace's feature unification would put that feature into the CLI build.

It reads the `relay.toml` the guard checked, builds `iroh-relay`'s `RelayService` with a `/meet`
handler beside `/relay`, serves both on one TLS listener, keeps the plain-HTTP `/generate_204`
check, QUIC address discovery on its own port and metrics when the config asks for them. The
meeting place itself is `crates/sluice-meet`; this crate is the transport and the config.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

`--config-path <file>` is the only argument, because the guard passes the sealed copy's path.
`--version` prints the release for the image check.
