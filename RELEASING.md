# Releasing Sluice

A release is the `sluice` binary of `sluice-cli` for six targets, a shell and a PowerShell
installer, checksums, a CycloneDX SBOM and GitHub artifact attestations, on a GitHub release. The
`rendezvous` and `relay` images go to ghcr.io from the same run, after the GitHub release exists,
each with its provenance and an SBOM per architecture attested. Nothing goes to crates.io.

Two workflows do it:

| Workflow | Runs on | Does |
|---|---|---|
| `release-plz.yml` | every push to `main` | opens or updates the release pull request: the version bump and the new `CHANGELOG.md` section |
| `release.yml` | a tag `v<version>` | runs all of `ci.yml`, checks the tag, builds the binaries with `dist` and both images for amd64 and arm64, waits for every build and then for approval, publishes the GitHub release, then waits again and pushes and attests the images |

**A release needs a second person.** The required reviewer of the `release` environment approves
the publishing jobs, and "Prevent self-review" stops whoever pushed the tag from doing it. Until the
maintainer adds that person (see [the settings](#repository-settings-the-pipeline-needs)), no
release can be published.

## What the maintainer approves, and where

1. **The release pull request.** Review the version and the changelog section, and edit the
   changelog until each entry says what a user notices (the writing-style skill). Merging it is
   the first approval.
2. **The tag.** You push it by hand, and it is the one event that starts a release.
3. **The `release` environment, for the binaries.** The `host` job waits in it for a required
   reviewer: Actions > the run > **Review deployments** > tick `release` > **Approve and deploy**.
   It has to be someone other than whoever pushed the tag.
4. **The `release` environment, for the images.** The two `image-publish` jobs start only after
   `host` has made the GitHub release, so they reach the environment later and wait for a second
   review in the same run. They wait together, so one review approves both.

So one release asks for four approvals: the merge, the tag, and two reviews of the environment by
the second person. The two reviews were read from GitHub's documentation, which says each job that
names an environment with required reviewers waits for one, and have not been seen on a run yet.

The release pull request shows no checks. GitHub runs no workflows for a pull request that
`GITHUB_TOKEN` opens, so that a workflow cannot start itself in a loop. The checks run instead on
the tag: `release.yml` calls all of `ci.yml` first and builds nothing until it passes.

## Cut a release

1. Wait for the release pull request, titled `chore: release v<version>`, to show the version you
   expect. release-plz works it out from the Conventional Commits since the last tag. Before 1.0 a
   breaking change bumps the minor version and anything else the patch. From 1.0 on, a breaking
   change bumps the major, `feat` the minor, and anything else the patch.
2. Edit its `CHANGELOG.md` section on the pull request branch if needed, then merge it.
3. Tag the merge commit and push the tag:

   ```
   git switch main && git pull
   git tag -a v0.2.0 -m "Sluice 0.2.0"
   git push origin v0.2.0
   ```

   The version in the tag must be the one in `Cargo.toml`: `dist` refuses a tag that names no
   package version.
4. Watch the `release` run. The `ci` jobs come first, then `plan`, which refuses a tag with
   anything but letters, digits, `.` and `-` after the `v`. Then the six binary builds, the four
   image builds and the installers. `host` waits for all of them, and the reviewer approves it
   when they are green, then the two `image-publish` jobs once the GitHub release is up.

   **A release goes out whole or not at all.** If the build of either image fails on either
   architecture, `host` does not start, so no GitHub release, file or image is made public. The
   fix is to open the run and choose **Re-run failed jobs**: the builds that passed keep their
   artifacts for 30 days, and `host` then waits for the reviewer again. If the re-run fails the
   same way, the cause is in the tagged commit: delete the tag, fix it on `main`, and tag the next
   patch version.
5. Check the release page. It holds:

   | Files | How many |
   |---|---|
   | `sluice-cli-<target>.tar.xz` or `.zip`, one per target | 6 |
   | `sluice-cli-<target>.tar.xz.sha256` or `.zip.sha256`, one per archive | 6 |
   | `sluice-cli-installer.sh` and `sluice-cli-installer.ps1` | 2 |
   | `source.tar.gz` and `source.tar.gz.sha256` | 2 |
   | `sha256.sum`, the checksums of the archives, installers and source | 1 |
   | `sluice-cli.cdx.xml`, the SBOM | 1 |
   | `dist-manifest.json`, what dist built and where | 1 |

   Then verify one archive and one image as below.

## Verify a release

Every file of the release has a provenance attestation signed through Sigstore. Check one with the
GitHub CLI, from any machine, against the workflow and the tag that built it:

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

The first checks the provenance of the index, and the second the SBOM of the arm64 image. The
index, `-amd64` and `-arm64` each have a provenance attestation, and each architecture has an SBOM.
The SPDX version in the predicate type is the one syft writes into the SBOM, which `actions/attest`
copies (`src/sbom.ts:85`).

Each binary also carries its dependency tree from `cargo auditable`, which `cargo audit bin sluice`
checks against the RustSec database.

## Roll back

A release cannot be edited into a good one. Take it down and go back to the one before:

1. Delete the GitHub release and its tag:

   ```
   gh release delete v0.2.0 --repo sanamhub/sluice --cleanup-tag --yes
   ```

2. Mark the previous release as latest, so the installers' `latest` URLs point at it again:

   ```
   gh release edit v0.1.0 --repo sanamhub/sluice --latest
   ```

3. Delete the image tags of the bad version, `<version>`, `<version>-amd64` and `<version>-arm64`,
   under the repository's **Packages** page, or leave them and say in the next release which
   version replaces them.
4. Fix on `main` and release a new patch version. Never reuse a version number: someone may
   already have the deleted files, and their attestations still name it.

## Repository settings the pipeline needs

- **A tag ruleset, first.** The `release.yml` that runs is the one in the tagged commit, so a
  writer could tag a commit whose `release.yml` drops `environment: release`, and its jobs would
  publish with no review. A ruleset is the one thing that commit cannot change. Settings > Rules >
  Rulesets > New ruleset > **New tag ruleset**:
  - **Ruleset name** `release tags`, **Enforcement status** Active;
  - **Bypass list** > Add bypass > **Repository admin**, which in a repository owned by a user is
    the owner alone, so only the maintainer can bypass it;
  - **Target tags** > Add target > Include by pattern > `v*`;
  - **Rules**: tick **Restrict creations**, **Restrict updates** and **Restrict deletions**, and
    leave **Block force pushes** ticked.
- **The `release` environment**: Settings > Environments > New environment > `release`. Under
  **Deployment protection rules**, tick **Required reviewers** and add at least one person besides
  the maintainer, and tick **Prevent self-review**. Nothing can be released until that person is
  added. Under **Deployment branches and tags**, choose **Selected branches and tags** and add the
  tag rule `v*`.
- **Actions may open pull requests**: Settings > Actions > General > Workflow permissions > tick
  **Allow GitHub Actions to create and approve pull requests**. Without it release-plz cannot open
  the pull request, and its job fails.
- **The image packages are public**: after the first release run, open each of
  `sluice-rendezvous` and `sluice-relay` under the owner's **Packages**, then Package settings >
  **Change visibility** > Public. A new package on ghcr.io starts private. Its source label links
  it to this repository, which is what lets the next run's `GITHUB_TOKEN` push to it.
- Attestations need a public repository, or a private one on GitHub Enterprise Cloud.

No secret is needed. Every workflow uses `GITHUB_TOKEN`, with the narrowest permissions each job
needs.

## Not done yet

- **The binaries are not signed.** Windows SmartScreen can warn on the first run of `sluice.exe`,
  and macOS Gatekeeper blocks a binary downloaded in a browser until the user allows it. Azure
  Artifact Signing for Windows and notarization for macOS need accounts the maintainer does not
  have yet (`docs/design.md`, **Tooling**). The attestations prove where a file came from, which
  signing will not replace.
- **Nothing is on crates.io.** The crates are not ready to publish, and publishing would need a
  registry token, which is a secret. `release-plz.toml` turns publishing off.

## Change the pipeline

`release.yml` was generated by `dist` 0.33.1 and then changed by hand, so `dist-workspace.toml`
sets `allow-dirty = ["ci"]` and `dist` leaves the file alone. To move to a new `dist`:

1. Download the new `dist` from its GitHub release and check it against the release's
   `sha256.sum`. Set `cargo-dist-version` in `dist-workspace.toml`.
2. Take `"ci"` out of `allow-dirty`, run `dist generate`, and put `"ci"` back.
3. Diff the new `release.yml` against the old one, and put back each hand change listed at the
   top of the file. The actions come out pinned to the commits in `[dist.github-action-commits]`,
   without the version comments.
4. Set `DIST_VERSION` in `release.yml` and the version and hashes in
   `.github/release/install-dist.sh` from the new release's `sha256.sum`.
5. Check every pin with `git ls-remote`, run `actionlint`, and run `dist plan`.

To move an action on, change its commit in `[dist.github-action-commits]` and in every workflow
that uses it, with the new version in the comment.

To move release-plz on, set `RELEASE_PLZ_VERSION` in `release-plz.yml`, and the version and hash
in `.github/release/install-release-plz.sh`. Its releases have no checksum file, so take the hash
from the asset's `digest` in `gh api repos/release-plz/release-plz/releases/tags/<tag>`, and check
that a download hashes to the same value.
