# Changelog

All notable changes to Sluice are in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/sanamhub/sluice/releases/tag/v0.1.0) - 2026-10-29

### Added

- *(send)* bound a shared send by the endpoints it serves
- *(send)* count the receivers a shared ticket serves, and name them
- *(core)* a second selection writes into the folder the first made
- *(core)* count the bytes a selection fetches, packs and all
- *(cli)* take part of a folder with --include
- *(cli)* read a ticket's manifest and print what it offers
- *(engine)* say once when a transfer has no direct path after ten seconds
- *(cli)* read every size flag with the one parser, in both unit systems
- check the network behind doctor --network
- run several receives through the queue and cap a command's rate
- follow a file that is still growing
- send and receive through pipes and fetch one byte range
- resume an interrupted receive and keep a restarting sender's ticket
- send and receive folders, packs and the hash sequence
- send and receive one file from the command line
- *(doctor)* report the home's access as a check of its own
- *(doctor)* check the Windows home folder's access list
- *(cli)* report the local checks with sluice doctor
- *(cli)* add sluice completions and cargo xtask man
- *(cli)* end a panic as a Bug and a closed stdout as success
- *(cli)* add sluice id to print and rotate the endpoint id
- add workspace with core error type and cli stub

### Fixed

- *(send)* reserve the places before the ticket is printed
- *(core)* a partial receive does not claim the whole download
- *(cli)* escape every human line a name reaches, and stop escaping twice
- *(cli)* end an interrupted inspect as cancelled and remove its store
- *(cli)* escape every human line that carries text a stranger wrote
- wait for the removals a command started before it ends
- *(engine,cli)* cover the close, the device path and the blocking removal
- *(cli,engine)* say only what holds about a network with no relay, and refuse pipes by UNC
- *(engine)* measure a live receive's bare output name against the working folder
- *(cli)* say what the size units are in every flag's help
- *(engine)* bound a live stream, and end it with the follower's error
- *(engine)* clean up a killed command's temporary store, and give a kept store its own bytes
- let the queue start a job again, and end the command on the jobs that failed
- trust a partial store only when the home holds a record of it
- make a receive check before it claims a partial store
- give a range the same cap and let the queue own its retries
- make a receive keep its store and its sender honest
- close the receiver and sender holes left before 1.0
- bound metadata, prove the greeting and choose the controller
- *(core)* refuse a key or a folder another user owns, on every open
- *(doctor)* fail a folder of the home that is a link
- *(core)* refuse a home another user owns, and a chmod that changed nothing
- *(core)* close the gap in the mode message and wrap the long lines

### Other

- write the README for the 0.1.0 release
- split the provider wait, the event tests and the receiver line
- say what a place is, and who a fingerprint names
- split the files and functions that outgrew their size
- *(cli)* start the interrupt test's listener on its runtime
- *(cli)* hold the receive's store, not the sender's, in the removal test
- *(engine,cli)* pass the whole suite after the sends keep their own stores
- say what a missing relay rules out, and name the relay for short codes
- say what doctor --network sends, and to whom
- *(cli)* keep a test's output out of the crate folder
- reconstruct the state a killed run leaves instead of racing it
- keep the new code inside the size rules
- say what send and recv do now
- turn on the send throttle only when a cap is set
- *(cli)* put the command before its parts
- lead the README with the file, not the protocol
- *(cli)* close the pipe after one byte on every Unix
- complete the package metadata for the public tree
- remove phase and process words from the public tree
- say what the release, guard and home changes mean
- say what is true after the release, guard and home changes
- say what is true after the release, guard and home changes
- pin the long decimals at a byte's edge and the tests' RFC key
- build and publish the sluice binary with dist on a v tag
- *(cli)* wrap the README lines over 100 characters
- *(cli)* wrap the --json refusal of --rotate under 100 columns
- *(cli)* write down what scripts can rely on
- *(xtask)* add cold-start, and run it in CI on Linux and Windows
- *(cli)* pin the exit codes, JSON shapes, colour and help text
- *(cli)* print through one output module, with global --json and --color
- split the plan into design, roadmap, performance and threat model
- record the pre-implementation design as the starting point
