# sluice-probe

**Measures:** how fast `iroh` and `iroh-blobs` move bytes between two machines, with one connection
or several, under Cubic, BBRv3 or NewReno, with default or tuned windows. It is the measurement from
ADR 0010 and the tool the bench harness drives. It is for the people who tune the transport, not for
users, and its output is for machines: one ticket line from `serve`, one JSON line from `fetch`, one
JSON line from `serve` for every connection that closes, and everything else on stderr.

**Must never:**

- be a dependency of a product crate
- choose a relay silently. `--relay` defaults to `disabled`, and a ticket made without one carries
  only direct addresses (ADR 0011).
- turn on port mapping or mDNS by itself. `--port-mapping` defaults to `off` and address lookup is
  empty, so a result shows what hole punching did and not what the router or the LAN offered.
- report a number it did not read. A field iroh does not expose is `null`.
- keep going after a handshake that did not negotiate `X25519MLKEM768` (ADR 0012). `fetch` exits
  non-zero instead.

**Notes:**

- It needs Rust 1.91, because `iroh` 1.4.1 and `iroh-blobs` 0.103.1 do. The msrv job leaves it out.
- `serve` holds the test data in memory in `blobs` mode unless `--store fs` is given, so `--size`
  has to fit in RAM by default. Raw mode keeps only a 64 MiB block in memory.
- `raw` mode repeats a 64 MiB block of seeded data to fill each stream. QUIC neither compresses nor
  deduplicates, so it costs the same on the wire as fresh data, and the sender never waits on the
  generator.
- `elapsed_s` runs from the first request to the last verified byte. Handshakes are not in it.
- `gso_segments` and `gro_segments` are always `null`. `noq` reports the segment limits per socket
  and iroh does not expose that socket. What iroh counts is `recv_datagrams` and `recv_gro_batches`,
  which show whether GRO ran, not how many segments it coalesced.
- `cpu_user_s` and `cpu_sys_s` come from `getrusage` on Linux and macOS and are `null` on Windows.
- Numbers from one machine are not results. The bench harness runs the probe on a pair of hosts.

**Tests:** `tests/loopback.rs` runs `serve` and `fetch` as processes over both modes, three
congestion controllers and one or four connections, and checks the JSON, the byte count and the key
exchange group. It also covers separate endpoints, a file store that keeps a reference rather than a
copy, and a timeout. `tests/sender.rs` checks the `connection_closed` lines, the CPU fields and the
controller series. `tests/rendezvous.rs` runs an `iroh-relay` server with its relay half off and
checks address discovery, the ticket, a dead address and a silent server. With
`PROBE_TEST_LOG_PATHS` set, the tests print the path readings they used, which is how a loop of the
suite counts fallbacks; run with `--success-output immediate` to see them.

## Run

On the machine that holds the data:

```
probe serve --size 10GiB --mode blobs --congestion bbr3 --window tuned
```

It prints one ticket line, then serves until killed. On the other machine:

```
probe fetch <ticket> --mode blobs --connections 4 --congestion bbr3 --window tuned
```

The ticket carries the mode, size, seed and addresses, so `fetch` needs none of them again. Both
ends take `--congestion` and `--window`, because the sender's controller and the receiver's windows
both limit throughput.

| Option             | Values                          | Default                  | `serve`           | `fetch`                    |
| ------------------ | ------------------------------- | ------------------------ | ----------------- | -------------------------- |
| `--mode`           | `raw`, `blobs`                  | `blobs`                  | yes               | yes, must match the ticket |
| `--congestion`     | `bbr3`, `cubic`, `newreno`      | `bbr3`                   | yes               | yes                        |
| `--window`         | `default`, `tuned`              | `default`                | yes               | yes                        |
| `--relay`          | `disabled` or a relay URL       | `disabled`               | yes               | yes                        |
| `--rendezvous`     | `https://host[:udp-port]`       | none                     | yes               | yes                        |
| `--rendezvous-ca`  | a PEM file                      | none                     | yes               | yes                        |
| `--port-mapping`   | `on`, `off`                     | `off`                    | yes               | yes                        |
| `--size`           | bytes, with `KiB`, `MiB`, `GiB` | `1GiB`                   | yes               | no                         |
| `--seed`           | `u64`                           | `0`                      | yes               | no                         |
| `--connections`    | 1 to 16                         | `1`                      | no                | yes                        |
| `--sink`           | `discard`, `store`              | `discard`                | no                | yes                        |
| `--dir`            | directory                       | `fetch`: a temporary one | `--store fs` only | `--sink store` only        |
| `--store`          | `mem`, `fs`                     | `mem`                    | yes               | no                         |
| `--timeout`        | seconds, 1 or more              | `600`                    | no                | yes                        |
| `--progress-lines` | flag                            | off                      | no                | yes                        |
| `--endpoints`      | 1 to 16                         | `1`                      | yes               | no                         |
| `--endpoints`      | `shared`, `separate`            | `shared`                 | no                | yes                        |

`--store fs` is for transfers that do not fit in memory, such as the 100 GB file of scenario S1 in
`docs/performance.md`. `serve` writes the generated data to `<dir>/data-<seed>-<size>.bin` once, in
constant memory, and reuses the file on later runs if its size matches. It imports the file into an
`iroh-blobs` file store in `<dir>/store` with `TryReference`, which keeps the blob as a reference to
the data file, and exits with an error if the store copied it anyway. The import is tagged
`probe-<seed>-<size>`, so a later run with the same seed and size skips the hashing pass. All of
this finishes before `serve` prints the ticket. `--store fs` needs `--mode blobs`.

`--endpoints` decides how many UDP sockets carry a transfer. By default every connection of a
`fetch` shares one endpoint, so one socket and one socket task, and `serve` has one too. `serve
--endpoints n` binds `n` endpoints, each with its own router serving the same data, and the ticket
carries all `n` addresses. `fetch --endpoints separate` binds one endpoint per connection, and
connection `i` dials address `i % n`; with `shared` it dials the same. ADR 0016 spreads a transfer
over several connections, and if the shared socket is the bottleneck that cannot help.

`--timeout` covers the transfer, not the handshakes. When it expires, `fetch` still prints its
result line, with `"outcome": "timeout"` and everything measured so far, then exits non-zero. A
stall is a result: `progress` shows when the bytes stopped growing, and `connection_stats` shows
what `noq` had counted by then.

`--progress-lines` makes `fetch` print one `{"event":"progress","t":<s>,"bytes":<n>}` line on stdout
each second, before the result line, which stays last. `t` counts seconds from the first request and
`bytes` is the payload received so far. The bench harness uses the lines to stop a transfer that has
collapsed.

`tuned` sets the stream and connection receive windows and the send window to 75,000,000 bytes, the
bandwidth-delay product of 10 Gbit/s at 60 ms. The values in force are in every result.

`--rendezvous` points both ends at a rendezvous server, which is `iroh-relay` with its relay half
off (ADR 0013, [the image](../../docker/rendezvous/README.md)). It goes into iroh's relay map with
QUIC address discovery on its UDP port, the URL's port or 7842. The endpoint asks the server for its
public address and uses it for nothing else, so a connection is direct or it does not exist. It
cannot be combined with `--relay`.

- Both ends wait up to 10 s for the server's answer before anything else. `serve` does it before
  printing the ticket, so the ticket carries the public address, and `fetch` does it before
  dialling. If no answer comes, the probe goes on without one and records that.
- The answer reaches the probe before iroh has copied it into the endpoint's own address. After the
  wait, `serve` waits again, inside the same 10 s, until the endpoint's address holds every address
  the server reported. A missing one is added to the ticket and listed under `added_from_report`, so
  a run where iroh never merged it can be told apart from one where it did. Without this, `fetch`
  could dial private addresses only and record a `no_path` that is not real.
- `endpoint.online()` is not used. It waits for a relay connection, and with no relay half it never
  returns.
- iroh adds the URL as its home relay anyway, so the ticket carries it and the endpoint keeps
  retrying a relay connection that is refused, with a backoff that reaches 16 s before jitter.
  Neither the ticket nor the connection waits for it.
- `--rendezvous-ca` adds the certificates in a PEM file to the roots trusted for the server, for a
  self-signed one in tests. The certificate must say `CA:FALSE`, or the handshake fails with
  `CaUsedAsEndEntity`.
- `--port-mapping` is `off` here and on in the product. With `on` the endpoint asks the router for a
  mapping through UPnP, PCP and NAT-PMP. iroh's `portmapper` feature is enabled in the workspace for
  it, because without the feature the option would read as on and nothing would run.
- mDNS is off in both modes: the probe builds its endpoint with no address lookup.

The ticket line then carries a `discovery` object: `took_s`, `complete`, `learned`, the public
addresses the server reported, and `added_from_report`, the ones `serve` had to add by hand. The
`fetch` result has the same object for its own side, `rendezvous` (the URL), and `connect_s`, from
the start of the dial to the connections being up. Each path in `connection_stats` has a
`remote_addr` on both ends, the address the other end was reached at.

## Result

One line. This one is wrapped for reading, and comes from a same-machine run on Windows (`serve` and
`fetch` on one host, 1 GiB, release build). It shows the shape of the output. It says nothing about
any link. On Windows `cpu_user_s` and `cpu_sys_s` are `null` and no GRO batches are counted.

```
{"outcome":"ok","mode":"blobs","size_bytes":1073741824,"received_bytes":1073741824,
 "connections":1,"serve_endpoints":1,"fetch_endpoints":1,"serve_store":"mem",
 "congestion":"bbr3",
 "window":{"mode":"default","stream_receive_bytes":1250000,
           "receive_bytes":4611686018427387903,"send_bytes":10000000},
 "sink":"discard","relay":"disabled","elapsed_s":4.264907,"throughput_gbit_s":2.014096577486918,
 "progress":[211091456,456114176,730218496,1002274816],
 "connection_stats":[{"key":"f7812bb23acdbf1e",
   "remote_id":"76efe258516f89ff079a57de3d202685850b7b4549818b349efca93481930bf4",
   "received_bytes":1073741824,"sent_datagrams":16560,
   "received_datagrams":761308,"lost_packets":80,"lost_bytes":1340,
   "paths":[{"selected":true,"relay":false,"rtt_ms":0.07114300000000001,"cwnd_bytes":5808,
             "congestion_events":24,"spurious_congestion_events":0,"lost_packets":80,
             "lost_bytes":1340,"sent_datagrams":16545,"received_datagrams":761303,"mtu":1452,
             "controller":{"pacing_rate_bytes_s":102601,"send_quantum_bytes":2904,
                           "ssthresh_bytes":null,"bytes_in_flight":0,
                           "bbr_state":"ProbeBw(Cruise)","bbr_max_bw_bytes_s":110786.0,
                           "bbr_app_limited":false}}]}],
 "path":"direct","rtt_ms":0.07114300000000001,"cpu_user_s":null,"cpu_sys_s":null,
 "key_exchange_group":"X25519MLKEM768","gso_segments":null,"gro_segments":null,
 "recv_datagrams":761309,"recv_gro_batches":0,"iroh_version":"1.4.1","iroh_blobs_version":"0.103.1",
 "verified_hash":"7b343701fd41de622e1208b9490efd7f5a54eee0fe68c052e522500f55aa0fbc"}
```

`outcome` is `ok`, `timeout` or `no_path`. `no_path` means no connection came up, because the dial
failed or `--timeout` passed first. Without a relay that is the answer to whether the two ends could
reach each other, so `fetch` prints a result line with `received_bytes` 0, `connect_s` and
`key_exchange_group` `null`, and the reason in `error`, then exits non-zero. `serve_endpoints` is
the number of addresses in the ticket and `fetch_endpoints` the number of endpoints `fetch` bound.
`serve_store` is `mem` or `fs` in blobs mode and `null` in raw mode. `progress` has one entry per
whole second of the transfer: the total payload bytes received so far, read from a one-second timer.
`connection_stats` has one entry per connection:

- `key` and `remote_id` identify the connection. See [Sender statistics](#sender-statistics).
- `received_bytes` is the payload the probe counted on that connection. With `--sink store` it
  follows the progress items the store reports.
- `sent_datagrams`, `received_datagrams`, `lost_packets` and `lost_bytes` are `noq`'s totals over
  every path the connection has used. `noq` does not count QUIC packets, so datagrams are the
  nearest it exposes.
- `paths` lists the paths open at the end, each with its RTT, congestion window, congestion events,
  losses, MTU and `controller`. `paths_at_s` is the time it was read and `paths_source` what read
  it. See [Paths read late](#paths-read-late). `unlisted_path_ids` is in [Paths iroh never
  listed](#paths-iroh-never-listed).

These statistics are the receiver's. QUIC declares a packet lost at the end that sent it, so
`lost_packets` counts the receiver's own packets, mostly acknowledgements, and `cwnd_bytes` is the
receiver's window (5808 bytes above, far below what 1 GiB in 4 s needs). The congestion state that
limits a download lives on the sender, and `serve` reports it.

After a timeout `received_bytes` is what arrived and `verified_hash` is the root the bytes would
have been checked against. `path` is `relay` if any connection's chosen path is a relay and `direct`
otherwise, and `rtt_ms` is the RTT of the first connection's chosen path. The chosen path is the one
a sample reads: the one iroh marked selected, the only path with a controller, or, with several and
none selected, the one with the most datagrams sent plus received, a tie to the lower path id. Both
fields are `null` only if a connection's list has no such path while another connection has a relay
open. `fetch` exits non-zero, without a result line, if the received byte count differs from
`size_bytes`, if a blob does not verify against the root, or if any connection negotiated another
key exchange group.

## Sender statistics

After the ticket, `serve` prints one line for every connection that closes, and flushes it. It has
`"event": "connection_closed"`, a `close_reason`, and the fields of a `connection_stats` entry, read
on the sending side after the close, when the counters are final. `received_bytes` is `null`,
because `serve` does not count the payload it writes. After ctrl-c `serve` waits up to 5 seconds for
the lines of the connections the shutdown closes.

The line also has `cpu_user_s` and `cpu_sys_s`: the CPU time the `serve` process has used since it
printed the ticket, read when that connection closed. The process is shared, so with several
connections the line of the last one to close holds the whole transfer. Both are `null` on Windows.
Here is the line that matches the `fetch` example, wrapped:

```
{"event":"connection_closed","close_reason":"closed by peer: done (code 0)",
 "key":"f7812bb23acdbf1e",
 "remote_id":"429b01e5102a4519a4339e0d285cd471acf4b11fdb52423182f64660860af89b",
 "received_bytes":null,"sent_datagrams":761314,"received_datagrams":16480,
 "lost_packets":0,"lost_bytes":0,
 "paths":[{"selected":true,"relay":false,"rtt_ms":0.098584,"cwnd_bytes":587870938,
           "congestion_events":0,"spurious_congestion_events":0,"lost_packets":0,
           "lost_bytes":0,"sent_datagrams":761309,"received_datagrams":16465,"mtu":1452,
           "controller":{"pacing_rate_bytes_s":270723044,"send_quantum_bytes":65536,
                         "ssthresh_bytes":null,"bytes_in_flight":0,
                         "bbr_state":"ProbeBw(Cruise)","bbr_max_bw_bytes_s":273457620.0,
                         "bbr_app_limited":true}}]}
```

Join a `serve` line to its `fetch` entry on `key`. Both ends derive it from the TLS session
(`export_keying_material`, 8 bytes in hex), so nothing extra crosses the wire, no two connections
share one, and the two ends compute the same value. `remote_id` is not enough, because with
`--endpoints shared` every connection of a `fetch` has the same one. The index of an entry in
`connection_stats` is the connection's index in the `fetch`, and the `serve` line carries no index
because `serve` accepts connections in whatever order they arrive.

Every path has a `controller` object, read from the controller of that path on that end:

| Field | Source | Controllers |
|---|---|---|
| `cwnd_bytes` | `PathStats::cwnd`, from `Path::stats()` | all |
| `pacing_rate_bytes_s`, `send_quantum_bytes` | `Controller::metrics()` on `Connection::congestion_state(path_id)` | BBR |
| `ssthresh_bytes` | the same call | Cubic, NewReno |
| `bytes_in_flight`, `bbr_state`, `bbr_max_bw_bytes_s`, `bbr_app_limited` | the derived `Debug` output of the `Bbr3` controller, top-level fields only | BBR |

`Controller` has no in-flight count, and `Bbr3` keeps its state and bandwidth estimate in private
fields with no accessors, so for BBR the last row parses the `Debug` text. A unit test builds a
`Bbr3` and fails if a `noq` upgrade renames a field. A field that is missing or does not parse is
`null`, and `ssthresh_bytes` is `null` while `noq` reports `u64::MAX` for "not set".

The controller fields are read once, at close, where the connection has been idle for a moment, not
in the middle of the transfer. The counters (`congestion_events`, `lost_packets`, `sent_datagrams`)
are totals over the whole connection.

### Controller series

Both ends also sample each connection's controller once a second and add a `controller_series` array
to the connection entry (`fetch`) or the `connection_closed` line (`serve`). Each sample has
`elapsed_s`, `open_paths`, `stale_paths`, `path`, `cwnd_bytes`, `pacing_rate_bytes_s`, `bbr_state`,
`bbr_max_bw_bytes_s` and `bbr_app_limited`. The last five follow the table above. A transfer shorter
than a second has an empty series.

```
"controller_series":[
 {"elapsed_s":1.0,"open_paths":1,"stale_paths":0,"path":"selected","cwnd_bytes":66988,
  "pacing_rate_bytes_s":333780186,"bbr_state":"Startup",
  "bbr_max_bw_bytes_s":216543040.0,"bbr_app_limited":false},
 {"elapsed_s":2.0,"open_paths":1,"stale_paths":0,"path":"selected","cwnd_bytes":66988,
  "pacing_rate_bytes_s":333780186,"bbr_state":"ProbeBw(Cruise)",
  "bbr_max_bw_bytes_s":337151703.0,"bbr_app_limited":true}]
```

- `open_paths` is how many paths iroh listed when the sample was read, and `stale_paths` how many of
  those had no controller in `noq`. `path` says which path was read: `selected` for the one iroh
  marked, `only` for a single path with a controller, `busiest` when several have one and none is
  selected, or `null` when none has.
- On `fetch` the samples are taken on the tick that appends to `progress`, so entry `i` pairs with
  `progress[i]` on every connection. On `serve` the clock starts when the connection is accepted, so
  compare the two by position and expect a small offset, not zero.
- A sample reads one path among those with a controller: the selected one, the only one, or the one
  that moved the most datagrams, a tie to the lower path id. iroh can hold two paths for a whole
  transfer without marking one, and a sample that said nothing then would say nothing for the run.
  The busiest path is where the traffic is, so `path` says `busiest`.
- A path is stale when iroh lists it and `noq` holds no controller. A stale selected path is skipped
  for another that has one. When every listed path is stale the sample is all `null`, which is the
  truth: no path had a controller to read.
- Stale paths come from iroh's list lagging `noq`'s: iroh removes a path only when it handles
  `noq`'s abandoned event, and when it falls behind that channel it skips events and does not
  resync. A skipped abandon leaves a dead path listed for a while.
- Reading a BBR controller clones it and formats its state, which is why there is one sample a
  second and not more.

### Paths read late

`Connection::paths()` leaves out closed paths. When the peer closes first, the list is `[]` by the
time this end reads it, which gave `"paths":[]` in 3 of 40 full-suite loopback runs: 2 of 20 on
Windows and 1 of 20 in WSL.

So each end keeps the newest list that had a path in it, from two sources: the once-a-second
controller sample, and `Connection::paths_stream()`, which yields the current list when first polled
and a fresh one whenever a path opens, closes or is selected. The stream covers a transfer under a
second, which has no sample, and a connection whose first path is not listed yet when accepted. Both
are polled inside loops that already wait, so there is no extra task and no wakeup while no path
changes.

`paths_source` says which reading `paths` is, and `paths_at_s` when it was read:

| `paths_source` | `paths` | `paths_at_s` |
|---|---|---|
| `live` | the list at report time, which had a path | the report time: `elapsed_s` on `fetch`, seconds since accept on `serve` |
| `stream` | the list read when the stream reported a change | when the change was received |
| `sample` | the list read with the latest controller sample | the sample's time |
| `none` | `[]`, because no reading ever had a path | `null` |

A `stream` or `sample` list is older than the connection's counters. A sample is up to a second old.
A stream list can be older still: it is from the last path change, which for a short transfer is
before the data flowed, so its RTT, window and datagram counts sit near zero and no path may be
marked `selected` yet. When both have read a list the later one is kept. The probe does not make a
path up when there is none.

### Paths iroh never listed

`unlisted_path_ids` names the ids, 0 to 63, that `noq` holds a controller for and `paths` does not
include. It is read for every report with `Connection::congestion_state`, and compared with the ids
of the listed paths.

Two shapes are the failure, both from iroh not listing a path `noq` opened. In the first, `paths` is
`[]` and `paths_source` is `none`. In the second, every listed path has `controller` `null`, because
`noq` dropped it, and the path in use is missing. Most non-empty fields are benign: `noq` holds a
controller from the moment it adds a path, iroh lists the path only once `noq` has validated it and
sent `Established` (10 to 120 ms in the loopback runs read, under the suite's load), and `noq` keeps
an abandoned path's controller until it has drained.

The probe makes up nothing for those ids: no address, statistics, RTT or controller reading. The
field is a lower bound, and an id above 63 is not reported.
