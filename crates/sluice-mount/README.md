# sluice-mount

**Owns:** exposing remote content as a read-only local path. `fuse3` on Linux, `winfsp` on Windows,
FSKit on macOS 26 and later. Feature-gated, and not built by default.

**Must never:** return a byte that has not been verified against the content root, and never block the
whole filesystem on one slow range fetch.

**Note:** the performance target names its access pattern deliberately: sequential reads with 1 MiB
readahead. Random 4 KiB reads through FUSE will not reach it, and the docs must say so.

**Tests:** mount a fixture, read it, compare bytes. Platform tests run on their own CI job.
