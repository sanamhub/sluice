# 0039. A paired device holds a grant, and remote control serves the same service over iroh

**Status:** Accepted **Date:** 2026-11-17 **Approved by:** @sanamhub **Relates to:** ADR 0018, ADR 0036, ADR 0037, ADR 0038

## Context

A transfer starts with a ticket, and the ticket is the only thing a provider trusts. A provider
serves under the user's identity key, so its ticket names an id that does not change, but the
endpoint carrying that id is bound for the transfer and closed after it, and a receiver binds a key
made for that one receive. Between transfers nothing answers for the daemon on the network, and the
control interface is a socket or a pipe only the local user can open, with the page on a loopback
address (ADR 0036, ADR 0037).

Two things people want need an identity that outlives a transfer:

1. **Send to my own devices without a ticket.** A laptop sends to a desktop because the two were
   paired once, and the desktop receives the content into an inbox while nobody is at the keyboard.
2. **Drive a daemon from another device.** A phone or a second laptop starts and watches transfers
   on a home box or a NAS.

Both rest on one primitive: a daemon keeps a list of endpoint ids it trusts, and each entry carries
a grant. The rest of this record says what a grant is, how one is made, and what its holder may do.

The pieces this lands on:

- The control interface is one plain `Service` struct behind gRPC with `tonic`: 1 MiB per message,
  64 open calls per connection and 32 connections (ADR 0037). A listener that reuses it costs no new
  schema and no new client.
- Every path a client names is a root's label and a path below it, opened from the root's handle a
  component at a time (ADR 0038, `SPEC.md` section 11). A remote caller gets that rule or becomes a
  way around it.
- Every connection passes the handshake check for `X25519MLKEM768` (ADR 0018). A remote listener is
  built by the same code, so it applies the same rule.
- `SPEC.md` section 4 already has one ticket envelope, one text form and a 4096-byte limit. A second
  envelope would be a second decoder and a second set of rules for one field.
- A 32-byte secret has 2^256 possible values. A pairing ticket with no address and no relay is 76
  bytes, which is 129 characters with the prefix.

**Prior art.** n0's iroh-share shipped this. Read on 2026-11-17 from
[`docs/remote-access.md`](https://github.com/n0-computer/iroh-share/blob/main/docs/remote-access.md),
[`iroh-share-proto/src/pairing.rs`](https://github.com/n0-computer/iroh-share/blob/main/iroh-share-proto/src/pairing.rs)
and [`docs/internals.md`](https://github.com/n0-computer/iroh-share/blob/main/docs/internals.md). It
is an application, not a library, so nothing here depends on it.

## What we take from iroh-share, and what we change

| What | iroh-share | Here | Why |
|---|---|---|---|
| Pairing | Its own ALPN on the daemon's endpoint, `/iroh-share/pair/1` | `/sluice/pair/1` on the daemon's endpoint | A pairing stream has to work before any grant exists, so it cannot be a call behind one |
| Credential | 32 random bytes in the ticket, compared in constant time | The same | The secret is the whole credential, and a constant-time compare costs a few cycles |
| Whose id | The authenticated connection's endpoint id, never one the client sends | The same | A client cannot name another device |
| When it is saved | The grant is written before success is reported | The same | A crash must not lose a pairing the client believes in |
| The answer | One status byte | The same, with the values in `SPEC.md` section 11.2 | 32 bytes in and 1 byte out need no parser |
| Revoking | `list` and `revoke`, and revoke closes live control connections and watch streams | The same, and the grant is read again on every call | A revocation that leaves a running watcher is not a revocation |
| A bad allowlist | Fails closed | Fails closed, and the daemon does not overwrite the file | A corrupt file must refuse peers, not admit them |
| Lifetime | Until the daemon stops | 10 minutes, and single use | Decision 2 |
| Envelope | `iroh-tickets`, postcard, prefix `irohshare` | Kind 2 of our envelope, prefix `sluice1` | One decoder, one text form, one set of rules |
| Grants | Full control only | `deliver` and `control`, `deliver` by default | Sending me a file is not driving my computer |
| Transport | iRPC (`irpc`) | gRPC with `tonic` over HTTP/2 | The schema is already a gRPC service and the local client stays one |
| A revoked grant | A ticket cannot restore revoked access | Spelled out: a revoke forgets the ticket that made the grant | The same outcome, and a test can show it |
| The local owner | An implicit grant for a second state directory | The socket of ADR 0037, unchanged | One local rule exists already, and pairing adds none |

## Decision

1. **Pairing is one stream on `/sluice/pair/1`, on the daemon's endpoint.** The client writes
   exactly 32 bytes and closes its side. The daemon compares them in constant time with every ticket
   it has made, has not forgotten and has not expired, writes one status byte, and closes.
   - The grant is written for the endpoint id of the authenticated connection. Nothing in the stream
     can name another device, so nothing in the stream can grant one.
   - The grant is on disk before status 0 is written. Status 2 means the file could not be read or
     written.
   - A file that cannot be read is no grants. The daemon starts, refuses every paired peer, refuses
     to pair, and leaves the file alone for its user to fix.
   - A daemon has to be pairable, so this listener is on whenever the daemon runs.
   - It accepts at most 4 streams from one endpoint at once and 16 in total, and closes a stream
     that writes more than 32 bytes, so a stranger who knows the endpoint id costs 4 streams and
     132 bytes.

2. **A pairing ticket is single use and expires.**
   - The default lifetime is 10 minutes. `sluice pair --ttl SECONDS` may set up to one hour, and the
     command refuses more: a pairing ticket is a bearer invitation to an inbox or to the whole
     control interface, and a long one is a standing key.
   - iroh-share's tickets live until the daemon stops. Ours cannot. An unused ticket is a key in a
     shell history, a chat message or a screenshot, and ten minutes bounds it at the cost of one
     command. The daemon's list of tickets lives in memory, so a restart ends them anyway, and an
     expiry at least says so.
   - It is kind 2 of the envelope in `SPEC.md` section 4, so one decoder and one text form exist,
     and a pairing ticket cannot be parsed by the content rules.
   - Redeeming a ticket again from the same endpoint id answers 0 while its grant is there, so a
     client that lost the status byte retries safely.
   - A revoke forgets the ticket that made the grant. A retry after a revoke is refused, and the
     device needs a new ticket. That is what "a ticket never restores a revoked grant" means here.

3. **Two grants, and `deliver` is the default.**
   - `deliver`: hand this daemon a ticket and have it received into the inbox, within the quota.
     Nothing else. It cannot list a name, read a file, start a send, watch a job or call the control
     interface.
   - `control`: every call of `proto/sluice/v1`, including the three that manage devices and
     `Shutdown`. It includes what `deliver` allows.
   - `sluice pair --grant deliver|control`, and no flag means `deliver`.
   - Why two: a control peer is the daemon's user. It can list the names under every root and start
     a send of any file there to a ticket it holds. The two things people asked for are not the same
     size, and a flag is a cheap way to say which one they asked for.
   - Why not one grant per call: each call would need its own decision and its own explanation, and
     the jobs people want are whole ones, deliver a file and drive the daemon, not sets of calls.

4. **Remote control is the same service on `/sluice/control/1`, and never TCP.**
   - gRPC over HTTP/2 on one bidirectional stream per connection, which is what `tonic` already
     serves over the socket, with the wire rules in `SPEC.md` section 11.4. The generated client and
     the existing `ControlService` are used unchanged. The transport is a `Channel` whose connector
     returns the stream, and a server that hands each accepted stream to the same service.
   - A client SHOULD put `WatchEvents` on a connection of its own, a second `Channel` from the same
     connector, and keep its other calls on the first. Two `Channel` values need no new client code,
     and a cancel then has flow control of its own, so a watch that is not being read cannot hold it
     up. A watch is the one call that can sit for minutes with bytes in flight.
   - The alternative was one stream per call, with the same `prost` messages and the gRPC framing
     but no HTTP/2. It gives every call its own flow control, its own reset and its own
     cancellation, where the rule above gives it to the one call that needs it. It also costs a
     second client: the generated stubs speak HTTP/2, so every binding that drives a daemon remotely
     would write its own framing and its own streaming for `WatchEvents`, which is the cost ADR 0037
     chose gRPC to avoid.
   - The cost we accept: HTTP/2's multiplexer nested inside QUIC's, one lost packet delays the other
     calls on that stream by a round trip, and the HTTP/2 connection window is shared. The
     two-connection rule takes the case that mattered out of it, a cancel behind a stalled watch,
     and leaves head-of-line delay among unary calls, which are one short request and one short
     answer. The daemon caps a watcher's queue at 256 events, reports `dropped`, and closes a
     watcher whose queue has been full for 10 seconds, so a watch cannot hold the window either.
   - The TCP clause of `SPEC.md` section 11 goes. A token on a port is a shared secret, a
     certificate story and a reachable port, and it names no device. The iroh connection names a
     device at the handshake, post-quantum, and its grant is per device. That also empties the last
     sentence of ADR 0036's point 6, which allowed every other TCP listener with TLS and a token: no
     other TCP listener exists after this.
   - The socket of ADR 0037 stays as it is. This listener is off by default, starts when the first
     `control` grant is written and stops when the last one is revoked. A peer without a grant is
     closed when it opens the stream, before a call is read.

5. **A delivery is one stream on `/sluice/deliver/1`.**
   - Decision 3 fixes what a `deliver` peer may do but not how it says it, so this record adds the
     request: a ticket's text, an optional name, one status byte, with the fields and values in
     `SPEC.md` section 11.3.
   - Not a call on the control schema: the control listener must stay off until a `control` grant
     exists, and a daemon with only `deliver` grants must not need it.
   - The bytes arrive on the static content transport, with the daemon as the receiver, so the
     refusal of every push in `SPEC.md` section 5 stands and the control schema gains no call that
     carries content.
   - The ticket may name any provider, as a ticket pasted into `sluice recv` may. The grant
     authorizes the delivery, not the source, and the content is verified against its root before it
     lands. The receive binds a key of its own (decision 8), so that provider learns nothing that
     links one delivery to another.
   - The inbox is a folder the daemon was started with, inside a root. No inbox means no `deliver`
     ticket and no delivery listener. The listener runs while at least one grant can use it, which
     is any `deliver` or `control` grant.

6. **A revoke takes effect immediately.**
   - `RevokeDevice` deletes the grant, writes the file, closes that endpoint's open connections and
     watch streams, forgets the ticket that made the grant, and answers after all of it.
   - The grant is read again for every call, so a connection that races the close serves nothing.
   - `sluice devices list | rename | revoke` manages the same list, and the schema gains
     `ListDevices`, `RenameDevice` and `RevokeDevice`, so the page and MCP manage devices too.
   - A label starts as the first 8 hex characters of the endpoint id, because the pairing stream
     names nothing else, and `rename` is how a person makes it recognizable.
   - The daemon's own user is not in the list. The socket is the local grant and a revoke cannot
     take it away.
   - The device that redeemed a ticket keeps the daemon's endpoint id and addresses under the name
     it was given (`sluice pair --name`, shown by `sluice pair --list`), in
     `$SLUICE_HOME/paired.json`. A device's saved daemons and a daemon's grants are different lists:
     `sluice devices` is the daemon's, and `sluice pair --list` is this device's.

7. **Paths stay on the daemon, and so do the bytes.**
   - Every path a remote caller names is a root's label and a path below it, checked and opened as
     `SPEC.md` section 11 says.
   - No file byte crosses the control or delivery connection. A delivery's bytes come in on the
     static transport, and a send's bytes go out on one.
   - A `control` peer can read the names under every root with `ListEntries`. That is one reason
     `deliver` is the default, and why the device list shows each device's grant.

8. **The daemon's endpoint is its identity, and a receive keeps its own key.**
   - The daemon binds one endpoint for the life of the process with `$SLUICE_HOME/identity.key`, and
     the three ALPNs of this record live on it.
   - A send's handlers attach to that endpoint while the send runs and are removed when it ends, so
     one id and one set of addresses answer for the daemon's identity whether or not a send is
     running.
   - A receive keeps what it has today: it binds a key made for that one receive, so the provider it
     fetches from sees an id that answers for nothing else. A delivery is a receive, so it does the
     same, and the provider a delivered ticket names learns nothing that links one delivery to
     another.
   - Only the listeners need the stable id, and fetching does not. A receive on the identity
     endpoint would tell every provider a daemon fetches from that the fetch is that daemon's, and
     two providers that compare the ids they saw could link its receives. That is a correlation
     handle nobody asked for. The listeners need one id because a peer has to reach the daemon
     before any grant exists, and a pairing ticket names it anyway.
   - A one-shot `sluice send` or `sluice recv` binds its own endpoint, as it does today.
   - A pairing ticket carries that endpoint id with the addresses and relay a content ticket
     carries, so a phone off the LAN needs the daemon to have a relay.
   - A daemon bound to loopback cannot pair with another computer: its ticket has no address another
     computer can reach.
   - The connection passes the handshake check like any other (ADR 0018), so a peer on another group
     is closed with `0x534C0002` before it writes a byte.
   - Rotating the identity key breaks every pairing, which the threat model and `docs/design.md`
     already say.

## Alternatives

- **A ticket that works until the daemon stops**, as iroh-share has. Refused in decision 2: an
  unused ticket is a standing key, and a restart is not a boundary anyone can see.
- **One stream per call for remote control.** Refused in decision 4: it gives every call its own
  cancellation, where the two-connection rule wins the case that matters, and it costs a second
  client in every binding.
- **TCP with TLS and a token**, which `SPEC.md` section 11 allows today. Refused: a second
  authentication story for a peer that can be named at the handshake, and a shared token is not a
  device.
- **A delivery as a call on the control schema.** Refused: it would make the control listener a
  requirement for a daemon that has no control grant.
- **One grant, full control**, as iroh-share has. Refused in decision 3.
- **`iroh-tickets` for the pairing ticket.** Refused: one envelope, one decoder, one text form.
- **A pairing stream that names the device in its request.** Refused: the authenticated identity is
  the only name a client cannot forge.
- **Writing the grant after answering 0.** Refused: the client would believe in a pairing the daemon
  forgot.
- **Pairing both daemons in one step.** Refused in the open questions, where each direction is
  granted separately.

## Open questions, with a recommendation

**Is remote control in 1.1, or does 1.1 ship only `deliver`?**

Recommendation: 1.1 ships `deliver`, and `control` ships in 1.2 next to the PWA and the Tauri app.
`deliver` is one direction and one request, and its failure lands in a folder a person can look at.
`control` is a remote administrator: the whole interface, on a device with no window into what it
can do until 1.2's clients exist. The wire is defined now for both, and a daemon that serves only
`deliver` has no control listener, so shipping `control` later breaks nothing. The cost of waiting
is a second laptop that cannot drive a NAS until 1.2; it can still send into the inbox.

**Inbox behaviour for `deliver`.**

Recommendation: auto-accept, with no prompt. A prompt nobody sees is a transfer that hangs, and the
inbox exists for the case where nobody is at the keyboard. The bounds replace the prompt: one
folder, a quota, no overwrite, a job that can be cancelled, and a device that can be revoked. The
default quota is 10 GiB, set with `--inbox-quota`. It holds a 4 GiB film or a phone's photo library,
and a mistake costs a cleanup rather than a disk. The daemon reserves the size a ticket states when
it accepts, so two deliveries cannot take the last gigabyte between them. When the quota is full,
the daemon refuses the next delivery before its first byte and names the room, and it never deletes
an inbox file to make room. A delivery that states no size, which is a live stream, runs to the room
and fails there.

**Can a pairing ticket travel as a short code?**

Recommendation: yes, as a carrier and not as a new protocol. `sluice pair --short-code` publishes
the pairing ticket's text through the relay with SPAKE2, as `SPEC.md` section 4 does for a content
ticket, with the same rate limit and the same warning that `spake2` has no independent audit. The
client redeems the code, gets the ticket and presents it on the pairing stream, and the ticket's own
expiry bounds the code. Not as the only carrier: a code needs a relay the user configured and an
unaudited primitive, and a 32-byte ticket needs neither. A phone that reads a QR code takes a ticket
in a second.

**Do two paired daemons pair each other both ways in one step?**

Recommendation: each direction is granted separately. A direction is a grant in one daemon and a
saved address in the other, and its ticket is handed from the receiving side to the sending side.
One step would mean one daemon keeps another daemon's unused ticket and spends it, which is a
credential for a different computer held in this one, and a compromise of one becomes a compromise
of both. A command that drives both ends can exist later, `sluice pair --both`, and it would still
make two tickets and two grants behind one keystroke. Build it only if two steps turn out to be
annoying.

**Does `WatchEvents` need snapshot-then-updates, and a rule that disconnects a slow watcher?**

Recommendation: no snapshot mode, and yes to the disconnect. Today a watch is one job, from the
moment the call is made, ending when the job ends, and a watcher whose queue is full loses events
and reads one `dropped` with the count (`SPEC.md` section 11). `ListJobs` is already the snapshot:
it gives each job's state and bytes, and the stream says what happens next. A snapshot on the stream
would duplicate that call, add a message type, and make the start of a stream a different thing from
its middle. The disconnect is worth adding because a remote client holds one of 64 call slots and a
share of one HTTP/2 connection. A watcher whose queue has been full for 10 seconds has its call
ended with `RESOURCE_EXHAUSTED` and the number of events it missed, and the client reads `ListJobs`
again. One rule on the socket and on iroh is one behavior to test. iroh-share disconnects slow
watchers and has the client reconnect for a fresh snapshot, which is the same shape without a
snapshot on the wire.

## Consequences

- The daemon answers on the network whenever it runs. The pairing ALPN must be reachable for a first
  pairing, so a daemon on a laptop has an endpoint a stranger who knows its id can open a stream to.
  The limits in decision 1 bound that, and a peer without a grant learns nothing.
- The daemon's endpoint id becomes stable and appears in every pairing ticket. It was never secret,
  which the threat model already says, and the grant list is what admits a device.
- The daemon holds one endpoint for its whole life for its identity and its listeners, and its
  sends attach to it, where every job binds an endpoint of its own today. A receive still binds one,
  so the id a provider sees for a fetch is not the daemon's, and the identity has one set of
  addresses whatever is running.
- Two new files in the home, `$SLUICE_HOME/devices.json` for the grants and
  `$SLUICE_HOME/paired.json` for the daemons this computer may drive. Neither holds a secret, and
  losing either loses the record, not a key: a device pairs again with a new ticket.
- `proto/sluice/v1` gains three calls and a device message, and the checked-in generated code
  changes with them in the change that implements this.
- A `control` device is as powerful as the person at the keyboard. The ticket that makes one and the
  device list must both say so.
- 1.1 ships `deliver`; `control` waits for 1.2. A daemon with only `deliver` grants runs no control
  listener.
- The inbox can hold 10 GiB that the daemon never cleans. The quota stops it growing, and the user
  decides what to delete.
- A pairing ticket in a terminal, a chat or a screenshot is a live invitation for 10 minutes. The
  command prints it once and no log line holds one.
- A content ticket is unchanged. Pairing adds a kind to the same envelope, and a send or a receive
  with a ticket works whether or not a device is paired.
- Revisit the transport if a binding cannot give a watch a connection of its own, or if watch
  traffic ever dominates a connection. Revisit the 10-minute expiry if a workflow needs a ticket to
  survive a lunch break. Revisit two-step pairing if people pair the same two computers often.
