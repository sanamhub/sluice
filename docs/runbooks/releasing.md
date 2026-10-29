# Releasing Sluice

A release is the `sluice` binary for six targets, a shell and a PowerShell installer, checksums, a
CycloneDX SBOM and GitHub artifact attestations, on a GitHub release. Two images go to ghcr.io from
the same run, after the release exists, with provenance and an SBOM per architecture attested:
`sluice-relay`, the server of ADR 0028, and `sluice`, the tool of ADR 0033. Nothing goes to
crates.io.

Installing and verifying a release is in [`docs/install.md`](../install.md). This runbook is for
making one.

## The pipeline

| Workflow | Runs on | Does |
|---|---|---|
| `release-plz.yml` | every push to `main` | opens or updates the release pull request with the version bump and the new changelog section |
| `release.yml` | a tag `v<version>` | runs all of `ci.yml`, checks the tag, builds six binaries and the relay and tool images for amd64 and arm64, waits for every build and an approval, publishes the release as a draft and then publishes it, then pushes and attests each image: each architecture first, then the version index |

## Who releases

One person, the owner, makes a release. The `release` environment and the two rulesets make that
person stop before anything becomes public, and the checks below are what they run while it is
stopped. None of them adds a second person: the owner releases alone, and the pause is a pause, not
a review by someone else. The owner of `sanamhub/sluice` is `@sanamhub`; GitHub's permissions page
says a personal repository "has a single owner who has full control" ([Permission levels][permission-levels]).

### The settings, as they are

- **The `release` environment.** Settings > Environments > `release`:
  - **Required reviewers** lists `sanamhub`, and **Prevent self-review** is off, so the owner may
    approve a deployment they started.
  - **Deployment branches and tags** is **Selected branches and tags**, with one policy: the tag
    `v*`.
  - **Allow administrators to bypass configured protection rules** is off
    (`can_admins_bypass: false`), so no one, the owner included, can skip the pause through the
    environment. The owner can change that setting, as they can change every setting here.
- **The `release tags` ruleset.** Settings > Rules > Rulesets > `release tags`: enforcement Active,
  target tags matching `refs/tags/v*`, and **Restrict creations**, **Restrict updates**,
  **Restrict deletions** and **Block force pushes** all on. The bypass list holds **Repository
  admin**, which in this repository is the owner, with bypass mode **Always**.
- **The `main` ruleset.** Settings > Rules > Rulesets > `main`: enforcement Active, target the
  default branch, and no bypass actors. It blocks deletion and force pushes, requires a pull
  request, and requires every check `ci.yml` reports to pass: `fmt`, `clippy` and `test` on three
  systems, `msrv`, `wasm`, `deny`, `relay-guard`, `workflows`, and `cold-start` on two. A pull
  request needs no approving review, because one person maintains the repository, and merges are
  rebase or squash only. The owner is subject to it too, so `main` moves only through a pull
  request.
- **The repository owner.** Settings > General shows `sanamhub` as owner; the same login is the
  only entry in both settings above.

The API returns these values: `gh api repos/sanamhub/sluice/environments/release` and a ruleset's
`gh api repos/sanamhub/sluice/rulesets/<id>`, where `<id>` comes from `gh api repos/sanamhub/sluice/rulesets`.

### What the pause protects against

- **A tag pushed by mistake.** Nothing is published until the owner approves `host`; the files,
  the release and both images wait.
- **A tag on the wrong commit.** The checks below refuse a tag that is not on `main`, or whose
  `.github/` differs from `main`'s.
- **A changed `.github/`.** `release.yml` runs from the tagged commit, and the environment makes
  that commit's workflow wait.
- **A release that starts itself.** No tag publishes anything on its own.

### What it does not do

- **It is not a second person.** The only reviewer is the owner, and **Prevent self-review** is off,
  so the person who pushed the tag approves the deployment. Nothing catches a mistake the owner
  does not notice.
- **It does not bind the owner.** The owner can change every setting above. The environment's
  bypass is off, so the pause cannot be skipped without changing that setting first. The `main`
  ruleset has no bypass at all, so even the owner cannot push to `main`; the tag ruleset's bypass is
  what lets the owner create the release tag.
- **It does not catch every tag.** A commit whose `release.yml` names no environment asks nobody
  for an approval. The ruleset limits who can create the tag, not what the tag's workflow says.
- **It does not prove the tag is the release pull request's merge commit.** The checks say the tag
  is on `main` with `main`'s `.github/`, which a later commit that keeps `.github/` the same also
  passes.

## What gets approved

1. **The release pull request.** Run its checks, review the version and changelog, and edit the
   changelog until each entry says what a user notices. Merging it is the first act.
2. **The tag.** Push it by hand; it starts a release.
3. **The `release` environment, for the release files.** `host` waits for an approval: Actions >
   the run > **Review deployments** > tick `release` > **Approve and deploy**.
4. **The `release` environment, for each image.** `image-publish` starts only after `host` has made
   the release. It has one job for `sluice-relay` and one for `sluice`, and both wait in the
   environment, so a run has three deployments to approve, all within 35 days of the tag push
   ([why][cut-a-release]).

### Checks before approving `host`

The run page names the commit being built. In a clone, with `main` fetched and `<sha>` the full
40-character commit:

```
git fetch origin main
git merge-base --is-ancestor <sha> origin/main
git diff --exit-code origin/main <sha> -- .github/
```

Approve only when both exit 0 and the second prints nothing. Otherwise refuse and say which command
failed. The first passes only when the tag is on `main`; the second only when its `.github/` is
`main`'s, so the workflow that runs is the one reviewed.

With **Prevent self-review** off, the owner can approve a re-run, including one they started. The
checks above still apply to the commit it builds.

**The limits are GitHub's.** A run is cancelled 35 days after it starts, and approval time counts
([Limits][run-limits]). A re-run is possible only up to 30 days after the initial run ([Re-running
workflows and jobs][re-running-runs]). An environment waits at most 30 days for its approval
([Reviewing deployments][reviewing-deployments]). Past any limit the fix is a new patch version.

## Checks on the release pull request

The release pull request starts with no checks, because events triggered by `GITHUB_TOKEN` create no
workflow run, and a `pull_request` workflow runs only on `opened`, `synchronize` or `reopened`
([GITHUB_TOKEN][github-token], [Events][events]). Run them on its branch by hand:

```
gh workflow run ci.yml --repo sanamhub/sluice \
  --ref "$(gh pr view <number> --repo sanamhub/sluice --json headRefName --jq .headRefName)"
```

That runs on the head commit the required checks are read from. No page says it satisfies a required
check; that is inferred and has not been seen on a run. It needs `ci.yml` with `workflow_dispatch` on
`main` first. If the merge box still shows expected checks, close and reopen the pull request, or
push a changelog edit.

`release.yml` runs all of `ci.yml` again on the tag and builds nothing until it passes, so a release
never skips the checks even if the pull request merged without them.

## Cut a release

1. Wait for the release pull request, titled `chore: release v<version>`, to show the version you
   expect. Before 1.0 a breaking change bumps the minor and anything else the patch; from 1.0, the
   major and the minor.
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

The tag's version must be the one in `Cargo.toml`: `dist` refuses a tag that names no package
version.
4. Watch the `release` run. The `ci` jobs come first, then `plan`, which refuses a tag with anything
   but letters, digits, `.` and `-` after the `v`. Then the six binary builds, the relay and tool
   builds on both architectures, and the installers. `host` waits for all of them, and the owner
   approves it when they are green, then each image job once the release is up.

**A failed build publishes nothing.** If either image fails on either architecture, `host` does not
start, so no file or image is public. Open the run and choose **Re-run failed jobs**: passed builds
keep their artifacts (30 days for binaries, 35 for images), and `host` waits for an approval again.
If the re-run fails the same way, the cause is in the tagged commit: delete the tag, fix it on
`main`, and tag the next patch version.

If `host` fails after creating the draft, the draft remains and the next `host` refuses to start
while a release or draft for the tag exists. It names the command: `gh release delete <tag> --yes`.
Then **Re-run failed jobs**, and approve again.

A failed push after `host` is different: the release is already public. This is the order, and what
a failure leaves, for each image:

   | Step that fails | Tags left public, for that image |
   |---|---|
   | `push-image.sh arch`, on either architecture | the architecture tags pushed before it, unattested |
   | an architecture's provenance or SBOM attestation | both architecture tags, partly attested |
   | `push-image.sh index` | both architecture tags, attested; the index only if it was written |
   | the index attestation | the index, unattested |

Every step of a re-run writes the tag again, so a re-run overwrites what the failed one left. Re-run
that job within 30 days of the tag push, and approve it again. A failure in one image's job does not
stop the other: each row of the matrix retries on its own.

**How `host` publishes.** It creates the release as a draft with every file attached, then marks it
published. An immutable release refuses a file added later ([Managing releases][managing-releases]).
A job that fails after the draft exists leaves it for the operator to delete.
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

Then verify one archive and both images as in [`docs/install.md`](../install.md). The images are
not files of the release; they are the two ghcr.io packages, each with the three tags below.

## Rehearsing a release

Run the whole pipeline once on a pre-release tag before the first release. `v0.1.0-rc.1` is a real
tag and a real GitHub release: it builds and publishes the binaries and both images under that
version, and then you delete them. Do this before `v0.1.0`, not after it.

**The tagged commit must carry the pre-release version.** `dist` 0.33.1 selects a package for a tag
only when the package's version equals the tag's version. A tag that matches no package stops with
`NothingToRelease` (`check_dist_package` and `require_releases` in [announce.rs][dist-announce]).
A `v0.1.0-rc.1` tag on a commit whose `Cargo.toml` still says `0.1.0` therefore builds nothing.
The rehearsal needs a version commit on `main`, and a second commit after it. Both go through pull
requests, not a push.

### The first commit: the rehearsal version

Make one commit on a branch with these contents:

- `Cargo.toml`: `[workspace.package] version = "0.1.0"` becomes `"0.1.0-rc.1"`.
- `Cargo.lock`: the three workspace entries, `sluice-cli`, `sluice-core` and `sluice-live`, change
  from `version = "0.1.0"` to `0.1.0-rc.1`. No dependency version changes.
- `CHANGELOG.md` is unchanged. There is no `0.1.0-rc.1` section, so the rehearsal's release notes are
  empty; that is what is being rehearsed, not the notes.

```
git switch main && git pull
git switch -c rehearse-0.1.0-rc.1
cargo check
git add Cargo.toml Cargo.lock
git commit -m "chore: rehearse v0.1.0-rc.1"
git push -u origin rehearse-0.1.0-rc.1
gh pr create --repo sanamhub/sluice --base main --title "chore: rehearse v0.1.0-rc.1"
```

Wait for the required checks, then merge with a rebase or a squash, the two methods the `main`
ruleset allows:

```
gh pr checks --repo sanamhub/sluice --watch
gh pr merge --repo sanamhub/sluice --squash --delete-branch
git switch main && git pull
```

Tag the merge commit and push it; the `release tags` ruleset lets the owner create the tag.

```
git tag -a v0.1.0-rc.1 -m "Sluice 0.1.0-rc.1"
git push origin v0.1.0-rc.1
```

### Run and approve it

The tag starts `release.yml` exactly as a final tag does. Approve `host` and then the two image
jobs, `sluice-relay` and `sluice`, under **Review deployments**, and run the same checks as for any
release. Then check:

- The release page is marked **Pre-release**, from `announcement_is_prerelease` in
  [announce.rs][dist-announce].
- Both images are public under `0.1.0-rc.1`, each with `0.1.0-rc.1`, `-amd64` and `-arm64`.

### Remove what the rehearsal published

A pre-release left in place would keep the version, its files and its image tags public, and
nothing in the final run deletes them.

```
gh release delete v0.1.0-rc.1 --repo sanamhub/sluice --cleanup-tag --yes
```

`--cleanup-tag` deletes `v0.1.0-rc.1` too; if it does not, `git push origin :refs/tags/v0.1.0-rc.1`
does, and the tag ruleset lets the owner do that. Then open **Packages**, choose `sluice-relay`, and
delete its three `0.1.0-rc.1` tags. Do the same under `sluice`. Their attestations stay in the
attestation store, naming digests no tag points at, which is harmless.

### The second commit: back to 0.1.0

Make the final version commit on a branch, the same way:

- `Cargo.toml`: `version = "0.1.0-rc.1"` becomes `"0.1.0"`.
- `Cargo.lock`: the three workspace entries go back to `0.1.0`.
- `CHANGELOG.md`: no change from you. release-plz adds the `0.1.0` section when the push runs its
  workflow.

```
git switch main && git pull
git switch -c release-0.1.0
git add Cargo.toml Cargo.lock
git commit -m "chore: release v0.1.0"
git push -u origin release-0.1.0
gh pr create --repo sanamhub/sluice --base main --title "chore: release v0.1.0"
gh pr checks --repo sanamhub/sluice --watch
gh pr merge --repo sanamhub/sluice --squash --delete-branch
```

Then wait for the release pull request, which now has the full changelog, merge it, and cut
`v0.1.0` as **Cut a release** says.

### What release-plz does with the pre-release tag

It ignores it: `git_tag_name = "v{{ version }}"` becomes the anchored `v(\d+\.\d+\.\d+)`, so
`v0.1.0-rc.1` does not match ([release_regex.rs][release-tag-regex]). After the second commit it
looks for `v0.1.0`, which does not exist, so it carries the whole changelog, not an rc.2. It may
open an rc.1 pull request while that version is on `main`; close it if the second commit does not.

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

3. Delete the bad version's tags in both packages: `<version>`, `<version>-amd64` and
   `<version>-arm64` under **Packages** > `sluice-relay`, then the same three under `sluice`. Or
   leave them and say in the next release which version replaces them.
4. Fix on `main` through a pull request and release a new patch version. Never reuse a version
   number: someone may already have the deleted files, and their attestations still name it.

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
- **Both image packages are public.** After the first run, open `sluice-relay` under **Packages**,
  then Package settings > **Change visibility** > Public, and do the same for `sluice`. A new
  ghcr.io package starts private.
- **No package can name one workflow.** Package settings grant Actions access to a repository, not a
  workflow file ([Package access][package-access]), so any workflow with that access and
  `packages: write` can push. The source label and the environment say which jobs are meant to, not
  which ones can. Check either image as [`docs/install.md`](../install.md#verify-an-image) shows.
- Attestations need a public repository, or a private one on GitHub Enterprise Cloud.

No secret is needed. Every workflow uses `GITHUB_TOKEN` with the narrowest permissions each job
needs.

## Package managers

Homebrew, Scoop, winget and the AUR are not published by the release workflow. `cargo xtask package`
writes their files from a release's checksums; [`packaging.md`](packaging.md) says what each registry
needs and how to check a file first.

## Not done yet

- **The binaries are not signed.** Windows SmartScreen can warn on the first run, and macOS
  Gatekeeper blocks a binary downloaded in a browser until the user allows it. Signing and
  notarization need accounts the maintainer does not have ([`docs/design.md`](../design.md),
  **Tooling**). Attestations prove where a file came from, which signing will not replace.
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
