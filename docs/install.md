# Install

Nothing has been released yet. When there is a release, download the archive for your computer,
check where it came from, and only then run it.

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

## What a binary carries

Each binary also carries its dependency tree from `cargo auditable`, which `cargo audit bin sluice`
checks against the RustSec database.
