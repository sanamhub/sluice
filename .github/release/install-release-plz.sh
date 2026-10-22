#!/usr/bin/env bash
# Installs release-plz at the exact version release-plz.yml names, from its GitHub release, and
# refuses an archive whose SHA-256 is not the one written here. The release publishes no checksum
# file, so the hash is the digest GitHub's API gives the asset, and a download on 2026-10-22 hashed
# to the same value. release-plz/action takes the binary through cargo-binstall with no hash of
# ours, which is why this script replaces it.
#
# To move release-plz on: change RELEASE_PLZ_VERSION in release-plz.yml and paste the new asset's
# digest below, from `gh api repos/release-plz/release-plz/releases/tags/release-plz-v<version>`.
set -euo pipefail

version="${RELEASE_PLZ_VERSION:?RELEASE_PLZ_VERSION is not set}"
if [ "$version" != "0.3.174" ]; then
  echo "install-release-plz.sh has the hash of release-plz 0.3.174, and RELEASE_PLZ_VERSION is" \
    "$version" >&2
  exit 1
fi

# release-plz.yml runs on ubuntu-latest only, so only that runner's build is listed.
case "${RUNNER_OS}-${RUNNER_ARCH}" in
  Linux-X64)
    asset=release-plz-x86_64-unknown-linux-gnu.tar.gz
    sha=411433a5e3bc833140dcab89f4ef482d6390298f72c6c15a958aef4e98003438 ;;
  *)
    echo "no release-plz hash for ${RUNNER_OS}-${RUNNER_ARCH}" >&2
    exit 1 ;;
esac

dir="${RUNNER_TEMP}/release-plz-install"
mkdir -p "$dir/bin"
cd "$dir"
curl --proto '=https' --tlsv1.2 -fsSLO \
  "https://github.com/release-plz/release-plz/releases/download/release-plz-v${version}/${asset}"
echo "${sha}  ${asset}" | sha256sum -c -
tar -xzf "$asset" -C bin release-plz
echo "$dir/bin" >> "$GITHUB_PATH"
"$dir/bin/release-plz" --version
