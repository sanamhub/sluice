# Publishing the package files of a release

A release carries the archives and the shell and PowerShell installers that `dist` builds
([`releasing.md`](releasing.md)). Homebrew, Scoop, winget and the AUR each want a file of their own
that names those archives and their checksums. This runbook writes those files from the release's own
`sha256` sums, so nobody retypes a hash, and says what publishing each one needs. Nothing in this
repository publishes them: the release workflow builds archives, and a maintainer copies a file into
the registry by hand.

## Write the files

1. Download the release's checksum assets into one folder, for example:

   ```
   gh release download v0.2.0 --repo sanamhub/sluice --pattern '*.sha256' --dir dist
   ```

2. Write the files from them:

   ```
   cargo xtask package 0.2.0 --from dist --out packaging/out
   ```

   The version is the tag without its `v`. The command writes one folder per registry:
   `packaging/out/homebrew/sluice.rb`, `packaging/out/scoop/sluice.json`,
   `packaging/out/winget/` with the version, installer and `en-US` locale manifests, and
   `packaging/out/aur/PKGBUILD`. It stops when the release has no checksum for one of the six
   targets, naming the archive, because a file without its hash cannot be published.

   `packaging/expected/` holds the same six files for a made-up version and made-up hashes. They are
   the generator's test: `cargo test -p xtask` compares what the templates give against them, so a
   template change is a diff a reviewer sees. Do not publish anything from `expected/`.

3. Do not edit a generated file. Change the template in `xtask/src/package/render.rs`, run
   `cargo xtask package` again, and update `packaging/expected/` with the new output in the same
   commit.

## Homebrew

The formula goes in a tap: a repository named `homebrew-tap` under the same owner, at
`Formula/sluice.rb`. A user then runs `brew install sanamhub/tap/sluice`.

- **What it needs:** an account with write access to the tap. No token is stored in this repository.
- **Check it first, on a Mac:** `brew audit --strict --online Formula/sluice.rb` and
  `brew install --build-from-source sanamhub/tap/sluice`, then `brew test sluice`.
- **Then:** commit the formula to the tap. Homebrew's own `homebrew-core` also takes formulae, but a
  submission there is a pull request against `Homebrew/homebrew-core` and follows its own rules; a tap
  is what this repository ships.

`brew` is not available on the machine these files were generated on, so `brew audit` did not run
there.

## Scoop

The manifest goes in a bucket: a repository named `scoop-bucket` under the same owner, at
`scoop.json` in its root (the file is called `sluice.json` in the bucket). A user then runs
`scoop bucket add sanamhub https://github.com/sanamhub/scoop-bucket` and `scoop install sluice`.

- **What it needs:** an account with write access to the bucket. Nothing is stored here.
- **Check it first, on Windows with Scoop:** `scoop checkver sluice` and `scoop install sluice`, then
  `sluice --version`. The `bin` entry is `sluice.exe`, because `dist` puts a `.zip`'s files at its
  root; a `.tar.xz` keeps them under a folder named for the archive.
- **Then:** commit the manifest to the bucket. The `autoupdate` block lets the bucket's own
  `checkver` job move it on when a newer release appears.

`scoop` is not available on the machine these files were generated on, so its checks did not run
there; the JSON was parsed and its `version`, `url` and `hash` fields were read back.

## winget

The three manifests go to `microsoft/winget-pkgs`, under
`manifests/s/sanamhub/sluice/<version>/`: the version, installer and `en-US` locale files, whose names
already say where they belong.

- **What it needs:** a fork of `microsoft/winget-pkgs`, a GitHub account that has signed Microsoft's
  CLA, and a pull request from the fork. No token is stored in this repository.
- **Check it first:** `winget validate --manifest packaging/out/winget` (this ran here and passed),
  then `winget install --manifest packaging/out/winget` on a Windows machine, which installs the
  archive's `sluice.exe` as a portable command.
- **Then:** copy the folder into the fork under the path above, commit, push, and open the pull
  request. The winget validation pipeline runs the same checks on it.

## AUR

`sluice-bin` is a binary package: the `PKGBUILD` downloads the release's two Linux archives and
installs the binary from them. It lives in its own AUR repository,
`ssh://aur@aur.archlinux.org/sluice-bin.git`.

- **What it needs:** an account on `aur.archlinux.org` with an SSH key added to it, and no token in
  this repository.
- **Check it first:** `namcap PKGBUILD` and `makepkg -f` in a clean folder (both on Arch), then
  `pacman -U sluice-bin-<version>-1-*.pkg.tar.zst`. `bash -n PKGBUILD` checks only the syntax and ran
  here.
- **Then:** copy the `PKGBUILD` into a clone of the AUR repository, run
  `makepkg --printsrcinfo > .SRCINFO`, commit both files, and push. The AUR serves the package from
  that commit; there is no review step and no way to edit a published version, so a mistake is a new
  `pkgrel`.

`namcap` and `makepkg` are not available on the machine these files were generated on, so they did
not run there.

## What each file carries

Every file names the project's homepage, the description `README.md` opens with, and the licence
`MIT OR Apache-2.0` in the form its registry expects: Homebrew's `any_of`, Scoop's and winget's
licence string, and the AUR's two entries. No file holds a personal address, an account name other
than the repository's owner, or a secret.
