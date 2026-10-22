#!/usr/bin/env bash
# Pushes one image of a release to ghcr.io: each architecture under `<version>-<arch>`, then the
# version tag as the index of both. release.yml's image-publish job runs it, and the attest steps
# after it read the digests it writes to GITHUB_OUTPUT.
#
# IMAGE is `rendezvous` or `relay`, OWNER the repository owner and TAG the checked `v<version>`
# tag. The image-build jobs left `<arch>.tar.gz` for each architecture in RUNNER_TEMP/image.
set -euo pipefail

: "${IMAGE:?}" "${OWNER:?}" "${TAG:?}" "${GITHUB_TOKEN:?}" "${GITHUB_ACTOR:?}"
: "${RUNNER_TEMP:?}" "${GITHUB_OUTPUT:?}"

version="${TAG#v}"
# A ghcr.io name is lower case, and an owner's login may not be.
repo="ghcr.io/$(echo "$OWNER" | tr '[:upper:]' '[:lower:]')/sluice-$IMAGE"

# The digest the registry gives a tag, which is what a pull of that tag fetches.
digest() {
  docker buildx imagetools inspect "$1" --format '{{json .Manifest}}' | jq --raw-output .digest
}

echo "$GITHUB_TOKEN" | docker login ghcr.io --username "$GITHUB_ACTOR" --password-stdin
for arch in amd64 arm64; do
  docker load --input "$RUNNER_TEMP/image/$arch.tar.gz"
  docker tag "sluice-$IMAGE:$arch" "$repo:$version-$arch"
  docker push "$repo:$version-$arch"
  echo "$arch=$(digest "$repo:$version-$arch")" >> "$GITHUB_OUTPUT"
done
docker buildx imagetools create --tag "$repo:$version" "$repo:$version-amd64" "$repo:$version-arm64"
{
  echo "name=$repo"
  echo "index=$(digest "$repo:$version")"
} >> "$GITHUB_OUTPUT"
