# Install

Nothing has been released yet. When something is, this is how to install it: download the archive
for the machine, check where it came from, and only then run anything from it.

## Download and verify an archive

| Machine | Archive |
|---|---|
| Linux, x86-64 | `sluice-cli-x86_64-unknown-linux-gnu.tar.xz` |
| Linux, ARM64 | `sluice-cli-aarch64-unknown-linux-gnu.tar.xz` |
| macOS, Apple silicon | `sluice-cli-aarch64-apple-darwin.tar.xz` |
| macOS, Intel | `sluice-cli-x86_64-apple-darwin.tar.xz` |
| Windows, x86-64 | `sluice-cli-x86_64-pc-windows-msvc.zip` |
| Windows, ARM64 | `sluice-cli-aarch64-pc-windows-msvc.zip` |

```
gh release download v0.2.0 --repo sanamhub/sluice \
  --pattern 'sluice-cli-x86_64-unknown-linux-gnu.tar.xz'
gh attestation verify sluice-cli-x86_64-unknown-linux-gnu.tar.xz --repo sanamhub/sluice \
  --signer-workflow sanamhub/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v0.2.0
```

`Verification succeeded!` means the file was built by `release.yml` in that repository, from the
commit of tag `v0.2.0`. Without `--signer-workflow`, any workflow of the repository that holds
`id-token: write` could have signed it, and without `--source-ref`, a run on any branch or tag.
Unpack the archive after that, and run `sluice`.

The attestation says where the file was built and from which commit. It is provenance, not a
statement that the file is safe. The CycloneDX SBOM is a file of the release, covered by the same
attestation; only the images carry SBOM attestations, one per architecture.

**The installers are a convenience, and check less.** The release also has `sluice-cli-installer.sh`
and `sluice-cli-installer.ps1`, which download the archive for the machine and unpack it. The shell
one compares the archive's SHA-256 with a value written into the script, and skips that on a machine
without `sha256sum`. The PowerShell one checks nothing. Neither checks a signature or an
attestation, so each is only as good as the copy of the script that was run. Prefer the archive, and
use an installer where those limits are acceptable. What `dist` 0.33.1 writes into them is in the
[threat model](threat-model.md#supply-chain).

If you are asked to pipe an unsigned binary into a shell from anywhere claiming to be this project,
it is not this project.

## Verify an image

An image is checked the same way, by its name in the registry. The version tag is the index of both
architectures, and `-amd64` and `-arm64` name each one:

```
gh attestation verify oci://ghcr.io/sanamhub/sluice-relay:0.2.0 --repo sanamhub/sluice \
  --signer-workflow sanamhub/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v0.2.0
gh attestation verify oci://ghcr.io/sanamhub/sluice-relay:0.2.0-arm64 --repo sanamhub/sluice \
  --signer-workflow sanamhub/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v0.2.0 \
  --predicate-type https://spdx.dev/Document/v2.3
```

The first checks the provenance of the index, and the second the SBOM of the arm64 image. The index,
`-amd64` and `-arm64` each have a provenance attestation, and each architecture has an SBOM. The
SPDX version in the predicate type is the one syft writes into the SBOM, which `actions/attest`
copies (`src/sbom.ts:85`).

**A tag is not the image.** `gh attestation verify` proves that `release.yml`, on that tag, built
the digest it is given. It does not say what the tag names now: no package can name one workflow, so
any workflow in a repository with access to the package and `packages: write` can move a tag. An
image is trusted only after its attestation passes and it is pulled by that digest:

```
digest="$(docker buildx imagetools inspect ghcr.io/sanamhub/sluice-relay:0.2.0 \
  --format '{{json .Manifest}}' | jq --raw-output .digest)"
gh attestation verify "oci://ghcr.io/sanamhub/sluice-relay@$digest" --repo sanamhub/sluice \
  --signer-workflow sanamhub/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v0.2.0
docker pull "ghcr.io/sanamhub/sluice-relay@$digest"
```

No package can name one workflow: GitHub's package settings grant Actions access to a repository,
not to a workflow file. The package's source label and the `release` environment say which jobs are
meant to push, not which ones can.

## What a binary carries

Each binary also carries its dependency tree from `cargo auditable`, which `cargo audit bin sluice`
checks against the RustSec database.
