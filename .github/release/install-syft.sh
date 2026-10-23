#!/usr/bin/env bash
# Installs syft at the exact version release.yml names, from its GitHub release, and refuses an
# archive whose SHA-256 is not the one written here. The hashes are that release's
# syft_<version>_checksums.txt, checked against a download on 2026-10-23. A checksum file fetched
# from the same place would only catch a broken download, not a swapped one.
#
# To move syft on: change SYFT_VERSION in release.yml, paste the new release's hashes below, and
# check them against `gh api repos/anchore/syft/releases/tags/v<version> --jq '.assets[]'`.
set -euo pipefail

version="${SYFT_VERSION:?SYFT_VERSION is not set}"
if [ "$version" != "1.55.0" ]; then
  echo "install-syft.sh has the hashes of syft 1.55.0, and SYFT_VERSION is $version" >&2
  exit 1
fi

case "${RUNNER_OS}-${RUNNER_ARCH}" in
  Linux-X64)
    asset=syft_1.55.0_linux_amd64.tar.gz
    sha=bb65249542db5fedeeb4d3f9b018b75e7264dbfbbb10486f044581a92f90490d ;;
  Linux-ARM64)
    asset=syft_1.55.0_linux_arm64.tar.gz
    sha=929bd163db2d7cd2255d3bcb8967208079813d7b46b0c952f7b794d8e2c7d304 ;;
  *)
    echo "no syft build for ${RUNNER_OS}-${RUNNER_ARCH}" >&2
    exit 1 ;;
esac

dir="${RUNNER_TEMP}/syft-install"
mkdir -p "$dir/bin"
cd "$dir"
curl --proto '=https' --tlsv1.2 -fsSLO \
  "https://github.com/anchore/syft/releases/download/v${version}/${asset}"
if command -v sha256sum > /dev/null; then
  echo "${sha}  ${asset}" | sha256sum -c -
else
  echo "${sha}  ${asset}" | shasum -a 256 -c -
fi
# The archive holds syft, its README, its LICENSE and its changelog, with no wrapper folder.
tar -xzf "$asset" -C "$dir/bin" syft
echo "$dir/bin" >> "$GITHUB_PATH"
"$dir/bin/syft" version
