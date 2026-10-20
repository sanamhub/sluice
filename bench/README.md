# sluice-bench

**Measures:** the M0 transport numbers from ADR 0010 on a pair of real hosts. For each scenario it
drives [`probe`](probe/README.md) on both ends, shapes both hosts' links with netem, measures what
the path and the disks can do (`iperf3`, `fio`) and what a parallel-TCP copy reaches (`mscp`), and
writes the results anyone can rerun. The binary is `bench`, the package `sluice-bench`. It is for the
people who run M0, not for users.

**Must never:**

- install anything on a host. `bench check` reports what is missing and stops there.
- leave netem on an interface. It removes the shaping after every scenario, on error and on ctrl-c.
- use `sudo` for anything but `sudo -n tc`, or ask for or store a password.
- publish a number it could not check. A Linux run whose receiver counted no GRO batches fails
  instead of reporting a throughput that came from a silent fallback (`docs/performance.md`, "How it
  is measured").
- delete a file it did not create. It keeps its files in each host's `data_dir` and removes only
  those.

**Notes:**

- It runs commands through the system `ssh` binary, or directly for `local`. There is no SSH crate.
  ssh runs with `BatchMode`, so keys or an agent have to work already.
- A remote host needs a POSIX shell. A Windows host works as `local` only.
- netem is Linux only, and needs two hosts. A scenario with netem is skipped, with the reason in
  `summary.md`, if either host is not Linux or has no `tc`, or if both ends are the same host. S1 has
  no netem and runs anywhere.
- netem shapes egress, so each host shapes what it sends. The sender carries half of `delay_ms`, the
  `loss_percent`, the `rate` and the `limit_packets`, so they act on the data. The receiver carries
  the other half of `delay_ms` and half of `jitter_ms`, so they act on the acknowledgements. The round
  trip gets the full delay. Loss on the acknowledgements is not shaped, and a rate applies to the
  data direction only. `summary.md` shows what `tc qdisc show` read back on each side.
- The probe moves one blob or one raw stream. The other two workloads in `docs/performance.md` (100k
  small files, a mixed tree) need the engine and are not here.
- `mscp` copies over ssh, so it needs a receiver that is not `local`. With both ends local it is
  recorded as not run, with that reason.
- The harness measures the stock `noq-proto`. BBRv3 with default windows collapses on 1.3.0
  ([noq#852](https://github.com/n0-computer/noq/issues/852)), and the `bbr3`/`default` rows show it
  until a noq release fixes it. Do not add a `[patch]` entry to make them look better.
- A collapsed run would use the whole timeout, five times per configuration, so `fetch` is stopped
  early. `fetch --progress-lines` prints the running total each second, and the harness stops the run
  when `stall_s` seconds in a row (a scenario key, 30 by default) each moved less than 1% of the
  busiest second so far. The run is counted as `stalled`, a result like a timeout and not a failure.
  `serve` is then interrupted, so it still prints a line for the connection. Where it cannot be
  interrupted (a local Windows process), the harness waits for the connection's idle timeout instead.
  A run shorter than `stall_s` is never stopped.

**Tests:** unit tests cover the summary maths (median, spread, slow runs, percentage of the lower
ceiling), the parsing of `scenarios.toml` and the hosts file, the checks on `tc qdisc show`, how netem
is split between the two hosts, that a side already shaped is undone when the other fails, and the
stall detector. On Unix one more test runs a shell script that behaves like a collapsed probe and
checks that the run is stopped early and still has its `serve` line.
`tests/smoke.rs` runs `bench smoke` as one integration test under `cargo nextest run` on all three
operating systems. It uses the probe only, because CI runners may lack `iperf3`, `fio` and `mscp`, and
the results record them as missing.

## Prepare two hosts

Both hosts need:

- the `probe` binary, built with `cargo build --release -p sluice-probe` (Rust 1.91 or newer). It can
  sit anywhere. The hosts file names its path.
- a directory the benchmark user can write, with room for the largest workload. S1 moves 100 GiB, and
  the sender keeps one file of that size.
- `iperf3` and `fio`, for the ceilings. Without them the run still works, but a row has no ceiling
  to be a percentage of, and its `% of lower ceiling` column reads `n/a`.
- `mscp` on both, if you want the parallel-TCP bar. It copies over ssh, so the sender must reach the
  receiver by ssh without a password.

Both hosts also need `tc` and a user that may run it without a password, because each one shapes its
own interface. A sudoers line such as this one, written with `visudo`, is enough, and the path is
whatever `command -v tc` prints:

```
bench ALL=(root) NOPASSWD: /usr/sbin/tc
```

The machine that runs `bench` needs ssh access to both hosts. It can be one of them.

Copy [`hosts.example.toml`](hosts.example.toml) outside the repository, because the real file names
real machines, and fill it in:

```
cp bench/hosts.example.toml ~/m0-hosts.toml
```

`host` is `local` or an ssh destination. `interface` is the NIC netem shapes, and both hosts need one
for the scenarios with netem. `address` is where the other end reaches this host, for `iperf3` and
`mscp`, if it differs from the host name.

## Check

```
cargo build --release -p sluice-probe -p sluice-bench
target/release/bench check --hosts ~/m0-hosts.toml
```

It lists, for each host, its operating system, which of `probe`, `iperf3`, `fio`, `tc` and `mscp` it
can run (with the version where the tool prints one), whether the data directory is writable, whether
the interface exists, and whether `sudo -n tc` works without a password. It changes nothing. It exits
non-zero if a host is unreachable, the probe is missing or the data directory is not writable. A
missing interface or `sudo -n tc` is reported and does not fail the check, because S1 does not need
them.

## Run M0

```
target/release/bench run --hosts ~/m0-hosts.toml --label lan-10g
```

That runs S1, S3 and S6 from [`scenarios.toml`](scenarios.toml), five runs per configuration. One
scenario, or fewer runs, for a first try:

```
target/release/bench run --hosts ~/m0-hosts.toml --label try --scenario S3 --runs 2
```

Each scenario, in order:

1. Applies netem to both interfaces, reads each back with `tc qdisc show`, and fails if either does
   not match what was asked.
2. Measures `iperf3` with one stream and with 8, and `fio` sequential read on the sender and write on
   the receiver, for the runs that touch disk.
3. Runs `mscp` if both hosts have it.
4. For each probe configuration, five times: starts `serve`, reads the ticket, runs `fetch` with the
   scenario's timeout and `--progress-lines`, joins the `fetch` line and the `serve` lines on `key`,
   and stops `serve`. A process that outlives the timeout by 30 s is killed.
5. Removes netem from both.

S1 at 100 GiB takes hours across its 7 configurations. S3 and S6 are 10 GiB and 50 GiB. For a first
pass, `--size 20GiB` replaces every workload's size for that run. The summary and `raw.jsonl` record
the override and the sizes it replaced, so a result is never mistaken for the published scenario. The
committed sizes stay as they are. `--size` takes `KiB`, `MiB`, `GiB` or `TiB`, and refuses anything
else, such as `20GB`, before anything runs.

Every probe configuration is one row of the matrix: mode, congestion, window, connections, endpoints
and, for `blobs`, the sink and store. The three rows the published numbers must show are
`bbr3`/`default`, `bbr3`/`tuned` and `cubic`/`default`.

## Results

`bench run` writes `bench/results/<UTC date>-<label>/`, or the place `--out-dir` names. It refuses to
write into a directory that exists.

- `raw.jsonl`: every probe, `iperf3`, `fio` and `mscp` result, one JSON object per line, with the
  scenario, configuration and run index. Probe lines carry the `fetch` line, the `serve` lines and the
  two joined on `key`, so the controller series of both ends sit side by side.
- `env.json`: CPU, NIC, kernel, MTU, file system and the exact command lines, for both hosts, as far
  as they can be read without root. A field it could not read is `null`.
- `summary.md`: per scenario and configuration, the median and spread of the runs, the result as a
  percentage of the lower ceiling, the number of timeouts, of stalled runs and of slow runs (any second
  below 10% of the run's peak second), in plain tables. Two more columns catch what that rule
  misses. The worst dip is the lowest second of a run over the median second of the same run,
  leaving out the first and last second, as the lowest across the runs. The outliers are finished
  runs that took more than 1.5 times as long as the median finished run.

The files are written even if a scenario fails or the run is interrupted, so what was measured is
kept.

## Smoke

```
cargo build -p sluice-probe -p sluice-bench
target/debug/bench smoke
```

Both ends on this machine, no netem, 64 MiB, two runs per configuration, the probe only. It finishes
in well under a minute and is the repeatable reproduction CI runs. A smoke result is a check that the
harness works, not a measurement.
