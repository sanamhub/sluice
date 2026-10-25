# Security

## Reporting a vulnerability

Report privately through GitHub Security Advisories on this repository. Do not open a public issue.

You will get an acknowledgement within 72 hours and an assessment within seven days. We coordinate
disclosure with you and credit you unless you would rather not be named.

A report needs something we can reproduce: an input, a test, or exact steps. Reports without one are
closed.

## Supported versions

Before 1.0, only the latest release gets fixes. From 1.0, the latest minor release and the one
before it get fixes.

## What Sluice protects against

See [`docs/threat-model.md`](docs/threat-model.md), which also lists what it does not protect
against and its known weaknesses.

## Verifying a release

Every release is built in CI. Every file has a GitHub artifact attestation of provenance, and the
attestation says where the file was built and from which commit; it is provenance only, and not a
statement that the file is safe. The CycloneDX SBOM is a file of the release, covered by the same
provenance attestation; only the images carry SBOM attestations, one per architecture. Check the
archive before running anything from it:

```
gh attestation verify sluice-cli-x86_64-unknown-linux-gnu.tar.xz --repo <owner>/sluice \
  --signer-workflow <owner>/sluice/.github/workflows/release.yml \
  --source-ref refs/tags/v<version>
```

[`docs/install.md`](docs/install.md#download-and-verify-an-archive) says what the two flags add, and
what the installers check and do not.

If you are asked to pipe an unsigned binary into a shell from anywhere claiming to be this project,
it is not this project.
