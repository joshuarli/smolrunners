#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/image-docker-state

fail() { printf 'image_docker: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --state-dir "$state/smolvm" "$@"
}

cleanup() {
  smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}' | while IFS= read -r name; do
    smolvm machine stop --name "$name" >/dev/null 2>&1 || true
    smolvm machine delete --name "$name" -f >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

wait_machine() {
  i=0
  while [ "$i" -lt 90 ]; do
    smolvm machine status --name "$1" >/dev/null 2>&1 && return 0
    i=$((i + 1))
    sleep 1
  done
  fail "machine $1 did not reach running state"
}

exec_guest() {
  name=$1
  shift
  smolvm machine exec --name "$name" --timeout 20m -- "$@"
}

rm -rf "$state"
mkdir -p "$state/tmp" "$state/home/.config" "$state/home/.cache" \
  "$state/rootfs" "$state/cache"

printf 'image_docker: building debian13 rootfs\n'
sh "$repo/images/debian13/build.sh" "$state/rootfs" "$state/cache"

mkdir -p "$state/rootfs/opt/docker-corpus" "$state/rootfs/etc/ssl/certs"
cp -a "$repo/tests/fixtures/docker-corpus/." "$state/rootfs/opt/docker-corpus/"
cp /etc/ssl/certs/ca-certificates.crt "$state/rootfs/etc/ssl/certs/ca-certificates.crt"

printf 'image_docker: booting the corpus guest on virtio-net with no published ports\n'
smolvm machine run -d --name sr-corpus-main -I "$state/rootfs/" \
  --net --net-backend virtio-net \
  -v /var/lib/docker \
  -- /bin/sleep 7200 > "$state/main-boot.log" 2>&1 || \
  fail "corpus guest boot failed: $(tail -10 "$state/main-boot.log")"
wait_machine sr-corpus-main

exec_guest sr-corpus-main sh /opt/docker-corpus/corpus.sh > "$state/corpus.log" 2>&1 || \
  fail "corpus failed: $(tail -30 "$state/corpus.log")"
cat "$state/corpus.log"

printf 'image_docker: two concurrent guests binding identical service ports\n'
for tag in a b; do
  smolvm machine run -d --name "sr-corpus-port-$tag" -I "$state/rootfs/" \
    --net --net-backend virtio-net \
    -v /var/lib/docker \
    -- /bin/sleep 7200 > "$state/port-$tag-boot.log" 2>&1 || \
    fail "port guest $tag boot failed: $(tail -10 "$state/port-$tag-boot.log")"
  wait_machine "sr-corpus-port-$tag"
  exec_guest "sr-corpus-port-$tag" sh /opt/docker-corpus/service.sh "$tag" \
    > "$state/port-$tag.log" 2>&1 || \
    fail "port guest $tag service failed: $(tail -30 "$state/port-$tag.log")"
  cat "$state/port-$tag.log"
done

grep -q '^guest=a .*response=PORT_GUEST_a$' "$state/port-a.log" || fail 'guest a did not serve on port 18080'
grep -q '^guest=b .*response=PORT_GUEST_b$' "$state/port-b.log" || fail 'guest b did not serve on port 18080'

printf 'image_docker: environment prerequisites, not local passes: Docker corpus items 3-5 need live GitHub; run make live-corpus\n'

printf 'image_docker: identical ports are not published on the host\n'
set +e
curl -fsS --max-time 3 http://127.0.0.1:18080/ > "$state/host-port.log" 2>&1
host_code=$?
set -e
[ "$host_code" -ne 0 ] || fail "port 18080 was published on the host: $(cat "$state/host-port.log")"
if command -v ss >/dev/null 2>&1; then
  ss -ltn 2>/dev/null | awk '$4 ~ /:18080$/ {found = 1} END {exit !found}' && \
    fail 'the host has a listener on port 18080'
fi

leftovers="$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}')"
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"

printf 'image_docker: ok\n'
