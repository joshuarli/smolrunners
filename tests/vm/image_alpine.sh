#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/image-alpine-state
cache=/work/tmp/image-alpine-cache
machine=sr-alpine-image

fail() { printf 'image_alpine: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --state-dir "$state/smolvm" "$@"
}

exec_guest() { smolvm machine exec --name "$machine" --timeout 10m "$@"; }

cleanup() {
  smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
  smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_machine() {
  i=0
  while [ "$i" -lt 90 ]; do
    smolvm machine status --name "$machine" >/dev/null 2>&1 && return 0
    i=$((i + 1))
    sleep 1
  done
  fail "machine $machine did not reach running state"
}

rm -rf "$state"
mkdir -p "$state/tmp" "$state/home/.config" "$state/home/.cache" "$state/rootfs" "$cache"

[ -x "$runtime/smolvm" ] || fail 'missing pinned runtime'

printf 'image_alpine: building alpine324 rootfs\n'
sh "$repo/images/alpine324/build.sh" "$state/rootfs" "$cache"

printf 'image_alpine: booting the Alpine guest\n'
smolvm machine run -d --name "$machine" -I "$state/rootfs/" \
  -- /bin/sleep 7200 > "$state/boot.log" 2>&1 || \
  fail "guest boot failed: $(tail -10 "$state/boot.log")"
wait_machine

printf 'image_alpine: proving a real Alpine guest, not Debian under an Alpine name\n'
release=$(exec_guest -- cat /etc/alpine-release) || fail 'guest has no /etc/alpine-release'
case "$release" in
  3.24.*) ;;
  *) fail "unexpected Alpine release: $release" ;;
esac
printf 'image_alpine: /etc/alpine-release=%s\n' "$release"
exec_guest -- /bin/sh -c '
  set -eu
  export PATH=/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
  grep -q "^ID=alpine$" /etc/os-release
  test -x /sbin/apk
  test ! -e /etc/debian_version
  test ! -x /usr/bin/dpkg
  if command -v apt-get >/dev/null 2>&1; then echo apt-get present >&2; exit 1; fi
  printf "identity: alpine=%s apk=%s\n" "$(cat /etc/alpine-release)" "$(apk --version 2>&1 | head -1)"
' > "$state/identity.log" 2>&1 || fail "identity check: $(cat "$state/identity.log")"
cat "$state/identity.log"

printf 'image_alpine: collector runs on Alpine\n'
collector_help=$(exec_guest -- /opt/smolrunners/bin/smolrunners-collector --help) || \
  fail 'collector --help failed in the Alpine guest'
printf '%s\n' "$collector_help" | grep -q 'usage: smolrunners-collector' || \
  fail "unexpected collector --help: $collector_help"
exec_guest -- /bin/sh -c '
  set -eu
  /opt/smolrunners/bin/smolrunners-collector --interval-ms 200 --batch-lines 3 \
    > /tmp/col.log 2>&1 &
  pid=$!
  sleep 3
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  grep -q "cpu.busy_cores" /tmp/col.log
  grep -q "\"scope\":\"workload\"" /tmp/col.log
  printf "collector samples: %s lines\n" "$(wc -l < /tmp/col.log)"
' > "$state/collector.log" 2>&1 || fail "collector sampling: $(cat "$state/collector.log")"
cat "$state/collector.log"

printf 'image_alpine: Docker works inside the Alpine guest\n'
exec_guest -e HOME=/tmp -e DOCKER_CONFIG=/tmp/.docker -- /bin/sh -c '
  set -eu
  export PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin
  mkdir -p /sys/fs/cgroup/smolrunners-workload.slice /work/docker /work/docker-exec /var/run /tmp/.docker
  dockerd --data-root=/work/docker --exec-root=/work/docker-exec \
    --host=unix:///var/run/docker.sock --pidfile=/work/docker.pid \
    --cgroup-parent=smolrunners-workload.slice --exec-opt native.cgroupdriver=cgroupfs \
    --storage-driver=vfs --iptables=false --ip6tables=false > /work/dockerd.log 2>&1 &
  ready=0
  attempt=0
  while [ "$attempt" -lt 60 ]; do
    if docker info >/dev/null 2>&1; then ready=1; break; fi
    attempt=$((attempt + 1))
    sleep 1
  done
  [ "$ready" -eq 1 ] || { tail -30 /work/dockerd.log >&2; exit 1; }
  printf "docker server=%s driver=%s\n" \
    "$(docker info --format "{{.ServerVersion}}")" \
    "$(docker info --format "{{.Driver}}")"
  (cd / && tar -cf - opt/smolrunners/bin/smolrunners-collector) | \
    docker import - sr-alpine-probe > /dev/null
  out=$(docker run --rm sr-alpine-probe /opt/smolrunners/bin/smolrunners-collector --help)
  printf "%s\n" "$out" | grep -q "usage: smolrunners-collector" || {
    echo "container output unexpected: $out" >&2
    exit 1
  }
  docker image rm sr-alpine-probe > /dev/null
  printf "docker container: %s\n" "$out"
' > "$state/docker.log" 2>&1 || fail "docker proof: $(tail -30 "$state/docker.log")"
cat "$state/docker.log"

printf 'image_alpine: verifying the installed manifest in the guest\n'
exec_guest -- /bin/sh /opt/smolrunners/verify.sh

printf 'image_alpine: no sr-* resources survive\n'
smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
leftovers="$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}')"
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"

printf 'image_alpine: ok\n'
