# Releasing Sluice

A release is the `sluice` binary for six targets, a shell and a PowerShell installer, checksums, a
CycloneDX SBOM and GitHub artifact attestations, on a GitHub release. The `relay` image goes to
ghcr.io from the same run, after the release exists, with provenance and an SBOM per architecture
attested. Nothing goes to crates.io.

Installing and verifying a release is in [`docs/install.md`](../install.md). This runbook is for
making one.

## The pipeline

| Workflow | Runs on | Does |
|---|---|---|
| `release-plz.yml` | every push to `main` | opens or updates the release pull request with the version bump and the new changelog section |
| `release.yml` | a tag `v<version>` | runs all of `ci.yml`, checks the tag, builds six binaries and the relay image for amd64 and arm64, waits for every build and an approval, publishes the release as a draft and then publishes it, then pushes and attests the image: each architecture first, then the version index |

## Who releases

One person, the owner, makes a release. The `release` environment and the `release tags` ruleset
make that person stop before anything becomes public, and the checks below are what they run while
it is stopped. Neither setting adds a second person to the release. That is deliberate: the owner
releases alone, and the pause is a pause, not a review by someone else.

The owner of `sanamhub/sluice` is `@sanamhub`. GitHub's permissions page says a personal repository
"has a single owner who has full control" ([Permission levels][permission-levels]).

### The settings, as they are

- **The `release` environment.** Settings > Environments > `release`:
  - **Required reviewers** lists `sanamhub`, and **Prevent self-review** is off, so the owner may
    approve a deployment they started.
  - **Deployment branches and tags** is **Selected branches and tags**, with one policy: the tag
    `v*`.
  - **Allow administrators to bypass configured protection rules** is ticked
    (`can_admins_bypass: true`), so the owner may also skip the pause.
- **The `release tags` ruleset.** Settings > Rules > Rulesets > `release tags`: enforcement Active,
  target tags matching `refs/tags/v*`, and **Restrict creations**, **Restrict updates**,
  **Restrict deletions** and **Block force pushes** all on. The bypass list holds **Repository
  admin**, which in this repository is the owner, with bypass mode **Always**.
- **The repository owner.** Settings > General shows `sanamhub` as owner; the same login is the
  only entry in both settings above.

The API returns these values, so a reader can check them without the settings pages:
`gh api repos/sanamhub/sluice/environments/release` shows the reviewer, self-review, branch policy
and admin bypass, and `gh api repos/sanamhub/sluice/rulesets/<id>` shows the tag rules and bypass.
Find `<id>` with `gh api repos/sanamhub/sluice/rulesets`.

### What the pause protects against

- **A tag pushed by mistake.** Pushing the tag starts the run, and nothing is published until the
  owner approves `host`; the binary files, the release and the image all wait.
- **A tag on the wrong commit.** The checks below refuse a tag that is not on `main`, and one whose
  `.github/` differs from `main`'s, so a workflow that was not reviewed does not run.
- **A changed `.github/`.** `release.yml` runs from the tagged commit. The environment makes that
  commit's workflow wait, and the `.github/` check shows whether it is the one on `main`.
- **A release that starts itself.** No tag publishes anything on its own. The pause is between the
  tag and the first public file.

### What it does not do

- **It is not a second person.** The only reviewer is the owner, and **Prevent self-review** is off,
  so the same person who pushed the tag approves the deployment. Nothing here catches a mistake the
  owner does not notice.
- **It does not bind the owner.** The owner can change every setting above: the reviewer, self
  review, the deployment branches, the environment's admin bypass, and the ruleset's bypass or
  rules. Both bypasses are on now, so the owner can also step around the pause without changing it.
- **It does not catch every tag.** A commit whose `release.yml` names no environment asks nobody
  for an approval. The ruleset still limits who can create the tag, not what the tag's workflow
  says.
- **It does not prove the tag is the release pull request's merge commit.** The checks say the tag
  is on `main` with `main`'s `.github/`. A later commit on `main` that keeps `.github/` the same
  passes them too.

## What gets approved

A release has four points where the owner acts:

1. **The release pull request.** Run its checks, review the version and the changelog, and edit the
   changelog until each entry says what a user notices. Merging it is the first act.
2. **The tag.** Push it by hand; it is the one event that starts a release.
3. **The `release` environment, for the binaries.** `host` waits for an approval: Actions > the run
   > **Review deployments** > tick `release` > **Approve and deploy**.
4. **The `release` environment, for the image.** `image-publish` starts only after `host` has made
   the release, so it waits for a second approval in the same run. One approval covers the one
   image.

Each job that names the environment waits separately, so `host` and `image-publish` are approved
one after the other. Both approvals are the owner's, and both are within 35 days of the tag push
([why][cut-a-release]).

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

### Re-runs

With **Prevent self-review** off, the owner can approve a re-run as well, including one they
started. The checks above still apply to the commit the re-run builds, and the limits below still
apply to the run.

**The limits are GitHub's.** A run is cancelled 35 days after it starts, and approval time counts
([Limits][run-limits]). A re-run is possible only up to 30 days after the initial run ([Re-running
workflows and jobs][re-running-runs]). An environment waits at most 30 days for its approval
([Reviewing deployments][reviewing-deployments]). Past any of those limits the fix is a new patch
version: the run and its artifacts are gone, and the version cannot be reused.

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
   but letters, digits, `.` and `-` after the `v`. Then the six binary builds, the two image builds
   and the installers. `host` waits for all of them, and the owner approves it when they are green,
   then `image-publish` once the release is up.

**A failed build publishes nothing.** If the image fails on either architecture, `host` does not
start, so no file or image is public. Open the run and choose **Re-run failed jobs**: the builds
that passed keep their artifacts (30 days for binaries, 35 for images), and `host` waits for an
approval again. If the re-run fails the same way, the cause is in the tagged commit: delete the tag,
fix it on `main`, and tag the next patch version.

If `host` fails after creating the draft, the draft remains and the next `host` refuses to start
while a release or draft for the tag exists. It names the command: `gh release delete <tag> --yes`.
Then **Re-run failed jobs**, and approve again.

A failed push after `host` is different: the release is already public. This is the order, and what
a failure leaves:

   | Step that fails | Tags left public |
   |---|---|
   | `push-image.sh arch`, on either architecture | the architecture tags pushed before it, unattested |
   | an architecture's provenance or SBOM attestation | both architecture tags, partly attested |
   | `push-image.sh index` | both architecture tags, attested; the index only if it was written |
   | the index attestation | the index, unattested |

Every step of a re-run writes its tag again, so a re-run overwrites what the failed one left. Re-run
that job within 30 days of the tag push, and approve it again.

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

Then verify one archive and the image as in [`docs/install.md`](../install.md).

## Rehearsing a release

Run the whole pipeline once on a pre-release tag before the first release. `v0.1.0-rc.1` is a real
tag and a real GitHub release: it builds and publishes the binaries and the image under that
version, and then you delete them. Do this before `v0.1.0`, not after it.

**The tagged commit must carry the pre-release version.** `dist` 0.33.1 selects a package for a tag
only when the package's version equals the tag's version. A tag that matches no package stops with
`NothingToRelease` (`check_dist_package` and `require_releases` in [announce.rs][dist-announce]).
A `v0.1.0-rc.1` tag on a commit whose `Cargo.toml` still says `0.1.0` therefore builds nothing.
The rehearsal needs a version commit on `main`, and a second commit after it.

### The first commit: the rehearsal version

On `main`, make one commit with these contents:

- `Cargo.toml`: `[workspace.package] version = "0.1.0"` becomes `"0.1.0-rc.1"`.
- `Cargo.lock`: the three workspace entries, `sluice-cli`, `sluice-core` and `sluice-live`, change
  from `version = "0.1.0"` to `0.1.0-rc.1`. No dependency version changes.
- `CHANGELOG.md` is unchanged. There is no `0.1.0-rc.1` section, so the rehearsal's release notes are
  empty; that is what is being rehearsed, not the notes.

```
cargo check
git add Cargo.toml Cargo.lock
git commit -m "chore: rehearse v0.1.0-rc.1"
git push origin main
git tag -a v0.1.0-rc.1 -m "Sluice 0.1.0-rc.1"
git push origin v0.1.0-rc.1
```

### Run and approve it

The tag starts `release.yml` exactly as a final tag does. Approve `host` and then `image-publish`
under **Review deployments**, and run the same checks as for any release. Then check:

- The release page is marked **Pre-release**. `dist` reads that from the tag
  (`announcement_is_prerelease` in [announce.rs][dist-announce]), and `release.yml` passes
  `--prerelease` to `gh release create`.
- The release has the seven kinds of file in the table under **Cut a release**.
- The image is public under the version `0.1.0-rc.1`, with the three tags the pipeline writes:
  `0.1.0-rc.1`, `0.1.0-rc.1-amd64` and `0.1.0-rc.1-arm64`.

### Remove what the rehearsal published

Do this before the final release. A pre-release left in place would keep the version, its files and
its image tags public, and nothing in the final run deletes them.

```
gh release delete v0.1.0-rc.1 --repo sanamhub/sluice --cleanup-tag --yes
```

`--cleanup-tag` deletes `v0.1.0-rc.1` too; if it does not, `git push origin :refs/tags/v0.1.0-rc.1`
does, and the `release tags` ruleset lets the owner do that. Then open **Packages** >
`sluice-relay` > the three `0.1.0-rc.1` tags and delete each one. Their attestations stay in the
attestation store, naming digests no tag points at, which is harmless.

### The second commit: back to 0.1.0

Make the final version commit on `main`:

- `Cargo.toml`: `version = "0.1.0-rc.1"` becomes `"0.1.0"`.
- `Cargo.lock`: the three workspace entries go back to `0.1.0`.
- `CHANGELOG.md`: no change from you. release-plz adds the `0.1.0` section when the push runs the
  release workflow.

```
git add Cargo.toml Cargo.lock
git commit -m "chore: release v0.1.0"
git push origin main
```

Then wait for the release pull request, which now has the full changelog, merge it, and cut
`v0.1.0` as **Cut a release** says.

### What release-plz does with the pre-release tag

It ignores it. The tag pattern comes from `git_tag_name = "v{{ version }}"`, and release-plz 0.3.174
turns `{{ version }}` into `(\d+\.\d+\.\d+)` and anchors the whole tag, so `v0.1.0-rc.1` does not
match ([release_regex.rs][release-tag-regex]). After the second commit the current version is
`0.1.0`, and release-plz looks for `v0.1.0`, which does not exist yet; with no release to diff
against, it carries the changelog for the whole history, not only the commits after the rehearsal.
It does not propose a `0.1.0-rc.2`.

While the rehearsal version is on `main`, release-plz may open a pull request for `0.1.0-rc.1`,
because it sees no stable tag and the version was bumped. The second commit replaces it with the
`0.1.0` pull request; close the rc one if it is still open.

[dist-announce]: https://github.com/axodotdev/cargo-dist/blob/v0.33.1/cargo-dist/src/announce.rs
[release-tag-regex]: https://github.com/release-plz/release-plz/blob/release-plz-v0.3.174/crates/release_plz_core/src/release_regex.rs

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

The approval settings are under **Who releases**. These are the other settings a release needs:

- **Release immutability.** Settings > **Releases** > **Enable release immutability**. It applies
  from the next release on ([Prevent release changes][prevent-changes], [Immutable
  releases][immutable]). It is off now
  (`gh api repos/sanamhub/sluice/immutable-releases` says `"enabled": false`), and it should be on
  before the first release, or that one stays mutable.
- **Actions may open pull requests.** Settings > Actions > General > Workflow permissions >
  **Allow GitHub Actions to create and approve pull requests** is on. Without it release-plz cannot
  open its pull request.
- **The image package is public.** After the first run, open `sluice-relay` under **Packages**,
  then Package settings > **Change visibility** > Public. A new ghcr.io package starts private.
- **No package can name one workflow.** Package settings grant Actions access to a repository, not a
  workflow file ([Package access][package-access]), so any workflow in a repository with that access
  and `packages: write` can push. The source label and the environment say which jobs are meant to
  push, not which ones can. Check an image as
  [`docs/install.md`](../install.md#verify-an-image) shows.
- Attestations need a public repository, or a private one on GitHub Enterprise Cloud.

No secret is needed. Every workflow uses `GITHUB_TOKEN` with the narrowest permissions each job
needs.

## Package managers

Homebrew, Scoop, winget and the AUR are not published by the release workflow. `cargo xtask package`
writes their files from a release's own checksums, and [`packaging.md`](packaging.md) says what each
registry needs, how to check a file before publishing it, and which of those checks this machine can
and cannot run.

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
[reviewing-deployments]: https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/review-deployments
[re-running-runs]: https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs
[run-limits]: https://docs.github.com/en/actions/reference/limits
[events]: https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows
[immutable]: https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases
[prevent-changes]: https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/establish-provenance-and-integrity/prevent-release-changes
[package-access]: https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility
[permission-levels]: https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/repository-access-and-collaboration/permission-levels-for-a-personal-account-repository
[managing-releases]: https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository
