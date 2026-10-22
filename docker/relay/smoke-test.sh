#!/usr/bin/env bash
# The relay image's smoke test: the shipped image starts and serves the captive portal check, and
# the guard refuses each config that would open the relay by mistake, with exit code 64. images.yml
# and release.yml run it on every build of the image, so a guard that lets a typo through fails CI.
#
# Usage: docker/relay/smoke-test.sh <image>
#
# It makes one network, a throwaway certificate and the containers it needs, all named after this
# process, and removes them when it exits. It touches no other container.
set -euo pipefail

image="${1:?usage: smoke-test.sh <image>}"
name="sluice-relay-smoke-$$"
work="$(mktemp -d)"
failed=0

# Called by the trap below, which shellcheck does not follow.
# shellcheck disable=SC2329
cleanup() {
  docker ps -aq --filter "name=^${name}-" | xargs -r docker rm -f > /dev/null
  docker network rm "$name" > /dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

# A certificate for 127.0.0.1 that says CA:FALSE, which the server needs to start. The folder and
# key are readable by everyone, so uid 65532 in the container can read them without a chown: they
# are thrown away when the script exits.
mkdir "$work/certs"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
  -keyout "$work/certs/default.key" -out "$work/certs/default.crt" -subj '/CN=127.0.0.1' \
  -addext 'subjectAltName=IP:127.0.0.1' -addext 'basicConstraints=critical,CA:FALSE' \
  -addext 'keyUsage=digitalSignature' -addext 'extendedKeyUsage=serverAuth' 2> /dev/null
chmod 755 "$work" "$work/certs"
chmod 644 "$work/certs/default.key"
docker network create "$name" > /dev/null

pass() { echo "ok    $1"; }
fail() { echo "FAIL  $1"; failed=1; }

# Starts the image with the given arguments and passes if /generate_204 answers 204 on port 80.
starts() {
  local check="$1" container="$name-$2"
  shift 2
  docker run -d --name "$container" --network "$name" -p 127.0.0.1::80 \
    -v "$work/certs:/etc/relay/certs:ro" "$image" "$@" > /dev/null
  local port code=000
  port="$(docker port "$container" 80/tcp | head -n 1 | cut -d: -f2)"
  for _ in $(seq 1 50); do
    code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/generate_204" || true)"
    [ "$code" = 204 ] && break
    sleep 0.2
  done
  if [ "$code" = 204 ]; then
    pass "$check: /generate_204 answered 204"
  else
    fail "$check: /generate_204 answered $code"
    docker logs "$container" 2>&1 | tail -n 5
  fi
  docker rm -f "$container" > /dev/null
}

# Runs the image with the given docker and guard arguments, and passes if it exits with 64 and its
# one line of stderr holds the given text.
refuses() {
  local check="$1" says="$2" code=0
  shift 2
  docker run --rm --network "$name" "$@" > /dev/null 2> "$work/stderr" || code=$?
  local line
  line="$(head -n 1 "$work/stderr")"
  if [ "$code" = 64 ] && [ "$(wc -l < "$work/stderr")" -eq 1 ] && [[ "$line" == *"$says"* ]]; then
    pass "$check: exit $code: $line"
  else
    fail "$check: exit $code: $(cat "$work/stderr")"
  fi
}

starts "the shipped image" shipped
refuses "--open with relay.toml" "--open was given, but \`access\` is an allowlist" \
  "$image" --open --config-path /etc/relay/relay.toml
refuses "open.toml without --open" "\`access\` is \"everyone\"" \
  "$image" --config-path /etc/relay/open.toml
starts "open.toml with --open" open --open --config-path /etc/relay/open.toml
printf '[acess]\nallowlist = []\n' > "$work/acess.toml"
chmod 644 "$work/acess.toml"
refuses "a mounted config with [acess]" "unknown key \`acess\`" \
  -v "$work/acess.toml:/etc/relay/relay.toml:ro" "$image"
refuses "a --config-path that does not exist" "does not exist" \
  "$image" --config-path /etc/relay/missing.toml

exit "$failed"
