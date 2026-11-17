# short-codes spike (T88)

A throwaway branch for ADR 0041. **Nothing here is for merging.** It answers three questions with
runnable evidence, and the ADR and `SPEC.md` section 4.1 are written from the answers.

## 1. One host and port

`iroh-relay` 1.4.1's library exports `RelayService`, `Handlers`, `TlsConfig` and
`RelayService::handle_connection`. A binary of ours registers `GET /meet` in `Handlers`, builds the
relay service with that handler, and serves both paths on one TLS listener. The upstream `Server`
cannot do this itself: its `ServerBuilder` is `pub(super)`.

`tests/exchange.rs::the_relay_and_the_meeting_place_share_one_tls_port` starts one server with a
self-signed certificate and, in one run:

- connects two `iroh-relay` clients through `/relay`, sends a Ping and checks the Pong, sends a
  Datagram from one to the other and checks the endpoint id and bytes;
- connects a `wss://…/meet` client to the same port and claims a nameplate.

QUIC address discovery stays available: `src/server.rs::tests::quic_address_discovery_runs_without_the_http_relay`
starts `iroh_relay::server::Server` with its relay half disabled and the same TLS config. The
non-exhaustive configs are built with `Default` and field assignment, not a struct literal.

## 2. SPAKE2 in the browser build

`spake2` 0.5.0-pre.0 with `getrandom` builds for `wasm32-unknown-unknown` with the tree's
`curve25519-dalek` 5 and `sha2` 0.11. `crates/sluice-wasm` gets an optional
`short-code-probe` feature that pulls SPAKE2 into the module; the feature is off by default and is
not part of the product.

Measured with `cargo build -p sluice-wasm --release --target wasm32-unknown-unknown`, built once
without the feature and once with it:

| Module | Raw | Brotli |
|---|---|---|
| `sluice_wasm.wasm` without the probe | 7 084 725 | 1 354 693 |
| with the SPAKE2 probe | 7 121 045 | 1 363 133 |
| **added** | **36 320** | **8 440** |

A standalone SPAKE2-only module is 84 342 raw and 23 746 brotli; the difference is code the browser
module already had (curve arithmetic, SHA-2, `getrandom`).

`spake2` 0.4.0, the newest stable, does **not** build for the target with this graph without adding
`getrandom` 0.2's `js` feature, and it brings a second `curve25519-dalek`. That is why the spike and
the ADR use the pre-release. Building `ring` for wasm needs `clang` on PATH; on this machine it is
the Swift toolchain's clang.

## 3. The exchange

`tests/exchange.rs` runs the full exchange through the real meeting place over WebSockets:

- `right_words_deliver_the_ticket`: `Claim`/`Join`, SPAKE2, both confirmations, sealed ticket, the
  receiver opens it, both see `Done`;
- `wrong_words_burn_the_nameplate_and_the_sender_sees_it`: a different word pair makes the tags
  disagree; the sender sends `Fail`, the meeting place burns the plate and sends `Burned` to both,
  and a later `Join` gets `NoCode`;
- `a_third_connection_to_a_held_nameplate_is_refused`: the third `Join` gets `Busy`;
- plus the relay test above.

`src/crypto.rs` unit tests cover the key schedule, a different word pair, and the AEAD's
transcript binding. In total: 11 unit tests and 4 integration tests.

## Sizes

The meeting place's own code adds **171 520 bytes** to a relay server binary of the same shape:
`serve.exe` (relay + `/meet`) is 4 567 040 bytes and `relay_only.exe` (the same relay service with no
`/meet`) is 4 395 520. The upstream `iroh-relay` 1.4.1 server binary built with `--locked
--features server` is 9 650 688; the spike binary is smaller because T89's config parser, metrics,
`clap` and QUIC startup are not in it. See the ADR's 172 KB estimate.

## Run it

```
cargo test --manifest-path spike/short-codes/Cargo.toml
cargo run --release --manifest-path spike/short-codes/Cargo.toml --bin brotli_size -- FILE...
```

The crate has its own `[workspace]` and lock file, so it cannot move the product's resolved
versions. The `short-code-probe` feature in `crates/sluice-wasm` is the only product-tree change,
and it is off by default.

## Limitations of this spike

- The meeting place keeps its state in one `Mutex<HashMap>`; the production one belongs to the
  relay binary and uses the same shape but with the guard-checked `[meet]` limits wired in.
- The accept loop scopes the peer IP with a task-local so the `Handlers` closure can see it.
  T89 keeps that or passes the address another way; the handler needs it for the per-IP rate.
- No config parser, no `memfd` handover, no image build, no browser client. Those are T89.
- A malicious relay can always drop or delay the meeting; the design protects the words and the
  ticket from the relay, not availability.
