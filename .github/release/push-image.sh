#!/usr/bin/env bash
# Pushes one image of a release to a registry, in two calls, so that each public tag is attested
# before the next is written:
#
#   push-image.sh arch    each architecture under `<version>-<arch>`, after checking its platform
#   push-image.sh index   the version tag as the index of those two digests, after checking it
#
# release.yml's image-publish job runs `arch`, attests both architectures, then runs `index` and
# attests the index. A failure in between leaves public tags without their attestation, which a
# re-run overwrites (RELEASING.md).
#
# IMAGE is `rendezvous` or `relay`, OWNER the repository owner and TAG the checked `v<version>`
# tag. The image-build jobs left `<arch>.tar.gz` for each architecture in RUNNER_TEMP/image.
# REGISTRY is the registry to push to and has no default: release.yml sets `ghcr.io` on each step,
# and images.yml's push test sets a local one. A default would let a value written to GITHUB_ENV by
# an earlier step choose where a release goes.
set -euo pipefail
# A failed command inside `$(...)` stops the script too, and not only the line that reads it.
shopt -s inherit_errexit

usage="usage: push-image.sh arch|index"
step="${1:?$usage}"
case "$step" in
  arch | index) ;;
  *)
    echo "$usage" >&2
    exit 2 ;;
esac

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

# A failure to log in must not look like a push: the token goes in on standard input and nowhere
# else.
login() {
  printf '%s\n' "$GITHUB_TOKEN" | docker login "$registry" --username "$GITHUB_ACTOR" --password-stdin
}

# The architecture each image is, before it is pushed under that architecture's name. A tarball
# saved under the wrong tag, or a stand-in built for another platform, would otherwise put an
# amd64 image behind `<version>-arm64`, and an attestation would then sign it as arm64.
push_architectures() {
  local arch image platform digest loaded
  for arch in amd64 arm64; do
    image="sluice-$IMAGE:$arch"
    # The sha256 recorded beside the tarball when the image was saved, before anything reads the
    # tarball: one swapped in the artifact store is not the image the SBOM describes.
    ( cd "$RUNNER_TEMP/image" && sha256sum --check --strict "$arch.tar.gz.sha256" )
    loaded="$(docker load --input "$RUNNER_TEMP/image/$arch.tar.gz")"
    # The tag inside the tarball, and not whatever the runner had under this name before, is what
    # the platform below is read from.
    if ! grep -qxF "Loaded image: $image" <<< "$loaded"; then
      echo "the $arch tarball does not hold $image:" >&2
      printf '%s\n' "$loaded" >&2
      return 1
    fi
    platform="$(docker image inspect "$image" --format '{{.Os}}/{{.Architecture}}')"
    if [ "$platform" != "linux/$arch" ]; then
      echo "$image is $platform, and push-image.sh pushes it as linux/$arch" >&2
      return 1
    fi
    docker tag "$image" "$repo:$version-$arch"
    docker push "$repo:$version-$arch"
    # Assigned before it is written, so a failed lookup stops the script with nothing written.
    digest="$(checked_digest "$repo:$version-$arch" image)"
    echo "$arch=$digest" >> "$GITHUB_OUTPUT"
  done
  echo "name=$repo" >> "$GITHUB_OUTPUT"
}

# That the version tag lists exactly the two digests pushed above, one per Linux architecture, so
# the index attestation names what a pull of the version tag resolves and then fetches.
check_index() {
  local reference="$1" amd64="$2" arm64="$3" raw want got
  raw="$(docker buildx imagetools inspect "$reference" --raw)"
  want="$(printf 'linux/amd64 %s\nlinux/arm64 %s\n' "$amd64" "$arm64" | sort)"
  got="$(jq --raw-output '.manifests[] | "\(.platform.os)/\(.platform.architecture) \(.digest)"' \
    <<< "$raw" | sort)"
  if [ "$got" = "$want" ]; then
    return 0
  fi
  echo "$reference lists these manifests, and push-image.sh expected the two it pushed:" >&2
  printf '%s\n' "$got" "--- expected ---" "$want" >&2
  return 1
}

# The version tag as the index of both architectures. The digests are read back from the registry
# and not passed in from the `arch` call: the index names what is there now, which is also what the
# architecture attestations signed.
create_index() {
  local amd64 arm64 index
  amd64="$(checked_digest "$repo:$version-amd64" image)"
  arm64="$(checked_digest "$repo:$version-arm64" image)"
  docker buildx imagetools create --tag "$repo:$version" \
    "$repo@$amd64" "$repo@$arm64"
  index="$(checked_digest "$repo:$version" index)"
  check_index "$repo:$version" "$amd64" "$arm64"
  {
    echo "name=$repo"
    echo "index=$index"
  } >> "$GITHUB_OUTPUT"
}

case "$step" in
  arch)
    login
    push_architectures ;;
  index)
    login
    create_index ;;
esac
