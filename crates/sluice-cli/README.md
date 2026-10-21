# sluice-cli

**Owns:** the `sluice` binary: subcommands, human output, `--json` output, shell completions, man
pages, progress display, `sluice doctor`, and `sluice mcp` (an MCP server over stdio, using `rmcp`).

**Must never:**

- open the store while a daemon holds its lock. It detects the lock and sends the request over the
  control socket instead.
- print a ticket to a log
- use colour when `NO_COLOR` is set

**Notes:** the cold start budget is 30 ms. A command does no config parsing, store opening or network
probing unless it needs them. Every transfer line shows whether it is direct or relayed.

**Tests:** `insta` snapshots of the help text, exit-code tests for scripts, and `cargo xtask cold-start` for
the cold start budget. That command builds `sluice` in release, then times `sluice id` and
`sluice --version`, 30 runs each after 3 warm-ups, and fails if a median is over 30 ms. CI runs it on
Linux and Windows.

## What scripts can rely on

The exit codes, the JSON shapes and the rules below are an interface. Scripts and agents depend on
them, so changing one is a breaking change and is tested. The wording of a message is not part of
it and can change in any release.

**Rules**

1. The output format never depends on whether stdout is a terminal. It is text unless `--json` is
   given, so `sluice id | clip` gets the bare id. A terminal decides only colour, progress and whether
   a prompt may be shown.
2. `--json` is global and works before or after the subcommand. With it, stdout holds only JSON, one
   object per line, with keys in `snake_case`. A failure then writes one JSON line to stderr and
   leaves stdout empty. The exit code is the same with or without `--json`. Without it, a failure is
   `sluice: <message>` on stderr.
3. `--color auto|always|never` is global and defaults to `auto`, which means colour only on a stream
   that is a terminal. `NO_COLOR` set to any non-empty value means `never` and wins over
   `--color always`. Help and usage errors follow the same choice.
4. A panic is a bug in sluice. It exits with code 70 and prints `sluice: internal error: <message>`
   to stderr, or the JSON error with kind `bug`, and not Rust's code 101 and backtrace. Setting
   `RUST_BACKTRACE` still shows where it happened.
5. A closed stdout is not an error. When the reader goes away, as in `sluice id | head -c 0`, sluice
   stops quietly with exit code 0. Any other failure to write to stdout is `Storage`, exit code 5.

**Exit codes**

| Code | Meaning |
|---|---|
| 0 | Success |
| 2 | A bad command line, as `clap` reports it. No JSON error is written for it |
| 3 | `Network`: a peer or relay could not be reached, or a connection failed mid-transfer. Retrying can help |
| 4 | `Verify`: received bytes did not match the BLAKE3 root. The bytes are discarded |
| 5 | `Storage`: the store or filesystem failed: full disk, permission or quota |
| 6 | `Policy`: sluice refused by rule, such as a home with a newer layout, a key that other users can read, or `--rotate` with nobody to ask |
| 70 | `Bug`: an internal invariant broke, or sluice panicked. A defect in sluice, never in the environment |
| 130 | `Cancelled`: the caller cancelled, for example by answering no to a prompt |

**JSON**

`sluice id --json` prints one object:

```
{"endpoint_id":"<64 lower-case hex characters>"}
```

A failure with `--json` is one line on stderr. `kind` is the variant in lower case: `network`,
`verify`, `storage`, `policy`, `cancelled` or `bug`. `message` is for people.

```
{"error":{"kind":"policy","message":"the store has layout version 2 and this sluice reads up to version 1. It does not write to a newer layout"}}
```

`sluice id --rotate` asks before it replaces the identity, and only on a terminal. With no terminal,
or with `--json`, it refuses with code 6 unless `--yes` is given.

**Other commands**

`sluice completions <shell>` prints the completion script for `bash`, `zsh`, `fish`, `powershell` or
`elvish`, with or without `--json`. `cargo xtask man <dir>` writes the man pages from the same
definition. They are built when wanted and are not checked in.
