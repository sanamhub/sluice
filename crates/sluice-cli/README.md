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

**Tests:** `insta` snapshots of output, `hyperfine` for cold start, and exit-code tests for scripts.
