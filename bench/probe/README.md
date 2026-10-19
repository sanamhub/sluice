# sluice-probe

**Measures:** how fast `iroh` and `iroh-blobs` move bytes between two machines, with one connection or
several, under Cubic, BBRv3 or NewReno, with default or tuned windows. It is the M0 measurement from
ADR 0010 and the tool the bench harness drives. It is for the people who tune the transport, not for
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
- `serve` holds the test data in memory in `blobs` mode unless `--store fs` is given, so `--size` has
  to fit in RAM by default. Raw mode keeps only a 64 MiB block in memory. The disk is not part of what
  `fetch --sink discard` measures with `--store mem`.
- `raw` mode repeats a 64 MiB block of seeded data to fill each stream. QUIC neither compresses nor
  deduplicates, so it costs the same on the wire as fresh data, and the sender never waits on the
  generator.
- `elapsed_s` runs from the first request to the last verified byte. Handshakes are not in it.
- `gso_segments` and `gro_segments` are always `null`. `noq` reports the segment limits per socket and
  iroh does not expose that socket. `ConnectionStats` has an `ios` counter that is hard-wired to 0 and
  marked deprecated. What iroh does count is `recv_datagrams` and `recv_gro_batches`, which the result
  carries as they are. They show whether GRO ran, not how many segments it coalesced.
- `cpu_user_s` and `cpu_sys_s` come from `getrusage` on Linux and macOS and are `null` on Windows.
- Numbers from one machine are not results. The bench harness runs the probe on a pair of hosts.

**Tests:** `tests/loopback.rs` starts `serve` and `fetch` as processes with 8 MiB, for `raw` and
`blobs`, `bbr3` and `cubic`, and 1 and 4 connections. It checks the JSON, the byte count and the key
exchange group, and runs under `cargo nextest run` on all three operating systems. One more case
puts 4 connections on 4 separate endpoints on both sides, and another serves 8 MiB from a file store
and checks that the store holds no copy of the data file. A last test asks for 64 GiB with
`--timeout 2` and checks the timeout line and the exit status.

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

| Option          | Values                          | Default                  | `serve`           | `fetch`                    |
| --------------- | ------------------------------- | ------------------------ | ----------------- | -------------------------- |
| `--mode`        | `raw`, `blobs`                  | `blobs`                  | yes               | yes, must match the ticket |
| `--congestion`  | `bbr3`, `cubic`, `newreno`      | `bbr3`                   | yes               | yes                        |
| `--window`      | `default`, `tuned`              | `default`                | yes               | yes                        |
| `--relay`       | `disabled` or a relay URL       | `disabled`               | yes               | yes                        |
| `--size`        | bytes, with `KiB`, `MiB`, `GiB` | `1GiB`                   | yes               | no                         |
| `--seed`        | `u64`                           | `0`                      | yes               | no                         |
| `--connections` | 1 to 16                         | `1`                      | no                | yes                        |
| `--sink`        | `discard`, `store`              | `discard`                | no                | yes                        |
| `--dir`         | directory                       | `fetch`: a temporary one | `--store fs` only | `--sink store` only        |
| `--store`       | `mem`, `fs`                     | `mem`                    | yes               | no                         |
| `--timeout`     | seconds, 1 or more              | `600`                    | no                | yes                        |
| `--endpoints`   | 1 to 16                         | `1`                      | yes               | no                         |
| `--endpoints`   | `shared`, `separate`            | `shared`                 | no                | yes                        |

`--store fs` is for transfers that do not fit in memory, such as the 100 GB file of scenario S1 in
`docs/performance.md`. `serve` writes the generated data to `<dir>/data-<seed>-<size>.bin` once, in
constant memory, and reuses the file on later runs if its size matches. It then imports the file into
an `iroh-blobs` file store in `<dir>/store` and serves from that. The import uses `TryReference`, which
keeps the blob as a reference to the data file and does not copy it. `serve` exits with an error if
the store copied the file anyway, since the copy would be another 100 GB. The import is tagged
`probe-<seed>-<size>`, so a later run with the same seed and size finds the blob already imported and
skips the hashing pass. All of this finishes before `serve` prints the ticket, so none of it is in
what `fetch` measures. `--store fs` needs `--mode blobs`, because raw mode has no store.

`--endpoints` decides how many UDP sockets carry a transfer. By default every connection of a `fetch`
shares one endpoint, so one socket and one socket task, and `serve` has one too. ADR 0016 spreads a
transfer over several connections to use more cores, and if the shared socket is the bottleneck that
cannot help. `serve --endpoints n` binds `n` endpoints, each with its own router serving the same
data, and the ticket carries all `n` addresses. `fetch --endpoints separate` binds one endpoint per
connection, and connection `i` dials address `i % n`. With `shared`, connection `i` still dials
address `i % n`, so the server side is the same in both and only the receiving socket changes.

`--timeout` covers the transfer, not the handshakes. When it expires, `fetch` still prints its result
line, with `"outcome": "timeout"` and everything measured up to that point, then exits non-zero. A
stall is a result: `progress` shows when the bytes stopped growing, and `connection_stats` shows what
`noq` had counted by then.

`tuned` sets the stream and connection receive windows and the send window to 75 000 000 bytes, the
bandwidth-delay product of 10 Gbit/s at 60 ms. The values in force are in every result.

## Result

One line. This one is wrapped for reading, and comes from a same-machine run on Windows (`serve` and
`fetch` on one host, 1 GiB, release build). It shows the shape of the output. It says nothing about any
link. On Windows `cpu_user_s` and `cpu_sys_s` are `null` and no GRO batches are counted.

```
{"outcome":"ok","mode":"blobs","size_bytes":1073741824,"received_bytes":1073741824,
 "connections":1,"serve_endpoints":1,"fetch_endpoints":1,"serve_store":"mem",
 "congestion":"bbr3",
 "window":{"mode":"default","stream_receive_bytes":1250000,
           "receive_bytes":4611686018427387903,"send_bytes":10000000},
 "sink":"discard","relay":"disabled","elapsed_s":4.1347848,"throughput_gbit_s":2.0774804512196137,
 "progress":[206503936,480493568,756236288,1037254656],
 "connection_stats":[{"received_bytes":1073741824,"sent_datagrams":17813,
   "received_datagrams":761323,"lost_packets":86,"lost_bytes":1353,
   "paths":[{"selected":true,"relay":false,"rtt_ms":0.069566,"cwnd_bytes":5808,
             "congestion_events":26,"spurious_congestion_events":0,"lost_packets":86,
             "lost_bytes":1353,"sent_datagrams":17798,"received_datagrams":761317,"mtu":1452}]}],
 "path":"direct","rtt_ms":0.069566,"cpu_user_s":null,"cpu_sys_s":null,
 "key_exchange_group":"X25519MLKEM768","gso_segments":null,"gro_segments":null,
 "recv_datagrams":761319,"recv_gro_batches":0,"iroh_version":"1.3.0","iroh_blobs_version":"0.103.1",
 "verified_hash":"7b343701fd41de622e1208b9490efd7f5a54eee0fe68c052e522500f55aa0fbc"}
```

`outcome` is `ok` or `timeout`. `serve_endpoints` is the number of addresses in the ticket and
`fetch_endpoints` the number of endpoints `fetch` bound, 1 for `shared` and the connection count for
`separate`. `serve_store` is `mem` or `fs` in blobs mode and `null` in raw mode. `progress` has one
entry per whole second of the transfer: the total payload bytes received so far. It is read from a
one-second timer, and the transfer never waits for it. `connection_stats` has one entry per
connection:

- `received_bytes` is the payload the probe counted on that connection. With `--sink store` it
  follows the progress items the store reports.
- `sent_datagrams`, `received_datagrams`, `lost_packets` and `lost_bytes` are `noq`'s totals over every
  path the connection has used. `noq` does not count QUIC packets, so datagrams are the nearest it
  exposes.
- `paths` lists the paths open at the end, each with its RTT, congestion window, congestion events,
  losses and MTU.

The statistics are the receiver's. QUIC declares a packet lost at the end that sent it, so
`lost_packets` here counts the receiver's own packets, which are mostly acknowledgements, and
`cwnd_bytes` is the receiver's window (5808 bytes in the example above, far below what 1 GiB in 4 s
needs). The congestion state that limits a download lives on the sender, and `serve` does not report it.

After a timeout `received_bytes` is what arrived, `path` is `null` if iroh had not marked a path
selected, and `verified_hash` is the root the bytes would have been checked against. Otherwise `fetch`
exits non-zero, without a result line, if the received byte count differs from `size_bytes`, if a blob
does not verify against the root, or if any connection negotiated a key exchange group other than
`X25519MLKEM768`.
