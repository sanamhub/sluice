# Releasing Sluice

A release is the `sluice` binary of `sluice-cli` for six targets, a shell and a PowerShell
installer, checksums, a CycloneDX SBOM and GitHub artifact attestations, on a GitHub release. The
`rendezvous` and `relay` images go to ghcr.io from the same run, after the GitHub release exists,
each with its provenance and an SBOM per architecture attested. Nothing goes to crates.io.

## Install a release

Nothing has been released yet. When something is, this is how to install it: download the archive
for the machine, check where it came from, and only then run anything from it.

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

**The installers are a convenience, and check less.** The release also has `sluice-cli-installer.sh`
and `sluice-cli-installer.ps1`, which download the archive for the machine and unpack it. The shell
one compares the archive's SHA-256 with a value written into the script, and skips that on a
machine without `sha256sum`. The PowerShell one checks nothing. Neither checks a signature or an
attestation, so each is only as good as the copy of the script that was run. Prefer the archive,
and use an installer where those limits are acceptable. What `dist` 0.33.1 writes into them is in
the [threat model][threat-model].

## The pipeline

Two workflows do it:

| Workflow | Runs on | Does |
|---|---|---|
| `release-plz.yml` | every push to `main` | opens or updates the release pull request: the version bump and the new `CHANGELOG.md` section |
| `release.yml` | a tag `v<version>` | runs all of `ci.yml`, checks the tag, builds the binaries with `dist` and both images for amd64 and arm64, waits for every build and then for approval, publishes the GitHub release as a draft and then publishes it, then waits again and pushes and attests the images: each architecture first, then the version index |

**Who can release.** The tag ruleset's bypass list names **Repository admin**, and in a repository
owned by a user that is the owner alone. GitHub's ruleset page lists "Repository admins" as
eligible for bypass access, and its permissions page says a repository owned by a personal account
"has a single owner who has full control of the repository"
([About rulesets][about-rulesets],
[Permission levels][permission-levels]).
So the owner can tag a commit that is not on `main`, whose `release.yml` names no environment and
asks for whatever permissions it likes. Nothing in this repository stops that.

The approval protects against two things, and only those:

- a release started by anyone other than the owner, who needs the admin bypass and a reviewer the
  pusher cannot be;
- a mistake by the owner: a tag on the wrong commit, a tag pushed before the release pull request
  merged, or `.github/` changed on the tag. [The two checks
  below][checks] catch the last two, when there is a review to run them.

The owner can undo every setting that makes the review real. Ticking **Allow administrators to
bypass configured protection rules** again, removing the second reviewer from the `release`
environment, or unticking **Prevent self-review** gives the shipped `release.yml` a release with no
review at all: the owner tags `main` and it runs. The tag ruleset's bypass does the same for a
commit that is not on `main` ([above][who-can-release]). So the approval is a check on a release
started by someone else and on the owner's mistakes, and it is not a bar to the owner releasing
alone.

## What the maintainer approves, and where

1. **The release pull request.** Run its checks, review the version and the changelog section,
   and edit the changelog until each entry says what a user notices (the writing-style skill).
   Merging it is the first approval.
2. **The tag.** You push it by hand, and it is the one event that starts a release.
3. **The `release` environment, for the binaries.** The `host` job waits in it for a required
   reviewer: Actions > the run > **Review deployments** > tick `release` > **Approve and deploy**.
   It has to be someone other than whoever pushed the tag.
4. **The `release` environment, for the images.** The two `image-publish` jobs start only after
   `host` has made the GitHub release, so they reach the environment later and wait for a second
   review in the same run. They wait together, so one review approves both.

So one release asks for four approvals: the merge, the tag, and two reviews of the environment by
the second person, both within 35 days of the tag push ([why][cut-a-release]). The two reviews
were read from GitHub's documentation, which says each job that names an environment with required
reviewers waits for one, and have not been seen on a run yet.

### Checks before approving `host`

The run page names the commit being built. In a clone of this repository, with `main` fetched and
`<sha>` that full 40-character commit:

```
git fetch origin main
git merge-base --is-ancestor <sha> origin/main
git diff --exit-code origin/main <sha> -- .github/
```

Approve only when both exit 0 and the second prints nothing. Otherwise refuse, and say which
command failed.

- The first passes only when the tagged commit is on `main`, so the release was not started from a
  branch.
- The second passes only when the tagged commit's `.github/` is `main`'s. The `release.yml` that
  runs is the tagged commit's, so this is the check that the workflow which runs is the one that was
  reviewed, with the environment, the permissions and the publishing order it has on `main`. A
  diff is a refusal even when it looks harmless: the tag's copy is what runs, and the tag ruleset's
  bypass is what makes a different copy possible.

Two things these checks cannot do. They cannot catch a tag whose `release.yml` names no
environment: nobody is asked to approve, so nobody runs them, and that is the bypass
[above][who-can-release]. And they pass for a later commit on `main` that keeps `.github/` the
same, so they say the tag is on `main` with the workflow that was reviewed, not that the tag is the
merge commit of the release pull request.

### Re-runs and who approves

GitHub's page on reviewing deployments says that, when the environment prevents self-approval,
"you will not be able to approve a deployment from a workflow run you initiated"
([Reviewing deployments][reviewing-deployments]).
The contexts page says a re-run has two actors: `github.actor` is "the user that triggered the
initial workflow run" and `github.triggering_actor` is "the user that initiated the workflow run",
and the two may differ on a re-run
([Contexts][contexts]).

No page says which of the two "Prevent self-review" reads, and no run has shown it. So treat both
as barred: whoever pushed the tag or clicked **Re-run** does not approve that run. When the
maintainer pushed the tag and re-runs a failed job, the other person approves, and when the other
person re-runs it, the maintainer cannot count on being allowed to.

## The release pull request's checks

The release pull request starts with no checks. GitHub's documentation says why, on the three
pages in the table below. If `main` requires status checks, the pull request cannot merge until
they run. Run them on its branch by hand:

```
gh workflow run ci.yml --repo sanamhub/sluice \
  --ref "$(gh pr view <number> --repo sanamhub/sluice --json headRefName --jq .headRefName)"
```

| Page | What it says, quoted |
|---|---|
| [GITHUB_TOKEN][github-token] | "events triggered by the `GITHUB_TOKEN` will not create a new workflow run", and "`workflow_dispatch` and `repository_dispatch` events always create workflow runs" |
| [Events that trigger workflows][events] | for `workflow_dispatch`, `GITHUB_SHA` is the "Last commit on the `GITHUB_REF` branch or tag", and the event "will only trigger a workflow run if the workflow file exists on the default branch"; for `pull_request`, "By default, a workflow only runs when a `pull_request` event's activity type is `opened`, `synchronize`, or `reopened`" |
| [Troubleshooting required status checks][required-checks] | "Required checks must pass on the latest commit SHA" |

So a dispatched run starts despite the token, and runs on the pull request's head commit, which is
the commit the required checks are read from. No page says in one sentence that such a run
satisfies a required check: that is inferred from the three, and has not been seen on a run. It
also needs `ci.yml` with `workflow_dispatch` on `main` first.

If the merge box still shows the checks as expected after the run, close the pull request and
reopen it. A reopen by a person is a `pull_request` event with their token, which runs the checks
by the second quote on the events page. Pushing a changelog edit to the branch runs them the same
way.

`release.yml` runs all of `ci.yml` again on the tag and builds nothing until it passes, so a
release never skips the checks, even if the pull request merged without them.

## Cut a release

1. Wait for the release pull request, titled `chore: release v<version>`, to show the version you
   expect. release-plz works it out from the Conventional Commits since the last tag. Before 1.0 a
   breaking change bumps the minor version and anything else the patch. From 1.0 on, a breaking
   change bumps the major, `feat` the minor, and anything else the patch.
2. Run its checks ([above][pr-checks]). Edit its `CHANGELOG.md` section
   on the pull request branch if needed, and merge it right after, with nothing merged to `main`
   in between. When `main` moves, release-plz 0.3.174 closes a release pull request that holds a
   commit of anyone but itself, and opens a new one from `main` without that commit. Its source
   says so (`crates/release_plz_core/src/command/release_pr/mod.rs:303-307`):

   ```
   // There's a contributor, so we don't want to force-push in this PR.
   // We close it because we want to save the contributor's work.
   ...
   info!("closing pr {} to preserve git history", opened_pr.html_url);
   ```

   The closed pull request keeps the edit, so it can be copied over, but the new one starts
   without it.
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

   **What a failure leaves behind.** A failed build publishes nothing: if the build of either
   image fails on either architecture, `host` does not start, so no GitHub release, file or image
   is made public. Open the run and choose **Re-run failed jobs**: the builds that passed keep
   their artifacts (30 days for the binaries, 35 for the images), and `host` then waits for the
   reviewer again. If the re-run fails the same way, the cause is in the tagged commit: delete the
   tag, fix it on `main`, and tag the next patch version.

   If `host` itself fails after it created the draft, the draft is still there and the next run of
   `host` refuses to start while a release or draft for the tag exists. The refusal names the
   command: `gh release delete <tag> --yes`. That deletes the release or draft and leaves the tag,
   which is the one this run was started from. Then choose **Re-run failed jobs**: the tag is not
   pushed again, and `host` waits for the reviewer again.

   A push that fails after `host` is different. The GitHub release is already public, so a failed
   `image-publish` leaves it without images, or with fewer than all of them. The job pushes in this
   order, and a failure leaves what the table says:

   | Step that fails | Tags left public |
   |---|---|
   | `push-image.sh arch`, on either architecture | the architecture tags pushed before it, unattested |
   | an architecture's provenance or SBOM attestation | both architecture tags, partly attested |
   | `push-image.sh index` | both architecture tags, attested; the index only if it was written |
   | the index attestation | the index, unattested |

   Every step of a re-run writes its tag again: `docker push` replaces the manifest behind
   `<version>-<arch>`, and `docker buildx imagetools create --tag` replaces the index. So a re-run
   overwrites what the failed one left. Re-run that job from the run page within 30 days of the tag
   push, and the second reviewer approves it again.

   **Both limits are GitHub's.** A run is cancelled 35 days after it starts, and approval time
   counts: "If a workflow run reaches this limit, the workflow run is cancelled. This period
   includes execution duration, and time spent on waiting and approval"
   ([Limits][run-limits]). A re-run is possible only "up
   to 30 days after its initial run"
   ([Re-running workflows and jobs][re-running-runs]).
   An environment also waits at most 30 days for its approval: "If a job is not approved within 30
   days, it will automatically fail"
   ([Reviewing deployments][reviewing-deployments]).
   So `host` has to be approved within 30 days of when it starts waiting, and `image-publish`
   within 30 days of when it starts waiting, which is after `host` makes the release. Both
   approvals have to land within 35 days of the tag push, and a failed `image-publish` has to be
   re-run within 30 days of it. Past any of those limits the fix is a new patch version: the run
   and its artifacts are gone, and the version cannot be reused.

   **How `host` publishes.** It creates the GitHub release as a draft with every file attached, and
   marks it published in a last step. An immutable release refuses a file added later: "you cannot
   add, replace, or delete assets after a release is published"
   ([Managing releases][managing-releases]).
   The same page's advice is "create releases as drafts first, attach all assets, and then
   publish". Nothing about the files changes: the attestation names their bytes, not the release.

   **The `create` command already did that**, so the reason for the two steps is not that one
   `create` published first. On the `ubuntu-24.04` image of 2026-10-12, which is image version
   20261012.334.1, `gh` is 2.102.0, and `gh release create` with files and no `--draft` creates a
   draft, uploads the assets, and then publishes. Its source says so, in
   `pkg/cmd/release/create/create.go`:

   ```
   if hasAssets && !opts.Draft {
       ...
       draftWhileUploading = true
       params["draft"] = true
   }
   ```

   The comment there reads "Save the release initially as draft and publish it after all assets
   have finished uploading", and after the uploads:

   ```
   if draftWhileUploading {
       rel, err := publishRelease(httpClient, baseRepo.RepoHost(), ...)
       ...
   }
   ```

   The workflow writes the same sequence itself so that the draft is a step it can name: a job that
   fails after the draft exists leaves it for the operator to delete, as below, while `create`
   deletes its own draft when an upload fails. The publish is then one line after the attestation
   of every file.

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

   Then verify one archive as in [Install a release][install], and one image as below.

## Verify an image, and what a binary carries

Every file of the release has a provenance attestation signed through Sigstore, and
[the first section][install] shows how to check an archive.

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

**A tag is not the image.** `gh attestation verify` proves that `release.yml`, on that tag, built
the digest it is given. It does not say what the tag names now: [no package can name one
workflow][settings], so any workflow in a repository with access to
the package and `packages: write` can move a tag. An image is trusted only after its attestation
passes and it is pulled by that digest:

```
digest="$(docker buildx imagetools inspect ghcr.io/sanamhub/sluice-relay:0.2.0 \
  --format '{{json .Manifest}}' | jq --raw-output .digest)"
gh attestation verify "oci://ghcr.io/sanamhub/sluice-relay@$digest" --repo sanamhub/sluice \
  --signer-workflow sanamhub/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v0.2.0
docker pull "ghcr.io/sanamhub/sluice-relay@$digest"
```

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
- **Administrators must not bypass the `release` environment.** On the same page, Settings >
  Environments > `release`, untick **Allow administrators to bypass configured protection rules**.
  GitHub's documentation says the default is the opposite: "By default, administrators can bypass
  the protection rules and force deployments to specific environments"
  ([Deployments and environments][deployments]).
  The same page's steps for the setting read "Deselect **Allow administrators to bypass configured
  protection rules**"
  ([Managing environments][managing-environments]).
  The maintainer owns the repository, so with the box ticked they could force every pending
  `release` job through alone. Unticking it is what makes the environment's review real. It is not
  the only way around a second person, and none of the others asks anyone to approve: the owner can
  tick this box again, remove the second reviewer from the environment, or untick **Prevent
  self-review**, and the tag ruleset's bypass lets the owner tag a commit whose `release.yml` names
  no environment. [The two checks above][checks] run only when a review is
  asked for, so they catch a tag on the wrong commit or with a changed `.github/`, and not a
  setting the owner has changed.
  The audit log's environment events are documented for organizations only, and all three are
  "via the API", such as `environment.update_protection_rule`; a personal account has a security
  log instead. No page says either records this change, so it has not been checked: if the
  repository has no organization, the other person should look at the box on the settings page
  before approving.
- **Release immutability**: Settings > scroll down to the **Releases** section > tick **Enable
  release immutability**. GitHub's page says what it locks: "Immutable releases are releases where
  the assets and associated Git tag cannot be changed after publication", and "you cannot add,
  replace, or delete assets after a release is published, and you cannot move or delete its tag
  while the release exists"
  ([Immutable releases][immutable],
  [Managing releases][managing-releases]).
  It applies from the next release on: "immutability will only apply to future releases"
  ([Prevent release changes][prevent-changes]).
  Turn it on before the first release, or that one stays mutable.
- **No package can name one workflow.** GitHub's package settings grant Actions access to a
  repository, not to a workflow file: "you must give explicit access to the repository where the
  workflow is stored", and "You can give multiple repositories workflow access to a package"
  ([Configuring a package's access control][package-access]).
  There is no per-workflow switch in those docs, so any workflow in a repository with that access
  and `packages: write` can push to the package. A package's source label and the `release`
  environment say which jobs are meant to push, not which ones can. See [Verify an image, and what
  a binary carries][verify-image].
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

[checks]: #checks-before-approving-host
[cut-a-release]: #cut-a-release
[install]: #install-a-release
[settings]: #repository-settings-the-pipeline-needs
[pr-checks]: #the-release-pull-requests-checks
[verify-image]: #verify-an-image-and-what-a-binary-carries
[who-can-release]: #who-can-release
[threat-model]: docs/threat-model.md#supply-chain
[github-token]: https://docs.github.com/en/actions/concepts/security/github_token
[managing-environments]: https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/manage-environments
[reviewing-deployments]: https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/review-deployments
[re-running-runs]: https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs
[run-limits]: https://docs.github.com/en/actions/reference/limits
[contexts]: https://docs.github.com/en/actions/reference/workflows-and-actions/contexts
[deployments]: https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments
[events]: https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows
[immutable]: https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases
[prevent-changes]: https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/establish-provenance-and-integrity/prevent-release-changes
[package-access]: https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility
[required-checks]: https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/collaborating-on-repositories-with-code-quality-features/troubleshooting-required-status-checks
[about-rulesets]: https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets
[permission-levels]: https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/repository-access-and-collaboration/permission-levels-for-a-personal-account-repository
[managing-releases]: https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository
