# Releasing Sluice

A release is the `sluice` binary for six targets, a shell and a PowerShell installer, checksums, a
CycloneDX SBOM and GitHub artifact attestations, on a GitHub release. The `rendezvous` and `relay`
images go to ghcr.io from the same run, after the release exists, each with provenance and an SBOM
per architecture attested. Nothing goes to crates.io.

Installing and verifying a release is in [`docs/install.md`](../install.md). This runbook is for
making one.

## The pipeline

| Workflow | Runs on | Does |
|---|---|---|
| `release-plz.yml` | every push to `main` | opens or updates the release pull request with the version bump and the new changelog section |
| `release.yml` | a tag `v<version>` | runs all of `ci.yml`, checks the tag, builds six binaries and both images for amd64 and arm64, waits for every build and an approval, publishes the release as a draft and then publishes it, then pushes and attests the images: each architecture first, then the version index |

## Who can release

The tag ruleset's bypass list names **Repository admin**, and in a repository owned by a user that
is the owner alone. GitHub's ruleset page lists "Repository admins" as eligible for bypass, and its
permissions page says a personal repository "has a single owner who has full control" ([About
rulesets][about-rulesets], [Permission levels][permission-levels]). The owner can therefore tag a
commit that is not on `main`, whose `release.yml` names no environment and asks for any permissions.
Nothing in this repository stops that.

The approval protects against two things: a release started by anyone other than the owner, and a
mistake by the owner, such as a tag on the wrong commit, a tag pushed before the release pull
request merged, or a changed `.github/`. The two checks below catch the last two when a review runs.

The owner can undo every setting that makes the review real: allow administrators to bypass again,
remove the second reviewer, or untick **Prevent self-review**. Then a tag on `main` runs with no
review. So the approval is a check on someone else's release and on the owner's mistakes, not a bar
to the owner releasing alone.

## What gets approved

1. **The release pull request.** Run its checks, review the version and the changelog, and edit the
   changelog until each entry says what a user notices. Merging it is the first approval.
2. **The tag.** Push it by hand; it is the one event that starts a release.
3. **The `release` environment, for the binaries.** `host` waits for a required reviewer: Actions >
   the run > **Review deployments** > tick `release` > **Approve and deploy**. It has to be someone
   other than whoever pushed the tag.
4. **The `release` environment, for the images.** The two `image-publish` jobs start only after
   `host` has made the release, so they wait for a second review in the same run. One review
   approves both.

One release asks for four approvals: the merge, the tag, and two reviews by the second person, both
within 35 days of the tag push ([why][cut-a-release]). GitHub's documentation says each job that
names an environment with required reviewers waits for one; that has not been seen on a run yet.

### Checks before approving `host`

The run page names the commit being built. In a clone, with `main` fetched and `<sha>` the full
40-character commit:

```
git fetch origin main
git merge-base --is-ancestor <sha> origin/main
git diff --exit-code origin/main <sha> -- .github/
```

Approve only when both exit 0 and the second prints nothing. Otherwise refuse, and say which command
failed. The first passes only when the tagged commit is on `main`. The second passes only when its
`.github/` is `main`'s, which is the check that the workflow that runs is the one that was reviewed.

Two things these checks cannot do. They cannot catch a tag whose `release.yml` names no environment:
nobody is asked to approve, so nobody runs them. And they pass for a later commit on `main` that
keeps `.github/` the same, so they say the tag is on `main` with the reviewed workflow, not that it
is the release pull request's merge commit.

### Re-runs and who approves

GitHub says that when an environment prevents self-approval, "you will not be able to approve a
deployment from a workflow run you initiated" ([Reviewing deployments][reviewing-deployments]). A
re-run has two actors: `github.actor` is "the user that triggered the initial workflow run" and
`github.triggering_actor` is "the user that initiated the workflow run"; they may differ
([Contexts][contexts]).

No page says which one **Prevent self-review** reads, and no run has shown it. Treat both as barred:
whoever pushed the tag or clicked **Re-run** does not approve that run.

## Checks on the release pull request

The release pull request starts with no checks. GitHub's documentation says why: events triggered by
`GITHUB_TOKEN` do not create a new workflow run, while `workflow_dispatch` always does, and a
`pull_request` workflow runs only on `opened`, `synchronize` or `reopened`
([GITHUB_TOKEN][github-token], [Events][events]). So run the checks on its branch by hand:

```
gh workflow run ci.yml --repo sanamhub/sluice \
  --ref "$(gh pr view <number> --repo sanamhub/sluice --json headRefName --jq .headRefName)"
```

That runs on the pull request's head commit, which is the commit required checks are read from. No
page says in one sentence that it satisfies a required check; that is inferred and has not been seen
on a run. It also needs `ci.yml` with `workflow_dispatch` on `main` first. If the merge box still
shows expected checks, close the pull request and reopen it, or push a changelog edit to the branch.

`release.yml` runs all of `ci.yml` again on the tag and builds nothing until it passes, so a release
never skips the checks even if the pull request merged without them.

## Cut a release

1. Wait for the release pull request, titled `chore: release v<version>`, to show the version you
   expect. Before 1.0 a breaking change bumps the minor version and anything else the patch; from
   1.0 on, a breaking change bumps the major, `feat` the minor, and anything else the patch.
2. Run its checks, edit the `CHANGELOG.md` section if needed, and merge it right after, with nothing
   merged to `main` in between. When `main` moves, release-plz 0.3.174 closes a release pull request
   that holds a commit of anyone but itself and opens a new one without that commit, to preserve the
   contributor's work (`crates/release_plz_core/src/command/release_pr/mod.rs:303-307`).
3. Tag the merge commit and push the tag:

   ```
   git switch main && git pull
   git tag -a v0.2.0 -m "Sluice 0.2.0"
   git push origin v0.2.0
   ```

The version in the tag must be the one in `Cargo.toml`: `dist` refuses a tag that names no package
version.
4. Watch the `release` run. The `ci` jobs come first, then `plan`, which refuses a tag with anything
   but letters, digits, `.` and `-` after the `v`. Then the six binary builds, the four image builds
   and the installers. `host` waits for all of them, and the approver approves it when they are
   green, then the two `image-publish` jobs once the release is up.

**A failed build publishes nothing.** If either image fails on either architecture, `host` does not
start, so no file or image is public. Open the run and choose **Re-run failed jobs**: the builds
that passed keep their artifacts (30 days for binaries, 35 for images), and `host` waits for the
reviewer again. If the re-run fails the same way, the cause is in the tagged commit: delete the tag,
fix it on `main`, and tag the next patch version.

If `host` fails after creating the draft, the draft remains and the next `host` refuses to start
while a release or draft for the tag exists. It names the command: `gh release delete <tag> --yes`.
Then **Re-run failed jobs**, and the approver approves again.

A failed push after `host` is different: the release is already public. This is the order, and what
a failure leaves:

   | Step that fails | Tags left public |
   |---|---|
   | `push-image.sh arch`, on either architecture | the architecture tags pushed before it, unattested |
   | an architecture's provenance or SBOM attestation | both architecture tags, partly attested |
   | `push-image.sh index` | both architecture tags, attested; the index only if it was written |
   | the index attestation | the index, unattested |

Every step of a re-run writes its tag again, so a re-run overwrites what the failed one left. Re-run
that job within 30 days of the tag push, and the second reviewer approves it again.

**The limits are GitHub's.** A run is cancelled 35 days after it starts, and approval time counts
([Limits][run-limits]). A re-run is possible only up to 30 days after the initial run ([Re-running
workflows and jobs][re-running-runs]). An environment waits at most 30 days for its approval
([Reviewing deployments][reviewing-deployments]). Past any of those limits the fix is a new patch
version: the run and its artifacts are gone, and the version cannot be reused.

**How `host` publishes.** It creates the release as a draft with every file attached, and marks it
published in a last step. An immutable release refuses a file added later ([Managing
releases][managing-releases]). `gh release create` already does the same at 2.102.0, but the
workflow writes the steps out so the draft is one it can name: a job that fails after the draft
exists leaves it for the operator to delete.
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

Then verify one archive and one image as in [`docs/install.md`](../install.md).

## Roll back

A release cannot be edited into a good one. Take it down and go back to the one before:

1. Delete the release and its tag:

   ```
   gh release delete v0.2.0 --repo sanamhub/sluice --cleanup-tag --yes
   ```

2. Mark the previous release as latest, so the installers' `latest` URLs point at it again:

   ```
   gh release edit v0.1.0 --repo sanamhub/sluice --latest
   ```

3. Delete the image tags of the bad version, `<version>`, `<version>-amd64` and `<version>-arm64`,
   under the repository's **Packages** page, or leave them and say in the next release which version
   replaces them.
4. Fix on `main` and release a new patch version. Never reuse a version number: someone may already
   have the deleted files, and their attestations still name it.

## Repository settings

- **A tag ruleset, first.** `release.yml` runs from the tagged commit, so a writer could tag a
  commit whose workflow drops `environment: release`. A ruleset is the one thing that commit cannot
  change. Settings > Rules > Rulesets > New ruleset > **New tag ruleset**: name `release tags`,
  enforcement Active; bypass list **Repository admin**; target tags `v*`; tick **Restrict
  creations**, **Restrict updates** and **Restrict deletions**, and leave **Block force pushes**
  ticked.
- **The `release` environment.** Settings > Environments > New environment > `release`. Tick
  **Required reviewers**, add at least one person besides the maintainer, and tick **Prevent
  self-review**. Under **Deployment branches and tags**, choose **Selected branches and tags** and
  add `v*`. Nothing can be released until that person is added.
- **Administrators must not bypass the environment.** On the same page, untick **Allow
  administrators to bypass configured protection rules**. The default is the opposite ([Deployments
  and environments][deployments], [Managing environments][managing-environments]), and with it
  ticked the owner could force every pending job through alone. The audit log's environment events
  are documented for organizations only; a personal account has a security log. No page says either
  records this change, so the other person should look at the box before approving.
- **Release immutability.** Settings > **Releases** > tick **Enable release immutability**. It
  applies from the next release on ([Prevent release changes][prevent-changes], [Immutable
  releases][immutable]). Turn it on before the first release, or that one stays mutable.
- **No package can name one workflow.** Package settings grant Actions access to a repository, not a
  workflow file ([Package access][package-access]), so any workflow in a repository with that access
  and `packages: write` can push. The source label and the environment say which jobs are meant to
  push, not which ones can. Check an image as [`docs/install.md`](../install.md#verify-an-image)
  shows.
- **Actions may open pull requests.** Settings > Actions > General > Workflow permissions > tick
  **Allow GitHub Actions to create and approve pull requests**. Without it release-plz cannot open
  its pull request.
- **The image packages are public.** After the first run, open `sluice-rendezvous` and
  `sluice-relay` under **Packages**, then Package settings > **Change visibility** > Public. A new
  ghcr.io package starts private.
- Attestations need a public repository, or a private one on GitHub Enterprise Cloud.

No secret is needed. Every workflow uses `GITHUB_TOKEN` with the narrowest permissions each job
needs.

## Not done yet

- **The binaries are not signed.** Windows SmartScreen can warn on the first run, and macOS
  Gatekeeper blocks a binary downloaded in a browser until the user allows it. Azure Artifact
  Signing and notarization need accounts the maintainer does not have
  ([`docs/design.md`](../design.md), **Tooling**). Attestations prove where a file came from, which
  signing will not replace.
- **Nothing is on crates.io.** The crates are not ready to publish, and publishing needs a registry
  token. `release-plz.toml` turns publishing off.

## Change the pipeline

`release.yml` was generated by `dist` 0.33.1 and changed by hand, so `dist-workspace.toml` sets
`allow-dirty = ["ci"]` and `dist` leaves it alone. To move to a new `dist`:

1. Download it from its GitHub release and check it against `sha256.sum`. Set `cargo-dist-version`
   in `dist-workspace.toml`.
2. Take `"ci"` out of `allow-dirty`, run `dist generate`, and put `"ci"` back.
3. Diff the new `release.yml` against the old one, and put back each hand change listed at the top
   of the file. The actions come out pinned to the commits in `[dist.github-action-commits]`,
   without the version comments.
4. Set `DIST_VERSION` in `release.yml` and the version and hashes in
   `.github/release/install-dist.sh` from the new release's `sha256.sum`.
5. Check every pin with `git ls-remote`, run `actionlint`, and run `dist plan`.

To move an action on, change its commit in `[dist.github-action-commits]` and in every workflow that
uses it, with the new version in the comment.

To move release-plz on, set `RELEASE_PLZ_VERSION` in `release-plz.yml`, and the version and hash in
`.github/release/install-release-plz.sh`. Its releases have no checksum file, so take the hash from
the asset's `digest` in `gh api repos/release-plz/release-plz/releases/tags/<tag>`, and check that a
download hashes to the same value.

[cut-a-release]: #cut-a-release
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
[about-rulesets]: https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets
[permission-levels]: https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/repository-access-and-collaboration/permission-levels-for-a-personal-account-repository
[managing-releases]: https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository
