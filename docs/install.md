# Install

Sluice 0.1.0 ships for six targets: Linux and macOS on x86-64 and ARM64, and Windows on x86-64 and
ARM64. Pick one of the channels below. Every one of them installs the same `sluice` binary; the
release page has the archives and their checksums, and `gh attestation verify` reads the provenance
from GitHub's attestation store.

| Channel | Platforms | Command |
|---|---|---|
| Shell installer | Linux and macOS, both architectures | `curl ... \| sh` |
| PowerShell installer | Windows, both architectures | `irm ... \| iex` |
| Archives | all six | `gh release download v0.1.0` |
| Homebrew | macOS and Linux, both architectures | `brew install sanamhub/tap/sluice` |
| Scoop | Windows, both architectures | `scoop install sluice` |
| winget | Windows, both architectures | `winget install --id sanamhub.sluice -e` |
| AUR | Arch Linux, x86-64 and ARM64 | `yay -S sluice-bin` |
| Container image | Linux amd64 and arm64 | `docker pull ghcr.io/sanamhub/sluice:0.1.0` |
| From source | the six targets, with Rust | `cargo install --path crates/sluice-cli` |

The archives are the reference form. The installers check less than an archive checked by hand, and
the package managers check the archive's hash but not its attestation. If a channel and this page
disagree, the archive and [`docs/runbooks/packaging.md`](runbooks/packaging.md) are what the release
builds.

## Archives

| Machine | Archive |
|---|---|
| Linux, x86-64 | `sluice-cli-x86_64-unknown-linux-gnu.tar.xz` |
| Linux, ARM64 | `sluice-cli-aarch64-unknown-linux-gnu.tar.xz` |
| macOS, Apple silicon | `sluice-cli-aarch64-apple-darwin.tar.xz` |
| macOS, Intel | `sluice-cli-x86_64-apple-darwin.tar.xz` |
| Windows, x86-64 | `sluice-cli-x86_64-pc-windows-msvc.zip` |
| Windows, ARM64 | `sluice-cli-aarch64-pc-windows-msvc.zip` |

```
gh release download v0.1.0 --repo sanamhub/sluice \
  --pattern 'sluice-cli-x86_64-unknown-linux-gnu.tar.xz*'
gh attestation verify sluice-cli-x86_64-unknown-linux-gnu.tar.xz --repo sanamhub/sluice \
  --signer-workflow sanamhub/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v0.1.0
sha256sum --check sluice-cli-x86_64-unknown-linux-gnu.tar.xz.sha256
tar xf sluice-cli-x86_64-unknown-linux-gnu.tar.xz
./sluice-cli-x86_64-unknown-linux-gnu/sluice --version
```

`Verification succeeded!` means the file was built by `release.yml` in that repository, from the
commit of tag `v0.1.0`. Without `--signer-workflow`, any workflow of the repository that holds
`id-token: write` could have signed it, and without `--source-ref`, a run on any branch or tag.
`sha256sum --check` prints `OK`; on macOS, use `shasum -a 256 --check` instead.

The attestation says where the file was built and from which commit. It is provenance, not a
statement that the file is safe. The CycloneDX SBOM is a file of the release, covered by the same
attestation; only the images carry SBOM attestations, one per architecture.

To upgrade, download the next version's archive the same way. To uninstall, remove the folder the
archive unpacked to; nothing was installed outside it.

## Shell installer

```
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/sanamhub/sluice/releases/latest/download/sluice-cli-installer.sh | sh
sluice --version
command -v sluice
```

It downloads the archive for the machine, compares its SHA-256 with the value written into the
script, and installs `sluice` to `$CARGO_HOME/bin`, or `$HOME/.cargo/bin` when `CARGO_HOME` is not
set. It adds that folder to `PATH` in the shell's profile. It skips the checksum on a machine
without `sha256sum`. It checks no signature and no attestation, so it is only as good as the copy of
the script that was run. What `dist` 0.33.1 writes into it is in the
[threat model](threat-model.md#supply-chain).

Upgrade by running the same command again. Uninstall with
`rm "$HOME/.cargo/bin/sluice"`, or the same file under `$CARGO_HOME/bin`.

## PowerShell installer

```
irm https://github.com/sanamhub/sluice/releases/latest/download/sluice-cli-installer.ps1 | iex
sluice --version
Get-Command sluice
```

It installs `sluice.exe` to `$env:CARGO_HOME\bin`, or `$HOME\.cargo\bin` when `CARGO_HOME` is not
set, and adds that folder to the user's `PATH`. It checks nothing: no hash, no signature and no
attestation. Prefer an archive from the release page when that matters.

Upgrade by running the same command again. Uninstall with
`Remove-Item "$HOME\.cargo\bin\sluice.exe"`, or the same file under `$env:CARGO_HOME\bin`.

## Homebrew

```
brew install sanamhub/tap/sluice
sluice --version
brew list --versions sluice
```

The formula downloads the release archives for macOS and Linux, on both architectures, and Homebrew
checks each one's SHA-256. Upgrade with `brew upgrade sluice`. Uninstall with
`brew uninstall sluice`, and remove the tap with `brew untap sanamhub/tap`.

The formula is in the tap described in
[`packaging.md`](runbooks/packaging.md#homebrew).

## Scoop

```
scoop bucket add sanamhub https://github.com/sanamhub/scoop-bucket
scoop install sluice
sluice --version
scoop list sluice
```

The manifest names the two Windows archives and their SHA-256, and points `sluice` at the
`sluice.exe` inside each. Upgrade with `scoop update` and then `scoop update sluice`. Uninstall with
`scoop uninstall sluice`, and remove the bucket with `scoop bucket rm sanamhub`.

## winget

```
winget install --id sanamhub.sluice -e
sluice --version
winget list --id sanamhub.sluice -e
```

The identifier is the one `cargo xtask package` writes, `sanamhub.sluice`. The manifests name the
two Windows archives and their SHA-256, and install `sluice.exe` as a portable command. Upgrade with
`winget upgrade --id sanamhub.sluice -e`. Uninstall with
`winget uninstall --id sanamhub.sluice -e`.

## AUR

```
yay -S sluice-bin
sluice --version
pacman -Q sluice-bin
```

`sluice-bin` is a binary package: the `PKGBUILD` downloads the two Linux archives, checks their
SHA-256, and installs `sluice` to `/usr/bin`. Any AUR helper works; `paru -S sluice-bin` is the same
package. Upgrade with `yay -Syu`. Uninstall with `sudo pacman -Rns sluice-bin`.

## Verify an image

Both images are checked the same way, by their name in the registry. The version tag is the index
of both architectures, and `-amd64` and `-arm64` name each one:

```
gh attestation verify oci://ghcr.io/sanamhub/sluice:0.1.0 --repo sanamhub/sluice \
  --signer-workflow sanamhub/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v0.1.0
gh attestation verify oci://ghcr.io/sanamhub/sluice:0.1.0-arm64 --repo sanamhub/sluice \
  --signer-workflow sanamhub/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v0.1.0 \
  --predicate-type https://spdx.dev/Document/v2.3
```

`sluice-relay` takes the place of `sluice` for the relay image. The first command checks the
provenance of the index, and the second the SBOM of the arm64 image. The index, `-amd64` and
`-arm64` each have a provenance attestation, and each architecture has an SBOM. The SPDX version in
the predicate type is the one syft writes into the SBOM, which `actions/attest` copies
(`src/sbom.ts:85`).

**A tag is not the image.** `gh attestation verify` proves that `release.yml`, on that tag, built
the digest it is given. It does not say what the tag names now: no package can name one workflow, so
any workflow in a repository with access to the package and `packages: write` can move a tag. An
image is trusted only after its attestation passes and it is pulled by that digest:

```
digest="$(docker buildx imagetools inspect ghcr.io/sanamhub/sluice:0.1.0 \
  --format '{{json .Manifest}}' | jq --raw-output .digest)"
gh attestation verify "oci://ghcr.io/sanamhub/sluice@$digest" --repo sanamhub/sluice \
  --signer-workflow sanamhub/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v0.1.0
docker pull "ghcr.io/sanamhub/sluice@$digest"
```

No package can name one workflow: GitHub's package settings grant Actions access to a repository,
not to a workflow file. The package's source label and the `release` environment say which jobs are
meant to push, not which ones can.

## The sluice image

`ghcr.io/sanamhub/sluice` is the tool for `linux/amd64` and `linux/arm64`. It runs as uid 65532, has
no shell, and keeps its home on a volume at `/home/sluice`:

```
docker volume create sluice-home
docker run --rm -v sluice-home:/home/sluice ghcr.io/sanamhub/sluice:0.1.0 id
docker run --rm ghcr.io/sanamhub/sluice:0.1.0 --version
```

A container that mounts the same volume is the same endpoint; one without it gets a fresh identity.
Send and receive with a folder mounted in, and use `--network host` on Linux for a direct path:

```
docker run --rm -it --network host -v sluice-home:/home/sluice \
  -v "$PWD:/data" ghcr.io/sanamhub/sluice:0.1.0 send /data/report.pdf
docker run --rm -it --network host -v sluice-home:/home/sluice \
  -v "$PWD/out:/out" ghcr.io/sanamhub/sluice:0.1.0 recv <ticket> --out /out
```

The mounted folder must be readable by uid 65532 to send from it and writable to receive into it.
On Docker's default bridge the container has a private address, so a peer elsewhere reaches it
through a relay passed with `--relay`; the transfer still works, at the relay's rate.
[`docker/cli/README.md`](../docker/cli/README.md) has the volume, the network modes and the build.

Upgrade with `docker pull ghcr.io/sanamhub/sluice:0.1.0` for the next version. Uninstall with
`docker image rm`, and remove the identity with `docker volume rm sluice-home`.

## The relay image

`ghcr.io/sanamhub/sluice-relay` is the relay for `linux/amd64` and `linux/arm64`, and it is a
different program from the tool image:

```
docker pull ghcr.io/sanamhub/sluice-relay:0.1.0
```

It runs closed, with an allowlist that ships empty, and it starts through a guard that refuses a
config that would open it by mistake. [`docker/relay/README.md`](../docker/relay/README.md) has the
certificate, the two recipes and the allowlist. Upgrade with the next version's tag; uninstall with
`docker image rm`.

## From source

The repository pins Rust 1.99.1 in `rust-toolchain.toml`, and `rustup` reads that file in the
checkout:

```
git clone https://github.com/sanamhub/sluice
cd sluice
rustc --version
cargo install --path crates/sluice-cli --locked
sluice --version
```

`rustc --version` starts with `rustc 1.99.1`; another version means `rustup` did not read the file,
and the build is not the one CI tested. Upgrade with `git pull` and the same `cargo install` line,
which needs `--force` once `sluice` is installed. Uninstall with `cargo uninstall sluice-cli`.

## What a binary carries

Each binary also carries its dependency tree from `cargo auditable`, which `cargo audit bin sluice`
checks against the RustSec database.
