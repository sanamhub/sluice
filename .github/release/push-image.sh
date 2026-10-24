#!/usr/bin/env bash
# Pushes one image of a release to a registry, in two calls, so that each public tag is attested
# before the next is written:
#
#   push-image.sh <registry> arch <amd64-id> <arm64-id>
#       each architecture under `<version>-<arch>`, after checking the loaded image's ID and
#       platform against what the build recorded
#   push-image.sh <registry> index <amd64-digest> <arm64-digest>
#       the version tag as the index of exactly those two digests, after checking it
#
# release.yml's image-publish job runs `arch`, attests both architectures, then runs `index` with
# the digests it just attested, and attests the index. The index is built from the digests and
# never from a tag, so a tag moved in between cannot put an unattested manifest in it. A failure in
# between leaves public tags without their attestation, which a re-run overwrites (RELEASING.md).
#
# The registry is the first argument and never comes from the environment: release.yml writes the
# literal `ghcr.io`, and images.yml's push test writes `localhost:5000`. A value an earlier step
# wrote to GITHUB_ENV cannot change it, and on a tag any registry but ghcr.io is refused.
#
# IMAGE is `rendezvous` or `relay`, OWNER the repository owner and TAG the checked `v<version>`
# tag. The image-build jobs left `<arch>.tar.gz` and `<arch>.spdx.json` for each architecture in
# RUNNER_TEMP/image.
set -euo pipefail
# A failed command inside `$(...)` stops the script too, and not only the line that reads it.
shopt -s inherit_errexit

usage="usage: push-image.sh <registry> arch|index <amd64> <arm64>"
registry="${1:?$usage}"
step="${2:?$usage}"
amd64="${3:?$usage}"
arm64="${4:?$usage}"
case "$step" in
  arch | index) ;;
  *)
    echo "$usage" >&2
    exit 2 ;;
esac
# A registry is a host name and an optional port. A slash, a space or a leading dash would make
# the repository name below mean something else.
if [[ ! "$registry" =~ ^[A-Za-z0-9.-]+(:[0-9]+)?$ ]]; then
  echo "'$registry' is not a registry: give a host name and an optional port" >&2
  exit 2
fi

: "${IMAGE:?}" "${OWNER:?}" "${TAG:?}" "${GITHUB_TOKEN:?}" "${GITHUB_ACTOR:?}"
: "${RUNNER_TEMP:?}" "${GITHUB_OUTPUT:?}"

# An image ID and a manifest digest are both `sha256:` and 64 lower-case hex digits. The IDs come
# from a build job's output, so they are checked before anything is loaded or pushed.
check_sha256() {
  if [[ ! "$1" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "the $2 '$1' is not sha256: and 64 lower-case hex digits" >&2
    exit 2
  fi
}
case "$step" in
  arch)
    check_sha256 "$amd64" "amd64 image ID"
    check_sha256 "$arm64" "arm64 image ID" ;;
  index)
    check_sha256 "$amd64" "amd64 digest"
    check_sha256 "$arm64" "arm64 digest" ;;
esac

# GitHub's documentation says a step's `env` overrides a job's or the workflow's, and says nothing
# of a value an earlier step wrote to GITHUB_ENV. On a tag, which is a release, the only registry
# is ghcr.io, and the token is never sent anywhere else, whatever an earlier step did.
if [[ "${GITHUB_REF:-}" == refs/tags/* && "$registry" != ghcr.io ]]; then
  echo "the registry is '$registry', and a release on a tag pushes to ghcr.io only" >&2
  exit 1
fi
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
  printf '%s\n' "$GITHUB_TOKEN" \
    | docker login "$registry" --username "$GITHUB_ACTOR" --password-stdin
}

# The architecture each image is, and that it is the image the build job recorded, before it is
# pushed under that architecture's name. A tarball saved under the wrong tag, or one swapped in the
# artifact store, would otherwise put another image behind `<version>-arm64`, and an attestation
# would then sign it as arm64. The ID is the config digest, which covers every layer, and it comes
# from an output of the build job, recorded before syft ran.
push_architectures() {
  local arch image platform digest loaded id expected
  for arch in amd64 arm64; do
    case "$arch" in
      amd64) expected="$amd64" ;;
      arm64) expected="$arm64" ;;
    esac
    image="sluice-$IMAGE:$arch"
    loaded="$(docker load --input "$RUNNER_TEMP/image/$arch.tar.gz")"
    # The tag inside the tarball, and not whatever the runner had under this name before, is what
    # the checks below are read from.
    if ! grep -qxF "Loaded image: $image" <<< "$loaded"; then
      echo "the $arch tarball does not hold $image:" >&2
      printf '%s\n' "$loaded" >&2
      return 1
    fi
    id="$(docker image inspect "$image" --format '{{.Id}}')"
    if [ "$id" != "$expected" ]; then
      echo "$image is $id, and the build recorded $expected: this is not the image its SBOM \
describes, so nothing was pushed" >&2
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
    # The registry must be serving the manifest this push wrote, and not one another push put
    # behind the same tag since: `docker push` records the digest it wrote in RepoDigests.
    if ! docker image inspect "$image" --format '{{json .RepoDigests}}' \
      | grep -qF "\"$repo@$digest\""; then
      echo "$repo:$version-$arch is $digest in the registry, and not the manifest just pushed for \
$image: nothing was attested" >&2
      return 1
    fi
    echo "$arch=$digest" >> "$GITHUB_OUTPUT"
  done
  echo "name=$repo" >> "$GITHUB_OUTPUT"
}

# That the version tag lists exactly the two digests given to this call, one per Linux architecture,
# so the index attestation names what a pull of the version tag resolves and then fetches.
check_index() {
  local reference="$1" amd64="$2" arm64="$3" raw want got
  raw="$(docker buildx imagetools inspect "$reference" --raw)"
  want="$(printf 'linux/amd64 %s\nlinux/arm64 %s\n' "$amd64" "$arm64" | sort)"
  got="$(jq --raw-output '.manifests[] | "\(.platform.os)/\(.platform.architecture) \(.digest)"' \
    <<< "$raw" | sort)"
  if [ "$got" = "$want" ]; then
    return 0
  fi
  echo "$reference lists these manifests, and push-image.sh expected the two it was given:" >&2
  printf '%s\n' "$got" "--- expected ---" "$want" >&2
  return 1
}

# The version tag as the index of the two digests this call was given: the ones image-publish
# attested. A tag read back from the registry would name whatever is there now, which a push in
# between can have moved.
create_index() {
  local index
  docker buildx imagetools create --tag "$repo:$version" "$repo@$amd64" "$repo@$arm64"
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
