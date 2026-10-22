#!/usr/bin/env bash
# Installs actionlint and shellcheck at exact versions, from their GitHub releases, and refuses an
# archive whose SHA-256 is not the one written here. ci.yml's `workflows` job runs it. The runner
# image has a shellcheck of its own, at whatever version the image ships, which is why this one goes
# first on PATH.
#
# actionlint's hash is in the release's checksums file, and shellcheck's release has none, so its
# hash is the digest GitHub's API gives the asset. Both were checked against a download on
# 2026-10-23. To move one on, change its version and paste the new hash from
# `gh api repos/<owner>/<repo>/releases/tags/<tag> --jq '.assets[] | "\(.name) \(.digest)"'`.
set -euo pipefail

actionlint_version=1.7.12
actionlint_sha=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8
shellcheck_version=0.11.0
shellcheck_sha=8c3be12b05d5c177a04c29e3c78ce89ac86f1595681cab149b65b97c4e227198

# The job runs on ubuntu-24.04 only, so only that runner's builds are listed.
if [ "${RUNNER_OS}-${RUNNER_ARCH}" != "Linux-X64" ]; then
  echo "no actionlint or shellcheck hash for ${RUNNER_OS}-${RUNNER_ARCH}" >&2
  exit 1
fi

dir="${RUNNER_TEMP}/lint-tools"
mkdir -p "$dir/bin"
cd "$dir"

actionlint="actionlint_${actionlint_version}_linux_amd64.tar.gz"
curl --proto '=https' --tlsv1.2 -fsSLO \
  "https://github.com/rhysd/actionlint/releases/download/v${actionlint_version}/${actionlint}"
echo "${actionlint_sha}  ${actionlint}" | sha256sum -c -
tar -xzf "$actionlint" -C bin actionlint

shellcheck="shellcheck-v${shellcheck_version}.linux.x86_64.tar.xz"
curl --proto '=https' --tlsv1.2 -fsSLO \
  "https://github.com/koalaman/shellcheck/releases/download/v${shellcheck_version}/${shellcheck}"
echo "${shellcheck_sha}  ${shellcheck}" | sha256sum -c -
tar -xJf "$shellcheck" --strip-components=1 -C bin "shellcheck-v${shellcheck_version}/shellcheck"

echo "$dir/bin" >> "$GITHUB_PATH"
"$dir/bin/actionlint" -version
"$dir/bin/shellcheck" --version
