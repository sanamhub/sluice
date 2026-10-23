#!/usr/bin/env bash
# Pushes one image of a release to ghcr.io: each architecture under `<version>-<arch>`, then the
# version tag as the index of both. release.yml's image-publish job runs it, and the attest steps
# after it read the digests it writes to GITHUB_OUTPUT.
#
# IMAGE is `rendezvous` or `relay`, OWNER the repository owner and TAG the checked `v<version>`
# tag. The image-build jobs left `<arch>.tar.gz` for each architecture in RUNNER_TEMP/image.
# REGISTRY is the registry to push to and has no default: release.yml sets `ghcr.io` on the push
# step, and images.yml's push test sets a local one. A default would let a value written to
# GITHUB_ENV by an earlier step choose where a release goes.
set -euo pipefail
# A failed command inside `$(...)` stops the script too, and not only the line that reads it.
shopt -s inherit_errexit

: "${IMAGE:?}" "${OWNER:?}" "${TAG:?}" "${GITHUB_TOKEN:?}" "${GITHUB_ACTOR:?}"
: "${RUNNER_TEMP:?}" "${GITHUB_OUTPUT:?}" "${REGISTRY:?}"

# GitHub's documentation says a step's `env` overrides a job's or the workflow's, and says nothing
# of a value an earlier step wrote to GITHUB_ENV. So on a tag, which is a release, the only
# registry is ghcr.io, and the token is never sent anywhere else, whatever an earlier step did.
if [[ "${GITHUB_REF:-}" == refs/tags/* && "$REGISTRY" != ghcr.io ]]; then
  echo "REGISTRY is '$REGISTRY', and a release on a tag pushes to ghcr.io only" >&2
  exit 1
fi
registry="$REGISTRY"
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
