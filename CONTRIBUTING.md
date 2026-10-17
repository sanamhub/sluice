# Contributing

## Ten minutes to a merged change

```
git clone <repo> && cd sluice
cargo nextest run          # no network, no daemon, no .NET needed
```

If that does not pass on a clean clone, that is a bug worth reporting on its own.

## Where to start

| You want to | Look at |
|---|---|
| Understand the design | `plan.md`, then `docs/adr/` for why |
| Understand the wire format | `SPEC.md` |
| Fix a small thing | Issues labelled `good first issue` |
| Add a packaging target | `dist-workspace.toml` |
| Add a network condition to the benchmarks | `bench/netem/` |
| Add a `doctor` check | `crates/sluice-cli` |
| Add an SDK | `proto/sluice/v1`, then `sdks/` |
| Translate the UI | `ui/src/locales/` |

Each crate has a README that answers, in under 200 words, what it owns, what it must never do, and where
its tests live. Read that one file before touching a crate.

## Rules the CI enforces

The code rules are listed once, in [`AGENTS.md`](AGENTS.md) under "Style, enforced by CI", with the
commands to run before opening a change. On top of those, CI gates benchmarks, binary size, and idle
RSS, so a change that regresses them fails.

Prose (docs, comments, commit messages, pull request descriptions) follows
[`.claude/skills/writing-style/SKILL.md`](.claude/skills/writing-style/SKILL.md).

## Decisions

Non-obvious decisions get an ADR in `docs/adr/`, numbered, one per file. If you are about to argue in a
pull request comment about something an ADR already settled, link the ADR instead. If the ADR is wrong,
write the next one superseding it.

If you are proposing something on the deliberately-not-building list in `plan.md` section 3, open a
discussion first. That list exists so a solo maintainer does not drown, and a 4000-line surprise pull
request will not be merged no matter how good it is.

## Protocol changes

`SPEC.md` changes before the code does. A wire change needs: the spec edit, test vectors in
`tests/vectors/`, and an interop note. Wire-breaking changes bump the ALPN. Additive changes take a
capability bit.

## Commits and releases

Conventional commits. `release-plz` generates versions and the changelog, so commit messages become
release notes. Do not hand-edit `CHANGELOG.md`.

## AI-assisted contributions

Welcome, with the same bar as anything else: it must pass tests, respect the crate boundary, and be
something you can explain in review. Three areas are hand-reviewed line by line and are a bad fit for
generated code without close reading: the fsync barrier and resume logic, the verified-range trust
boundary, and anything in `sluice-ffi`. Silent errors there are the ones that end a project's reputation.

The rules:

- **Say so.** If a substantial part of a pull request was generated, say which part in the description.
  It changes how we review the change, not whether we accept it.
- **You own it.** You must be able to explain every line in review. "The model wrote it" is not an
  answer to a review question.
- **The three areas above need a human owner.** Changes there need approval from that area's CODEOWNER,
  who has read every line. This also applies to maintainers using AI.
- **Security reports need a reproduction.** A report without a reproducing input or test will be
  closed. That includes generated reports. See `SECURITY.md`.
- **Keep it small.** A large generated change with no prior discussion will be closed unread, for the
  same reason as the 4000-line rule above.

## Code of conduct

See `CODE_OF_CONDUCT.md`. Be decent. Slow, thoughtful review beats fast, careless merging.
