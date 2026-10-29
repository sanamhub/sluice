# sluice-cli

The `sluice` command is the program you run. It sends and receives files, prints this machine's id,
checks your setup with `doctor`, and prints shell completions. It owns the subcommands, human and
`--json` output, man pages, progress display and `sluice mcp` (an MCP server over stdio, using
`rmcp`).

There is no release yet: build it with `cargo build -p sluice-cli`. A transfer between two folders
on one machine, or two machines that can reach each other, works today. Speeds have not been
measured on a real network, only on loopback.

**Must never:**

- open the store while a daemon holds its lock. It detects the lock and sends the request over the
  control socket instead.
- print a ticket to a log
- use colour when `NO_COLOR` is set

**Notes:** the cold start budget is 30 ms. A command does no config parsing, store opening or
network probing unless it needs them. Every transfer line shows whether it is direct or relayed.

**Tests:** `insta` snapshots of the help text, exit-code tests for scripts, and `cargo xtask
cold-start` for the cold start budget. That command builds `sluice` in release, then times `sluice
id` and `sluice --version`, 30 runs each after 3 warm-ups, and fails if a median is over 30 ms. CI
runs it on Linux and Windows.

## What scripts can rely on

The exit codes, the JSON shapes and the rules below are an interface. Scripts and agents depend on
them, so changing one is a breaking change and is tested. The wording of a message is not part of
it and can change in any release.

**Rules**

1. The output format never depends on whether stdout is a terminal. It is text unless `--json` is
   given, so `sluice id | clip` gets the bare id. A terminal decides only colour, progress and
   whether a prompt may be shown.
2. `--json` is global and works before or after the subcommand. With it, stdout holds only JSON, one
   object per line, with keys in `snake_case`. A failure then writes one JSON line to stderr and
   leaves stdout empty. The exit code is the same with or without `--json`. Without it, a failure is
   `sluice: <message>` on stderr.
3. `--color auto|always|never` is global and defaults to `auto`, which means colour only on a stream
   that is a terminal. `NO_COLOR` set to any non-empty value means `never` and wins over
   `--color always`. Help and usage errors follow the same choice.
4. A panic is a bug in sluice. It exits with code 70 and prints `sluice: internal error: <message>`
   to stderr, or the JSON error with kind `bug`, and not Rust's code 101 and backtrace. Setting
   `RUST_BACKTRACE` still shows where it happened.
5. A closed stdout is not an error. When the reader goes away, as in `sluice id | head -c 0`, sluice
   stops quietly with exit code 0. Any other failure to write to stdout is `Storage`, exit code 5.

**Exit codes**

| Code | Meaning |
|---|---|
| 0 | Success |
| 2 | A bad command line, as `clap` reports it. No JSON error is written for it |
| 3 | `Network`: a peer or relay could not be reached, or a connection failed mid-transfer. Retrying can help |
| 4 | `Verify`: received bytes did not match the BLAKE3 root. The bytes are discarded |
| 5 | `Storage`: the store or filesystem failed: full disk, permission or quota |
| 6 | `Policy`: sluice refused by rule, such as a home with a newer layout, a home or a key another user owns, a key that other users can read or that has more than one name, or `--rotate` with nobody to ask |
| 70 | `Bug`: an internal invariant broke, or sluice panicked. A defect in sluice, never in the environment |
| 130 | `Cancelled`: the caller cancelled, for example by answering no to a prompt |

**JSON**

`sluice id --json` prints one object:

```
{"endpoint_id":"<64 lower-case hex characters>"}
```

A failure with `--json` is one line on stderr. `kind` is the variant in lower case: `network`,
`verify`, `storage`, `policy`, `cancelled` or `bug`. `message` is for people.

```
{"error":{"kind":"policy","message":"the store has layout version 2 and this sluice reads up to version 1. It does not write to a newer layout"}}
```

`sluice id --rotate` asks before it replaces the identity, and only on a terminal. With no terminal,
or with `--json`, it refuses with code 6 unless `--yes` is given.

**`sluice send`**

`send PATH...` takes one file, a folder, or several paths of either, imports them where they are and
prints one ticket. A folder arrives inside a folder of its own name; several paths keep their own
names. `send -` reads standard input to the end and sends it as one raw blob, because its root is
known only then. `--expires 1h|7d|never` sets the ticket's life (24 hours by default), `--keep`
serves until Ctrl+C instead of after one complete download, `--port` names the UDP port, and
`--relay` or `--n0-relays` choose a relay. `--limit-rate 50M` caps what the whole command offers per
second. A receive that took only part of a folder does not count as the one download: the send
keeps serving after it, so a sender that expects receivers taking a part uses `--keep`, and ends it
with Ctrl+C.

`send --live FILE` follows one regular file that is still being written: each segment is sealed and
signed as it fills, and a receiver can fetch it while the writer keeps going. The stream closes
after the file has not grown for two seconds; `--close-after SECONDS` changes that. A live ticket
carries a stream id and the sender's key, not a file name.

**`sluice recv`**

`recv TICKET...` takes one or more tickets, or `-` to read one ticket from standard input.
`--out DIR` names the folder (the current one by default), or `-` writes a single file's bytes to
standard output; a live ticket has no name, so its `--out` is the file to write. `--force` replaces a
file or link that is already there, `--allow-hidden` accepts a top-level name starting with `.`,
`--links` creates the symlinks the sender recorded (never followed outside the output), and
`--no-times` skips restoring modification times. `--range 4G:4.1G` fetches one byte range of a
one-file transfer. `--priority high|normal|low` orders several tickets, and `--limit-rate 50M` caps
the whole command. `--include PATTERN` takes only the entries of a folder ticket whose path matches
the pattern; give it more than once to take the entries any of them match. `*` matches inside one
name and `**` crosses a `/`, and the match is case-sensitive. A pattern that does not parse, or that
matches nothing in the ticket, is refused before anything is fetched or written. A folder is made
when an entry under it is taken, a link is taken when it matches, and only the taken files' content
is fetched. A partial store made with one selection is not continued by a receive with another: it
starts a store of its own. Because this receive takes only a part of the ticket, it does not end a
one-time `sluice send`: the sender is still serving afterwards, and only a receive of the whole
ticket stops it. A sender that serves partial receivers should run with `--keep` and stop it with
Ctrl+C.

Several tickets run through the queue: three at once by default, high before normal before low, and
a network failure waits the queue's backoff before the same ticket is tried again from its partial
store. A range, `--out -` and a live ticket each take one ticket at a time.

A folder arrives inside a folder of its own name. An existing top-level name refuses the whole
transfer before any content is fetched unless `--force`; an existing folder of the same name is
written into, and a folder is never replaced by a file or a link, even with `--force`. A link at a
target is replaced as an entry and never followed. A receive that was killed resumes only when its
partial store's marker names the same ticket; a file whose content already matches is skipped, and
anything else at its target is a clash.

Before it writes anything, a receive prints one line with what the manifest will write, the same
line `sluice inspect` prints: `3 files, 2 folders, 1.2 GiB`. In `--json` that is an event of its
own, `{"event":"planned","files":3,"folders":2,"links":0,"bytes":1258291}`. With `--include` the
line counts only what it takes and says what part of the transfer that is, `1 file, 1.0 KiB of 9
files, 12.0 MiB`, and the JSON event carries `"of_files":9,"of_bytes":12582912` beside the counts.

**`sluice inspect`**

`inspect TICKET` reads the ticket's hash sequence and manifest, and prints what the transfer offers:
how many files, folders and links, how many bytes, and the names as a tree with a folder's name
ending in `/` and a link's in `@`. It fetches no content and writes nothing: the manifest is read
into a temporary store under the home's `tmp` that is removed before the command ends, under the
same limits a receive applies. The tree shows at most 50 names; `--all` lists every one, and a cut
tree ends with a line that counts the rest. A name is printed with a control or a bidirectional
character written as its escape (`\u{202e}`), so it cannot reorder the line it is in.

`--include PATTERN` prints the view a receive with the same pattern would write: only the entries it
takes, the folders above them, and the summary line ending `of 9 files, 12.0 MiB`. It takes the same
patterns, refuses the same ones, and is the way to try a pattern before receiving with it. Neither
`--include` nor a pattern on a live or a raw ticket has anything to choose from, so both are refused.

A live ticket has no manifest, so there is nothing to read: the command says so and exits 0. A raw
ticket (from `send -`) has a size and no name.

```
1 file, 1.5 KiB
one.bin
```

With `--json` the same is one object, and it carries every entry whatever `--all` says, because a
line limit is about a terminal and not about a value:

```
{"files":2,"folders":2,"links":0,"bytes":6,"entries":[{"path":"tree","kind":"directory","size":0},{"path":"tree/a.txt","kind":"file","size":4},{"path":"tree/sub","kind":"directory","size":0},{"path":"tree/sub/b.txt","kind":"file","size":2}]}
```

`kind` is `file`, `directory` or `link`, and `size` is the file's bytes. A live ticket prints
`{"live":true}`.

**`sluice doctor`**

`sluice doctor` checks the home, who owns it and who else can reach it, its layout version, its
lock, the identity key and the free space on the home's volume, and changes nothing. It prints one
line per check with its status, `ok`, `warning`, `failed` or `skipped`, and under a warning or a
failure the step that fixes it. With `--json` it prints one object:

```
{"checks":[{"name":"home","status":"ok","detail":"<path>, from SLUICE_HOME","fix":null},...]}
```

`name` is `version`, `home`, `access`, `layout`, `lock`, `key`, `space`, then either `transport` or
the network names `udp`, `address`, `nat`, `relay` and `portmap`, in that order. `access` reports the
home folder, whether another user owns it or others can reach it, and `key` the key: its owner,
whether it is a link or a regular file, how many names it has, and its access. So a home and a key
that are both open to others are two failures. `home` also fails when `blobs`, `db` or `logs` is a
link or belongs to another user, which `sluice id` refuses to open, and when the lock or `lock.pid`
is not a plain file. `sluice id` refuses a key that is a link, is not a regular file, has more than
one name, or belongs to another user, and doctor reports each the same way. A home is read by the
path the system resolves for it, so one that is a link, or longer than 260 characters on Windows,
is checked as `sluice id` opens it, and a message names the path as it was given. `detail` and `fix`
are for people, and `fix` is `null` when there is nothing to do.

`--network` leaves the local report as it is and adds the network lines: a UDP socket is bound, the
relay named by `--relay` (or n0's with `--n0-relays`) is asked for a net report, and the public
address, the NAT mapping (one address to every server, or a different one, which is a hard NAT) and
whether a router answered a UPnP, PCP or NAT-PMP request are reported. Without a relay the line says
`none, so two hard NATs cannot connect`. A network check never changes the exit code: it is a
warning line, because a network is a fact to report.

The report is the command's result, so it goes to stdout whatever it finds, with or without
`--json`, and nothing goes to stderr. The exit code is 0 when no check failed: warnings do not fail.
Otherwise it is the code of the most serious kind that failed, which is the highest number: 6 for
policy, such as a key other users can read, a home that another user owns, or a layout newer than
this sluice, and 5 for storage.

**Other commands**

`sluice completions <shell>` prints the completion script for `bash`, `zsh`, `fish`, `powershell` or
`elvish`, with or without `--json`. `cargo xtask man <dir>` writes the man pages from the same
definition. They are built when wanted and are not checked in.
