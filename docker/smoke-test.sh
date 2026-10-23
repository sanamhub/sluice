#!/usr/bin/env bash
# The smoke test of both images: the shipped image starts, with a read-only root, as the config it
# was given and not as iroh-relay's open default, and the guard refuses each config that would open
# more than the operator wrote, with exit code 64 and one line that names the key. images.yml and
# release.yml run it on every build of either image, so a guard that lets a mistake through fails
# CI (ADR 0019, ADR 0020).
#
# Usage: docker/smoke-test.sh relay|rendezvous <image>
#
# It needs a Linux host with Docker, openssl, curl and bash. It makes one network, a throwaway
# certificate and the containers it needs, all named after a random suffix, and removes them when
# it exits. It touches no other container.
set -euo pipefail

usage="usage: smoke-test.sh relay|rendezvous <image>"
kind="${1:?$usage}"
image="${2:?$usage}"
case "$kind" in
  relay | rendezvous) ;;
  *) echo "$usage" >&2; exit 2 ;;
esac
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Random, and not the process id, which two runs in two containers or namespaces may share.
name="sluice-smoke-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')"
etc="/etc/$kind"
work="$(mktemp -d)"
failed=0
runs=0

# Called by the trap below, which shellcheck does not follow. Every container is named after
# $name, so a run that timed out is removed here too.
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

# A name of its own for each container, so a cleanup finds it whatever it did.
next_name() { runs=$((runs + 1)); printf '%s-%s' "$name" "$runs"; }

# The address of a container on the network of this run.
address_of() {
  docker inspect --format "{{(index .NetworkSettings.Networks \"$name\").IPAddress}}" "$1"
}

# Whether something accepts a TCP connection at the address and port, within 3 seconds.
listens() {
  # The address and port are passed as $0 and $1, and not written into the command.
  # shellcheck disable=SC2016
  timeout 3 bash -c 'exec 3<> "/dev/tcp/$0/$1"' "$1" "$2" 2> /dev/null
}

# Whether the TLS server on a published port presents the throwaway certificate, which only this
# run's config names: iroh-relay's open default has no HTTPS at all.
presents_our_certificate() {
  local port="$1" served ours
  served="$(timeout 10 openssl s_client -connect "127.0.0.1:$port" -servername 127.0.0.1 \
    < /dev/null 2> /dev/null | openssl x509 -noout -fingerprint -sha256 2> /dev/null)" || return 1
  ours="$(openssl x509 -in "$work/certs/default.crt" -noout -fingerprint -sha256)"
  [ -n "$served" ] && [ "$served" = "$ours" ]
}

# Up with this config, and not with iroh-relay's open default, which also answers /generate_204 on
# port 80. Both images log the QUIC listener, which only a config that asks for it starts, and the
# relay's HTTPS port must present the certificate this run made.
up() {
  local container="$1" logs port
  # Into a variable first: with `grep -q` at the end of a pipe, grep exits on its first match,
  # docker gets SIGPIPE, and pipefail turns the match into a failure.
  logs="$(docker logs "$container" 2>&1)"
  grep -q 'QUIC server listening' <<< "$logs" || return 1
  [ "$kind" = relay ] || return 0
  port="$(docker port "$container" 80/tcp | head -n 1 | cut -d: -f2)"
  [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/generate_204")" = 204 ] \
    || return 1
  port="$(docker port "$container" 443/tcp | head -n 1 | cut -d: -f2)"
  presents_our_certificate "$port"
}

# The metrics server is on at port 9090 with no login unless the config turns it off. The relay's
# own HTTPS port is the control: if it cannot be reached from here either, the 9090 result means
# nothing.
metrics_are_off() {
  local container="$1" address
  address="$(address_of "$container")"
  if ! listens "$address" 443 && [ "$kind" = relay ]; then
    fail "metrics: the relay's port 443 cannot be reached from the test, so 9090 proves nothing"
  elif listens "$address" 9090; then
    fail "metrics: port 9090 is open"
  else
    pass "metrics: port 9090 is closed"
  fi
}

# Starts the image with a read-only root and the given arguments, and passes if it comes up as
# that config, with its HTTPS, its QUIC listener and no metrics port.
starts() {
  local check="$1" container publish=()
  container="$(next_name)"
  shift 1
  [ "$kind" = relay ] && publish=(-p 127.0.0.1::80 -p 127.0.0.1::443)
  docker run -d --read-only --name "$container" --network "$name" "${publish[@]}" \
    -v "$work/certs:$etc/certs:ro" "$image" "$@" > /dev/null
  local ok=no
  for _ in $(seq 1 50); do
    if up "$container"; then ok=yes; break; fi
    sleep 0.2
  done
  if [ "$ok" = yes ]; then
    pass "$check: up as this config, with a read-only root"
    metrics_are_off "$container"
  else
    fail "$check: did not come up as this config"
    docker logs "$container" 2>&1 | tail -n 5
  fi
  docker rm -f "$container" > /dev/null
}

# What the container's PID 1 is, as the host sees it: iroh-relay itself, started with the path of
# its own descriptor 0, and that descriptor is the sealed copy of the shipped config, not the file
# (ADR 0021). The copy's link names a `memfd:`, and its bytes are the shipped config's. The
# container runs with no `--security-opt`, so Docker's default seccomp profile is what allows the
# memfd and its seals, and with a read-only root. It runs as the host user so that the host may
# read /proc/<pid>/fd, and only here.
handover() {
  local container pid want cmdline
  container="$(next_name)"
  docker run -d --read-only --user "$(id -u):$(id -g)" --name "$container" --network "$name" \
    -v "$work/certs:$etc/certs:ro" "$image" > /dev/null
  for _ in $(seq 1 50); do up_quic "$container" && break; sleep 0.2; done
  pid="$(docker inspect --format '{{.State.Pid}}' "$container")"
  if [ ! -r "/proc/$pid/cmdline" ] || [ ! -r "/proc/$pid/fd/0" ]; then
    echo "SKIP  handover: /proc/$pid is not readable from here (not a native Linux engine)"
    docker rm -f "$container" > /dev/null
    return
  fi
  want="/usr/local/bin/iroh-relay --config-path=/proc/self/fd/0 "
  [ "$kind" = rendezvous ] && want="/usr/local/bin/iroh-relay --dev --config-path=/proc/self/fd/0 "
  cmdline="$(tr '\0' ' ' < "/proc/$pid/cmdline")"
  local shipped="$here/$kind/manual.toml"
  [ "$kind" = relay ] && shipped="$here/relay/relay.toml"
  local copy
  copy="$(readlink "/proc/$pid/fd/0")"
  if [ "$cmdline" = "$want" ] && [[ "$copy" == *"memfd:"* ]] \
    && cmp -s "/proc/$pid/fd/0" "$shipped"; then
    pass "handover: PID 1 is '${cmdline% }', and its descriptor 0 is the sealed copy of the config"
  else
    fail "handover: PID 1 is '$cmdline', descriptor 0: $copy"
  fi
  docker rm -f "$container" > /dev/null
}

# Only the QUIC line, which both images log when their config is the one that started.
up_quic() {
  local logs
  logs="$(docker logs "$1" 2>&1)"
  grep -q 'QUIC server listening' <<< "$logs"
}

# Runs the image with the given docker and guard arguments, and passes if it exits with 64 within
# 30 seconds and its one line of stderr holds the given text. A run of its own name, so that a
# container that does not stop is removed by the cleanup, and a check cannot hang the script.
refuses() {
  local check="$1" says="$2" code container line
  container="$(next_name)"
  shift 2
  docker run -d --name "$container" --network "$name" "$@" > /dev/null
  if ! code="$(timeout 30 docker wait "$container")"; then
    fail "$check: did not exit within 30 s"
    docker rm -f "$container" > /dev/null
    return
  fi
  docker logs "$container" 2>&1 > /dev/null | cat > "$work/stderr"
  docker rm -f "$container" > /dev/null
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
upper="$(printf 'AB%.0s' $(seq 1 32))"

# The checks both images share: a variable that is set and empty replaces the config's tokens with
# one that is empty, which iroh-relay would take as the only token.
both() {
  refuses "IROH_RELAY_ACCESS_TOKEN set but empty" "IROH_RELAY_ACCESS_TOKEN is set but empty" \
    -e IROH_RELAY_ACCESS_TOKEN= "$image"
}

relay() {
  starts "the shipped image"
  starts "open.toml with --open" --open --config-path /etc/relay/open.toml
  handover
  refuses "--open with relay.toml" "--open was given, but \`access\` is an allowlist" \
    "$image" --open --config-path /etc/relay/relay.toml
  refuses "open.toml without --open" "\`access\` is \"everyone\"" \
    "$image" --config-path /etc/relay/open.toml
  refuses "--open without a rate" "--open needs \`limits.client.rx.bytes_per_second\`" \
    -v "$(config norate "${metrics}access = \"everyone\"\n${tls}" relay.toml)" \
    "$image" --open --config-path /etc/relay/relay.toml
  refuses "--rendezvous with relay.toml" "needs \`enable_relay = false\`" \
    "$image" --rendezvous --config-path /etc/relay/relay.toml
  refuses "--dev" "--dev is refused" "$image" --dev --config-path /etc/relay/relay.toml
  refuses "a --config-path that does not exist" "does not exist" \
    "$image" --config-path /etc/relay/missing.toml
  relay_configs
  both
}

# Configs that parse, and that iroh-relay would run in a way the operator did not write.
relay_configs() {
  refuses "a mounted config with [acess]" "unknown key \`acess\`" \
    -v "$(config acess "${metrics}[acess]\nallowlist = []\n" relay.toml)" "$image"
  refuses "a denylist without --open" "\`access\` is a denylist" \
    -v "$(config deny "${metrics}[access]\ndenylist = []\n${tls}" relay.toml)" "$image"
  refuses "no access" "\`access\` is not set" \
    -v "$(config none "${metrics}${tls}" relay.toml)" "$image"
  refuses "dangerous_http_only" "\`dangerous_http_only\` is true" \
    -v "$(config http "${metrics}${allow}${tls}dangerous_http_only = true\n" relay.toml)" "$image"
  refuses "no [tls] without --behind-proxy" "there is no [tls] table" \
    -v "$(config notls "${metrics}${allow}" relay.toml)" "$image"
  refuses "--behind-proxy with relay.toml" "--behind-proxy was given" \
    "$image" --behind-proxy --config-path /etc/relay/relay.toml
  refuses "no enable_metrics" "\`enable_metrics\` is not set" \
    -v "$(config metrics "${allow}${tls}" relay.toml)" "$image"
  refuses "an HTTP check with a query" "\`access.http.url\` holds a query" \
    -v "$(config query "${metrics}[access.http]\nurl = \"https://a.example/?k=v\"\n" \
      relay.toml)" "$image"
  refuses "an HTTP check with a user name" "holds a user name or password" \
    -v "$(config user "${metrics}[access.http]\nurl = \"https://user@a.example/\"\n" \
      relay.toml)" "$image"
  refuses "an upper-case allowlist id" "has upper-case hex digits" \
    -v "$(config upper "${metrics}[access]\nallowlist = [\"$upper\"]\n" relay.toml)" "$image"
  refuses "shared_token = []" "\`access.shared_token\` is empty" \
    -v "$(config tokens "${metrics}[access]\nshared_token = []\n" relay.toml)" "$image"
}

rendezvous() {
  starts "the shipped image"
  handover
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
  both
}

case "$kind" in
  relay) relay ;;
  rendezvous) rendezvous ;;
esac
exit "$failed"
