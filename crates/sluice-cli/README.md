# sluice-cli

**Owns:** the `sluice` binary. Subcommands, human output, `--json` output, shell completions, man pages,
progress rendering, and `sluice mcp` (stdio MCP server via `rmcp`).

**Must never:** open the store when a daemon holds the lock. Detect the lock and delegate over the
control socket instead. Never print a ticket to a log, and never colorize when `NO_COLOR` is set.

**Note:** cold start budget is 30 ms. Do no work before it is needed: no config parse, no store open, no
network probe on a command that does not require one.

**Tests:** `insta` snapshots for output, `hyperfine` for the cold-start budget, exit-code tests for
scripting.
