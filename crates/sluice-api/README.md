# sluice-api

**Owns:** the local control interface. The generated types and gRPC service of
`proto/sluice/v1/control.proto`, the `Service` struct that answers each call over the engine, the job
table, the roots a daemon may use, and the daemon that serves the interface on a Unix socket or a
Windows named pipe.

**Must never:**

- carry a file byte: it is a control plane
- put a ticket in a log line, an error, an event or a job summary, or return one to any call but the
  one that made it
- listen on a TCP port, loopback included
- serve a connection whose peer is not the user the daemon runs as
- read or write a path that is not under a root the daemon was started with
- grow a buffer for a slow watcher: it drops events and says how many

**Notes:** `Service` is a plain struct with an async method per call, so the MCP server can call it in
process with no socket. The generated code is checked in at `src/gen/sluice.v1.rs` and the workspace
build needs no `protoc`. To rewrite it after a schema change, run
`SLUICE_API_WRITE_GENERATED=1 cargo test -p sluice-api --test generated`.

**Tests:** unit tests for the roots, the watcher buffer, the job table and each platform's peer
check; the daemon's end-to-end tests run two `sluice daemon` processes from `sluice-cli`.
