# sluice-probe

**Measures:** how fast `iroh` and `iroh-blobs` move bytes between two machines, with one connection or
several, under Cubic, BBRv3 or NewReno, with default or tuned windows. It is the M0 measurement from
ADR 0010 and the tool the T3 harness drives. It is for the people who tune the transport, not for
users, and its output is for machines: one ticket line from `serve`, one JSON line from `fetch`,
everything else on stderr.

**Must never:**

- be a dependency of a product crate
- choose a relay silently. `--relay` defaults to `disabled`, and a ticket made without one carries
  only direct addresses (ADR 0011).
- report a number it did not read. A field iroh does not expose is `null`.
- keep going after a handshake that did not negotiate `X25519MLKEM768` (ADR 0012). `fetch` exits
  non-zero instead.

**Notes:**

- It needs Rust 1.91, because `iroh` 1.3.0 and `iroh-blobs` 0.103.1 do. The msrv job leaves it out.
- `serve` holds the test data in memory in `blobs` mode, so `--size` has to fit in RAM. The disk is
  not part of what `fetch --sink discard` measures.
- `raw` mode repeats a 64 MiB block of seeded data to fill each stream. QUIC neither compresses nor
  deduplicates, so it costs the same on the wire as fresh data, and the sender never waits on the
  generator.
- `elapsed_s` runs from the first request to the last verified byte. Handshakes are not in it.
- `gso_segments` and `gro_segments` are always `null`. `noq` reports the segment limits per socket and
  iroh does not expose that socket. `ConnectionStats` has an `ios` counter that is hard-wired to 0 and
  marked deprecated. What iroh does count is `recv_datagrams` and `recv_gro_batches`, which the result
  carries as they are. They show whether GRO ran, not how many segments it coalesced.
- `cpu_user_s` and `cpu_sys_s` come from `getrusage` on Linux and macOS and are `null` on Windows.
- Numbers from one machine are not results. The T3 harness runs the probe on a pair of hosts.

**Tests:** `tests/loopback.rs` starts `serve` and `fetch` as processes with 8 MiB, for `raw` and
`blobs`, `bbr3` and `cubic`, and 1 and 4 connections. It checks the JSON, the byte count and the key
exchange group, and runs under `cargo nextest run` on all three operating systems.

## Run

On the machine that holds the data:

```
probe serve --size 10GiB --mode blobs --congestion bbr3 --window tuned
```

It prints one ticket line, then serves until killed. On the other machine:

```
probe fetch <ticket> --mode blobs --connections 4 --congestion bbr3 --window tuned
```

The ticket carries the mode, size, seed and addresses, so `fetch` needs none of them again. Both ends
take `--congestion` and `--window`, because the sender's controller and the receiver's windows both
limit throughput.

| Option          | Values                          | Default         | `serve` | `fetch`                    |
| --------------- | ------------------------------- | --------------- | ------- | -------------------------- |
| `--mode`        | `raw`, `blobs`                  | `blobs`         | yes     | yes, must match the ticket |
| `--congestion`  | `bbr3`, `cubic`, `newreno`      | `bbr3`          | yes     | yes                        |
| `--window`      | `default`, `tuned`              | `default`       | yes     | yes                        |
| `--relay`       | `disabled` or a relay URL       | `disabled`      | yes     | yes                        |
| `--size`        | bytes, with `KiB`, `MiB`, `GiB` | `1GiB`          | yes     | no                         |
| `--seed`        | `u64`                           | `0`             | yes     | no                         |
| `--connections` | 1 to 16                         | `1`             | no      | yes                        |
| `--sink`        | `discard`, `store`              | `discard`       | no      | yes                        |
| `--dir`         | directory                       | a temporary one | no      | yes                        |

`tuned` sets the stream and connection receive windows and the send window to 75 000 000 bytes, the
bandwidth-delay product of 10 Gbit/s at 60 ms. The values in force are in every result.

## Result

One line. This one is wrapped for reading, and comes from a same-machine run on Windows (`serve` and
`fetch` on one host, 1 GiB, release build). It shows the shape of the output. It says nothing about any
link. On Windows `cpu_user_s` and `cpu_sys_s` are `null` and no GRO batches are counted.

```
{"mode":"blobs","size_bytes":1073741824,"received_bytes":1073741824,"connections":1,
 "congestion":"bbr3",
 "window":{"mode":"default","stream_receive_bytes":1250000,
           "receive_bytes":4611686018427387903,"send_bytes":10000000},
 "sink":"discard","relay":"disabled","elapsed_s":10.6149166,"throughput_gbit_s":0.8092324146946195,
 "path":"direct","rtt_ms":0.056201,"cpu_user_s":null,"cpu_sys_s":null,
 "key_exchange_group":"X25519MLKEM768","gso_segments":null,"gro_segments":null,
 "recv_datagrams":761168,"recv_gro_batches":0,"iroh_version":"1.3.0","iroh_blobs_version":"0.103.1",
 "verified_hash":"7b343701fd41de622e1208b9490efd7f5a54eee0fe68c052e522500f55aa0fbc"}
```

`fetch` exits non-zero if the received byte count differs from `size_bytes`, if a blob does not verify
against the root, or if any connection negotiated a key exchange group other than `X25519MLKEM768`.
