# The package files a release needs

`expected/` holds what `cargo xtask package 1.2.3` writes with made-up checksums: the Homebrew
formula, the Scoop manifest, the three winget manifests and the AUR `PKGBUILD`, one folder per
registry. They are the generator's test — a template change shows up as a diff here — and are not
files to publish.

[`docs/runbooks/packaging.md`](../docs/runbooks/packaging.md) says how a maintainer writes the real
ones from a release and publishes each one.
