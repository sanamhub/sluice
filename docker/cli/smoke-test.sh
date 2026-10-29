#!/usr/bin/env bash
# Checks the tool image the release ships: the entry point, the identity the volume keeps, the
# local report, and a transfer between two containers on one network with no route out.
#
# Usage: docker/cli/smoke-test.sh <image>
#
# images.yml and release.yml run it on every build, so an image that misses a library, drops the
# entry point or cannot write its home fails before it is pushed. The network is created --internal:
# the two containers can reach each other and nothing else, so a pass shows the transfer needed no
# relay and no internet. The folders are on the host and mounted in, which is how a user sends a
# folder that is already there.
set -euo pipefail

usage="usage: smoke-test.sh <image>"
image="${1:?$usage}"
# Random, and not the process id, which two runs in two namespaces may share.
name="sluice-cli-smoke-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')"
work="$(mktemp -d)"
failed=0

# Every container, volume and the network are named after $name, so a run that timed out is
# removed here too. `set +e`, so one failure does not stop the removal of what follows.
# shellcheck disable=SC2329
cleanup() {
  set +e
  docker ps -aq --filter "name=^${name}-" | xargs -r docker rm -f > /dev/null
  docker volume rm "${name}-home" "${name}-out" "${name}-sender" "${name}-receiver" > /dev/null 2>&1
  docker network rm "$name" > /dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

pass() { echo "ok    $1"; }
fail() { echo "FAIL  $1"; failed=1; }

# The distroless image has no shell to run the entry point through, so Docker runs `sluice`
# directly and every command below is one the user types.
version="$(docker run --rm "$image" --version)"
if [[ "$version" == sluice\ * ]]; then pass "version: $version"; else fail "version: $version"; fi

user="$(docker image inspect "$image" --format '{{.Config.User}}')"
if [ "$user" = "65532:65532" ]; then pass "user: $user"; else fail "user: $user"; fi

if docker run --rm --entrypoint /bin/sh "$image" -c true > /dev/null 2>&1; then
  fail "the image has a shell"
else
  pass "no shell"
fi

# A volume that outlives the container is the home, so the endpoint id is the same one. Without it
# the container writes a new home on its own layer and is a new endpoint.
docker volume create "${name}-home" > /dev/null
first="$(docker run --rm --read-only -v "${name}-home:/home/sluice" "$image" id)"
second="$(docker run --rm --read-only -v "${name}-home:/home/sluice" "$image" id)"
fresh="$(docker run --rm --read-only "$image" id)"
if [[ "$first" =~ ^[0-9a-f]{64}$ ]]; then pass "id: $first"; else fail "id: $first"; fi
if [ "$first" = "$second" ]; then pass "the volume keeps the id"; else fail "the volume changed the id"; fi
if [ "$first" != "$fresh" ]; then pass "a fresh container gets a fresh id"; else fail "a fresh container reused the id"; fi

# `doctor` only reads. Its home is the volume, and the root is read-only to show it writes nothing
# outside the home.
if docker run --rm --read-only -v "${name}-home:/home/sluice" "$image" doctor > /dev/null; then
  pass "doctor"
else
  fail "doctor"
fi

# A folder that is already there, sent to a second container over a network with no gateway.
mkdir -p "$work/data" "$work/out"
head -c 1048576 /dev/urandom > "$work/data/payload.bin"
chmod 0755 "$work/data"
chmod 0644 "$work/data/payload.bin"
# The image runs as 65532, and a bind mount keeps the host's owner, so the output folder is opened
# to it. The data folder only has to be readable.
chmod 0777 "$work/out"
docker network create --internal "$name" > /dev/null

docker run --detach --name "${name}-sender" --network "$name" --read-only \
  -v "$work/data:/data:ro" -v "${name}-sender:/home/sluice" \
  "$image" send /data/payload.bin > /dev/null

ticket=""
for _ in $(seq 1 120); do
  ticket="$(docker logs "${name}-sender" 2>&1 | grep -m1 -o 'sluice1[0-9a-z]*' || true)"
  if [ -n "$ticket" ]; then break; fi
  if [ "$(docker inspect --format '{{.State.Running}}' "${name}-sender")" != true ]; then break; fi
  sleep 0.5
done

if [ -n "$ticket" ]; then
  pass "ticket"
  if docker run --rm --name "${name}-receiver" --network "$name" --read-only \
    -v "$work/out:/out" -v "${name}-receiver:/home/sluice" \
    "$image" recv "$ticket" --out /out; then
    if cmp -s "$work/data/payload.bin" "$work/out/payload.bin"; then
      pass "send and recv over an internal network"
    else
      fail "the received file differs"
    fi
  else
    fail "recv"
  fi
else
  fail "ticket"
fi

# The sender stops once the one download completes; a wait that times out means it did not.
if [ "$(timeout 30 docker wait "${name}-sender" 2> /dev/null || echo timeout)" = 0 ]; then
  pass "the sender stopped after one download"
else
  fail "the sender did not stop after one download"
fi

if [ "$failed" -ne 0 ]; then
  echo "the tool image failed its smoke test"
  exit 1
fi
