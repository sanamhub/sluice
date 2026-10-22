#!/usr/bin/env bash
# Pushes one image of a release to ghcr.io: each architecture under `<version>-<arch>`, then the
# version tag as the index of both. release.yml's image-publish job runs it, and the attest steps
# after it read the digests it writes to GITHUB_OUTPUT.
#
# IMAGE is `rendezvous` or `relay`, OWNER the repository owner and TAG the checked `v<version>`
# tag. The image-build jobs left `<arch>.tar.gz` for each architecture in RUNNER_TEMP/image.
# REGISTRY is unset in a release; a test of this script points it at a local registry.
set -euo pipefail
# A failed command inside `$(...)` stops the script too, and not only the line that reads it.
shopt -s inherit_errexit

: "${IMAGE:?}" "${OWNER:?}" "${TAG:?}" "${GITHUB_TOKEN:?}" "${GITHUB_ACTOR:?}"
: "${RUNNER_TEMP:?}" "${GITHUB_OUTPUT:?}"

registry="${REGISTRY:-ghcr.io}"
version="${TAG#v}"
# A ghcr.io name is lower case, and an owner's login may not be.
repo="$registry/$(printf '%s' "$OWNER" | tr '[:upper:]' '[:lower:]')/sluice-$IMAGE"

# The digest the registry gives a reference, which is what a pull of it fetches, after checking
# that the manifest there is the kind expected: `image` for one platform's image, `index` for the
# list of both. An attestation on anything else would sign what no one pulls.
checked_digest() {
  local reference="$1" kind="$2" manifest media digest
  manifest="$(docker buildx imagetools inspect "$reference" --format '{{json .Manifest}}')"
  media="$(jq --raw-output .mediaType <<< "$manifest")"
  digest="$(jq --raw-output .digest <<< "$manifest")"
  case "$kind:$media" in
    image:application/vnd.oci.image.manifest.v1+json) ;;
    image:application/vnd.docker.distribution.manifest.v2+json) ;;
    index:application/vnd.oci.image.index.v1+json) ;;
    index:application/vnd.docker.distribution.manifest.list.v2+json) ;;
    *)
      echo "$reference is $media, and push-image.sh expects one $kind manifest there" >&2
      return 1 ;;
  esac
  if [[ ! "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "$reference has the digest '$digest', and not sha256: and 64 lower-case hex digits" >&2
    return 1
  fi
  printf '%s\n' "$digest"
}

printf '%s\n' "$GITHUB_TOKEN" | docker login "$registry" --username "$GITHUB_ACTOR" --password-stdin
declare -A digests
for arch in amd64 arm64; do
  docker load --input "$RUNNER_TEMP/image/$arch.tar.gz"
  docker tag "sluice-$IMAGE:$arch" "$repo:$version-$arch"
  docker push "$repo:$version-$arch"
  # Assigned before it is written, so a failed lookup stops the script with nothing written.
  digest="$(checked_digest "$repo:$version-$arch" image)"
  digests[$arch]="$digest"
  echo "$arch=$digest" >> "$GITHUB_OUTPUT"
done
# From the digests just checked, not the tags, so the index lists exactly the images attested.
docker buildx imagetools create --tag "$repo:$version" \
  "$repo@${digests[amd64]}" "$repo@${digests[arm64]}"
index="$(checked_digest "$repo:$version" index)"
{
  echo "name=$repo"
  echo "index=$index"
} >> "$GITHUB_OUTPUT"
