# 0037. The control interface is gRPC on a socket or pipe only this user can reach

**Status:** Proposed **Date:** 2026-11-12 **Relates to:** ADR 0003, ADR 0017

## Context

`proto/sluice/v1/control.proto` defines the local interface, and `SPEC.md` section 11 says it is served
over a Unix socket or a Windows named pipe. A daemon needs a transport for it, a way to check who is
at the other end, and generated code that a plain `cargo build` can use without a protobuf compiler
installed.

Two transports were possible: gRPC over the socket, or a length-prefixed protobuf message per call.
The second is smaller, but it makes every other language write its own framing and its own
streaming for `WatchEvents`, and the schema is already a gRPC service. `cargo deny check` passes
with `tonic` and its dependencies, with no new exception.

## Decision

1. **gRPC with `tonic` over the socket or pipe.** The server limits a message to 1 MiB in both
   directions, a connection to 64 open calls, and the daemon to 32 connections at once.
2. **The generated code is checked in** under `crates/sluice-api/src/gen`. A test compiles the
   schema again, with `protox`, a protobuf compiler written in Rust, so it needs no installed
   program, and fails when the checked-in copy differs. Building the workspace runs no generator.
3. **The listener is private to the user.** On Unix it is a socket in a folder of mode 0700 inside
   the home, and on Windows a named pipe whose access list names this user, `SYSTEM` and
   `Administrators`. There is no TCP listener, not even on loopback.
4. **Every connection's peer is checked when it is accepted.** On Linux the kernel's `SO_PEERCRED` and
   on macOS `getpeereid` give the user id, and on Windows the pipe's client process id gives the
   token's user SID. Another user is refused before the server sees the connection. The Windows
   client checks the same of the process that made the pipe.
5. **One plain struct, `Service`, has an async method for each call.** The gRPC adapter calls it, and
   an MCP server can call it in process with no socket.
6. **`Shutdown` is a call of the interface**, so `sluice daemon stop` needs no process id and no
   signal, which Windows does not have in the same form.

## Consequences

- The binary carries `tonic`, `hyper` and `tower`, which no other command uses.
- A path is opened from its root's handle one component at a time (`openat2` with
  `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV` on Linux, `openat`
  with `O_NOFOLLOW` and a volume check elsewhere, and on Windows each component opened as itself and
  proved from its handle to be a child of its parent's). The engine is given the path and the identity
  of the object the walk opened, and checks that identity where it opens the path itself.
