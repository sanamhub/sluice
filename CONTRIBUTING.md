# Contributing

## Getting started

```
git clone <repo> && cd sluice
cargo nextest run
```

That needs only a Rust toolchain: no network, no daemon. If it fails on a clean clone, that is a bug
worth reporting by itself.

| To | Start with |
|---|---|
| Understand the design | [`docs/design.md`](docs/design.md), then [`docs/adr/`](docs/adr/README.md) |
| Understand the wire format | [`SPEC.md`](SPEC.md) |
| Fix something small | Issues labelled `good first issue` |
| Add a network condition to the benchmarks | `bench/netem/` |
| Add a `doctor` check | `crates/sluice-cli` |
| Work on a language binding | `sdk/<language>/` |
| Translate the UI | `ui/src/locales/` |

Each crate has a short README saying what it owns, what it must never do, and where its tests are. Read
it before changing the crate.

## Before opening a pull request

```
cargo nextest run
cargo clippy --all-targets -- -D warnings
cargo deny check
buf lint proto && buf breaking proto --against '.git#tag=<last-release>'
```

CI runs the same commands, plus the benchmark, binary size and idle memory gates. A change that makes
any of them worse fails.

The code rules CI enforces are listed in [`AGENTS.md`](AGENTS.md#style-enforced-by-ci). Prose (docs,
comments, commit messages, pull request descriptions) follows
[`.claude/skills/writing-style/SKILL.md`](.claude/skills/writing-style/SKILL.md).

## Decisions

A decision that is not obvious from the code gets an ADR in `docs/adr/`. If a pull request discussion
is about something an ADR already settled, link the ADR. If the ADR is wrong, propose one that
supersedes it.

[`docs/roadmap.md`](docs/roadmap.md#not-built) lists what Sluice deliberately does not build. Open a
discussion before working on anything on that list, or on anything larger than a few hundred lines.

## Protocol changes

`SPEC.md` changes before the code does. A wire change needs the spec edit, test vectors in
`tests/vectors/`, and a note on interoperability, in the same pull request. Breaking changes bump the
ALPN; additive ones take a capability bit.

## Commits and releases

Conventional Commits. `release-plz` builds the version and changelog from them, so a commit message is
a release note. Do not edit `CHANGELOG.md` by hand.

## Code you didn't write by hand

Generated code is held to the same bar as anything else: it passes the tests, stays inside its crate,
and you can explain every line in review.

- Say in the pull request description which parts were generated, so review can focus there.
- Three areas need a line-by-line read and approval from their code owner, whoever wrote the change:
  - the fsync barrier and resume logic
  - the verified-range trust boundary
  - `sluice-ffi`
- Security reports need a reproduction (see `SECURITY.md`).

## Conduct

See [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).
