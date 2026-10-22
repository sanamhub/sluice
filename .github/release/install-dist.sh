#!/usr/bin/env bash
# Installs dist at the exact version release.yml names, from its GitHub release, and refuses an
# archive whose SHA-256 is not the one written here. The hashes come from that release's
# sha256.sum. Checking against a checksum file fetched from the same place would only catch a
# broken download, not a swapped one.
#
# To move dist on: change DIST_VERSION in release.yml, paste the new release's hashes below, and
# regenerate release.yml as RELEASING.md says.
set -euo pipefail

version="${DIST_VERSION:?DIST_VERSION is not set}"
if [ "$version" != "0.33.1" ]; then
  echo "install-dist.sh has the hashes of dist 0.33.1, and DIST_VERSION is $version" >&2
  exit 1
fi

case "${RUNNER_OS}-${RUNNER_ARCH}" in
  Linux-X64)
    asset=cargo-dist-x86_64-unknown-linux-gnu.tar.xz
    sha=da68e159d5fed1ee61b00284da444e719927dbd0b7d0e2c34e57a04414453cf9 ;;
  Linux-ARM64)
    asset=cargo-dist-aarch64-unknown-linux-gnu.tar.xz
    sha=e883f4c6c17f2458d6ced950d978c04b45423ed3a5fdbebcb8f8de568389e1c1 ;;
  macOS-ARM64)
    asset=cargo-dist-aarch64-apple-darwin.tar.xz
    sha=b8bf041a38e9e54ecf41eee2574ca6644043796620bb0ee8340e9c0acbaaae8f ;;
  macOS-X64)
    asset=cargo-dist-x86_64-apple-darwin.tar.xz
    sha=e9dc303035fbb2f44295948e0a076f1d1d218a0328e9b84e16955863bba92bf4 ;;
  # dist publishes no Windows ARM64 build. The x64 one runs there under emulation, and it only
  # drives cargo, which is native.
  Windows-X64 | Windows-ARM64)
    asset=cargo-dist-x86_64-pc-windows-msvc.zip
    sha=79dfcce84e6a8e69210f11fc23d445c3aac7af6a29e0e981867f053c0a869316 ;;
  *)
    echo "no dist build for ${RUNNER_OS}-${RUNNER_ARCH}" >&2
    exit 1 ;;
esac

dir="${RUNNER_TEMP}/dist-install"
mkdir -p "$dir/bin"
cd "$dir"
curl --proto '=https' --tlsv1.2 -fsSLO \
  "https://github.com/axodotdev/cargo-dist/releases/download/v${version}/${asset}"
if command -v sha256sum > /dev/null; then
  echo "${sha}  ${asset}" | sha256sum -c -
else
  echo "${sha}  ${asset}" | shasum -a 256 -c -
fi
case "$asset" in
  *.zip) unzip -oq "$asset" -d bin ;;
  *) tar -xJf "$asset" --strip-components=1 -C bin ;;
esac
echo "$dir/bin" >> "$GITHUB_PATH"
"$dir/bin/dist" --version
