# Changelog

All notable changes to Sluice are in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/sanamhub/sluice/releases/tag/v0.1.0) - 2026-10-25

### Added

- *(doctor)* report the home's access as a check of its own
- *(doctor)* check the Windows home folder's access list
- *(cli)* report the local checks with sluice doctor
- *(cli)* add sluice completions and cargo xtask man
- *(cli)* end a panic as a Bug and a closed stdout as success
- *(cli)* add sluice id to print and rotate the endpoint id
- add workspace with core error type and cli stub

### Fixed

- *(core)* refuse a key or a folder another user owns, on every open
- *(doctor)* fail a folder of the home that is a link
- *(core)* refuse a home another user owns, and a chmod that changed nothing
- *(core)* close the gap in the mode message and wrap the long lines

### Other

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
