#!/usr/bin/env bash
# The smoke test of both images: the shipped image starts, with a read-only root, and the guard
# refuses each config that would open more than the operator wrote, with exit code 64 and one line
# that names the key. images.yml and release.yml run it on every build of either image, so a guard
# that lets a mistake through fails CI (ADR 0019, ADR 0020).
#
# Usage: docker/smoke-test.sh relay|rendezvous <image>
#
# It makes one network, a throwaway certificate and the containers it needs, all named after a
# random suffix, and removes them when it exits. It touches no other container.
set -euo pipefail

usage="usage: smoke-test.sh relay|rendezvous <image>"
kind="${1:?$usage}"
image="${2:?$usage}"
case "$kind" in
  relay | rendezvous) ;;
  *) echo "$usage" >&2; exit 2 ;;
esac
# Random, and not the process id, which two runs in two containers or namespaces may share.
name="sluice-smoke-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')"
etc="/etc/$kind"
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

# The relay is up when /generate_204 answers 204 on port 80. The rendezvous server has no HTTP
# port, and is up when it logs that its QUIC listener is bound.
up() {
  local container="$1"
  if [ "$kind" = relay ]; then
    local port
    port="$(docker port "$container" 80/tcp | head -n 1 | cut -d: -f2)"
    [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/generate_204")" = 204 ]
  else
    # Into a variable first: with `grep -q` at the end of a pipe, grep exits on its first match,
    # docker gets SIGPIPE, and pipefail turns the match into a failure.
    local logs
    logs="$(docker logs "$container" 2>&1)"
    grep -q 'QUIC server listening' <<< "$logs"
  fi
}

# Starts the image with a read-only root and the given arguments, and passes if it comes up.
starts() {
  local check="$1" container="$name-$2" publish=()
  shift 2
  [ "$kind" = relay ] && publish=(-p 127.0.0.1::80)
  docker run -d --read-only --name "$container" --network "$name" "${publish[@]}" \
    -v "$work/certs:$etc/certs:ro" "$image" "$@" > /dev/null
  local ok=no
  for _ in $(seq 1 50); do
    if up "$container"; then ok=yes; break; fi
    sleep 0.2
  done
  if [ "$ok" = yes ]; then
    pass "$check: up, with a read-only root"
  else
    fail "$check: did not come up"
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

# Writes a config, given with \n for its line breaks, into the work folder, readable by uid 65532,
# and prints the -v that mounts it over the shipped one.
config() {
  local file="$work/$1.toml"
  printf '%b' "$2" > "$file"
  chmod 644 "$file"
  printf '%s' "$file:$etc/$3:ro"
}

metrics='enable_metrics = false\n'
allow='[access]\nallowlist = []\n'
tls='[tls]\nquic_bind_addr = "0.0.0.0:7842"\ncert_mode = "Manual"\n'
quic='enable_quic_addr_discovery = true\n'

relay() {
  starts "the shipped image" shipped
  starts "open.toml with --open" open --open --config-path /etc/relay/open.toml
  refuses "--open with relay.toml" "--open was given, but \`access\` is an allowlist" \
    "$image" --open --config-path /etc/relay/relay.toml
  refuses "open.toml without --open" "\`access\` is \"everyone\"" \
    "$image" --config-path /etc/relay/open.toml
  refuses "--rendezvous with relay.toml" "needs \`enable_relay = false\`" \
    "$image" --rendezvous --config-path /etc/relay/relay.toml
  refuses "--dev" "--dev is refused" "$image" --dev --config-path /etc/relay/relay.toml
  refuses "a --config-path that does not exist" "does not exist" \
    "$image" --config-path /etc/relay/missing.toml
  refuses "a mounted config with [acess]" "unknown key \`acess\`" \
    -v "$(config acess "${metrics}[acess]\nallowlist = []\n" relay.toml)" "$image"
  refuses "a denylist without --open" "\`access\` is a denylist" \
    -v "$(config deny "${metrics}[access]\ndenylist = []\n" relay.toml)" "$image"
  refuses "no access" "\`access\` is not set" \
    -v "$(config none "$metrics" relay.toml)" "$image"
  refuses "dangerous_http_only" "\`dangerous_http_only\` is true" \
    -v "$(config http "${metrics}${allow}${tls}dangerous_http_only = true\n" relay.toml)" "$image"
  refuses "no enable_metrics" "\`enable_metrics\` is not set" \
    -v "$(config metrics "$allow" relay.toml)" "$image"
}

rendezvous() {
  starts "the shipped image" shipped
  refuses "no enable_relay = false" "needs \`enable_relay = false\`" \
    -v "$(config on "${metrics}${quic}${tls}" manual.toml)" "$image"
  refuses "--open with --rendezvous" "--open and --rendezvous are given together" \
    "$image" --rendezvous --open --config-path "$etc/manual.toml"
  refuses "manual.toml without --rendezvous" "\`enable_relay\` is false" \
    "$image" --config-path "$etc/manual.toml"
  refuses "--dev" "--dev is refused" "$image" --dev --rendezvous --config-path "$etc/manual.toml"
  refuses "no enable_metrics" "\`enable_metrics\` is not set" \
    -v "$(config metrics "enable_relay = false\n${quic}${tls}" manual.toml)" "$image"
  refuses "an access setting" "\`access\` is set" \
    -v "$(config access "enable_relay = false\n${metrics}access = \"everyone\"\n" manual.toml)" \
    "$image"
  refuses "dangerous_http_only" "\`dangerous_http_only\` is true" \
    -v "$(config http "enable_relay = false\n${metrics}${quic}${tls}dangerous_http_only = true\n" \
      manual.toml)" "$image"
}

case "$kind" in
  relay) relay ;;
  rendezvous) rendezvous ;;
esac
exit "$failed"
