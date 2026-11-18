# sluice-meet

**Owns:** the short code as a person reads it, the BIP-0039 words, the SPAKE2 exchange that binds the
code to a nameplate, the meeting-place frames, and the two transport-free state machines: the
client's (sender and receiver) and the relay's meeting place. Specified in `SPEC.md` section 4.1 and
ADR 0041.

**Does not own:** the WebSocket, the HTTP listener, the relay configuration, the clock or the
commands. A caller gives this crate frames and the milliseconds its own clock reads; it gives back
frames to send and events to print. The CLI and the relay are the two callers today, and the browser
tab is the third when it comes.

**Must never:** touch the network, the filesystem or the clock, or write a code, nameplate, ticket,
SPAKE2 message or sealed value to a log. [`Code`] has no `Display`, its `Debug` prints the nameplate
and hides the words, and every error names the rule that failed and nothing of the code.

**The `spake2` warning.** `spake2` 0.4.0 has no independent audit, by its own README. It is the
newest stable release and ADR 0041 decision 3 chooses it over the pre-release, but its word-bound
exchange is the security path that protects a ticket. This crate, `SPEC.md` section 4.1 and the
threat model say so; a caller must not describe the code exchange as audited.

**Tests:** the code rules, the word list's pinned SHA-256, the key schedule and AEAD, every frame and
its refusals, and a scripted meeting through both state machines. The `short-code/` vectors under
`tests/vectors/` are run by `cargo xtask conformance`; the crate's own tests cover the same rules
without reading the files, so a change to either side shows as a disagreement.
