# Contributing

Sluice is built in the open, and a small change with a test is the most useful kind. Read the crate
README before you change a crate: it says what the crate owns and what it must never do.

## Getting started

```
git clone https://github.com/sanamhub/sluice && cd sluice
cargo nextest run
```

That needs a Rust toolchain, `cargo-nextest` and no network or daemon. If it fails on a clean clone,
that is a bug worth reporting by itself.

| To | Start with |
|---|---|
| Understand the design | [`docs/design.md`](docs/design.md), then [`docs/adr/`](docs/adr/README.md) |
| Understand the wire format | [`SPEC.md`](SPEC.md) |
| Fix something small | Issues labelled `good first issue` |
| Add a network condition to the benchmarks | `bench/harness/` |
| Add a `doctor` check | `crates/sluice-cli` |

## How the code is written

`SPEC.md` defines the wire. If the code and the spec disagree, the code is wrong, and a wire change
edits the spec and adds vectors under `tests/vectors/` in the same pull request.

Work inside one crate at a time where you can. [`docs/roadmap.md`](docs/roadmap.md#not-built) lists
what Sluice deliberately does not build. A pull request that adds one of those, or a dependency that
duplicates what we already have, needs a discussion first.

Tests decide whether a change is done. Protocol work needs property tests, parsing needs a fuzz
target, and concurrency needs a `turmoil` test that prints its seed on failure.

CI enforces the code rules. There is no `unsafe` outside `sluice-core` and the FFI crates.
Libraries use `thiserror` and binaries `anyhow`. There are six error variants and no more. Public
APIs use concrete types and enum dispatch, not traits or generics, and each transfer has one actor
with owned state and messages. Functions stay under 60 lines in the sans-IO crates, and source files
under 400 lines everywhere. Every public item in a library has a doc comment with an example.
Tracing spans carry the transfer id, and filenames and tickets are never logged at the default
level.

Performance is part of the interface. File bytes never cross a language boundary or an API, an idle
process does not wake, nothing is initialized before a command needs it, and a cold start to a
printed result stays under 30 ms. User files are read where they are and copied only when asked.

Three areas get a line-by-line review from their code owner, whoever wrote the change: the fsync
barrier and resume logic, the verified-range trust boundary, and the C ABI in `sluice-ffi`.

## Before opening a pull request

```
cargo nextest run
cargo clippy --all-targets -- -D warnings
cargo deny check
```

CI runs the same commands, plus the fuzz targets, the binary size and idle memory gates, and the
cold start measurement. A change that makes any of them worse fails.

## Writing

Prose, comments and commit messages read like one person wrote them. Keep a paragraph to a few
sentences, use the plain word, and reach for a number before an adjective.

No em dashes. No closing line that repeats the section. No "seamless", "robust", "leverage" or
"delve". Say how a claim was checked, or say that it is a guess.

Commit messages use Conventional Commits: a lower-case imperative summary under 72 characters, then
a body that says why the change was needed. The diff already says what changed.

## Decisions

A decision that is not obvious from the code gets an ADR in `docs/adr/`. If a pull request discussion
is about something an ADR already settled, link the ADR. If the ADR is wrong, propose one that
supersedes it.

Open a discussion before working on anything on the ["not built"
list](docs/roadmap.md#not-built), or on anything larger than a few hundred lines.

## Protocol changes

`SPEC.md` changes before the code does. A wire change needs the spec edit, test vectors in
`tests/vectors/`, and a note on interoperability, in the same pull request. Breaking changes bump
the ALPN; additive ones take a capability bit.

## Commits and releases

Conventional Commits. `release-plz` builds the version and changelog from them, so a commit message
is a release note. Do not edit `CHANGELOG.md` by hand.

## Code you didn't write by hand

Generated code is held to the same bar as anything else: it passes the tests, stays inside its
crate, and you can explain every line in review.

- Say in the pull request description which parts were generated, so review can focus there.
- Security reports need a reproduction (see [`SECURITY.md`](SECURITY.md)).

## Conduct

See [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).
