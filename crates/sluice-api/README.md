# sluice-api

**Owns:** the one service definition from `proto/sluice/v1`, served with `tonic` over a Unix socket or
Windows named pipe, and optionally over TCP. Also the generated client used by the CLI and the SDKs.

**Must never:** carry file bytes. This is a control plane. A client asks the engine to move bytes and
watches progress; it never transports them. Any endpoint that would accept or return file content is a
design error, not a feature request.

**Note:** the proto is governed by `buf lint` and `buf breaking`. It is the contract every SDK is
generated from, so a careless field change breaks every language at once.

**Tests:** round-trip tests per RPC, `buf breaking` against the previous release in CI.
