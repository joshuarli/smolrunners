#!/bin/sh
set -eu

runtime=/opt/runtime
repo=/work/smolrunners
fixture=$repo/crates/smolrunners/src/host/cache/smolrunners-cache
state=/work/tmp/cache-docker-state
image='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'
machine=sr-cache-docker

fail() { printf 'cache_docker: %s\n' "$*" >&2; exit 1; }
note() { printf 'cache_docker: %s\n' "$*"; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --error-format json --state-dir "$state/smolvm" "$@"
}

cache() { "$fixture" --state "$state/cache" "$@"; }

jassert() {
  document=$1
  expression=$2
  printf '%s' "$document" | python3 -c '
import json, sys
doc = json.load(sys.stdin)
assert eval(sys.argv[1]), (sys.argv[1], doc)
' "$expression" || fail "assertion failed: $expression for $document"
}

leftovers() {
  [ -d "$state/smolvm" ] || return 0
  smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}'
}

cleanup() {
  for name in $(leftovers); do
    smolvm machine stop --name "$name" >/dev/null 2>&1 || true
    smolvm machine delete --name "$name" -f >/dev/null 2>&1 || true
  done
  for cgroup in /sys/fs/cgroup/sr-*; do
    [ -d "$cgroup" ] && rmdir "$cgroup" 2>/dev/null || true
  done
}
trap cleanup EXIT

assert_clean() {
  remaining="$(leftovers)"
  [ -z "$remaining" ] || fail "leftover machines: $remaining"
  cgroups="$(ls /sys/fs/cgroup 2>/dev/null | awk '/^sr-/ {print}')"
  [ -z "$cgroups" ] || fail "leftover cgroups: $cgroups"
}

wait_for_machine() {
  count=0
  while [ "$count" -lt 60 ]; do
    if smolvm machine exec --name "$1" --timeout 10sec -- true >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
    count=$((count + 1))
  done
  fail "timeout waiting for machine $1"
}

note 'prepare nested-host state and rootfs'
[ -x "$fixture" ] || fail "missing cache fixture at $fixture"
[ -x "$runtime/smolvm" ] || fail "missing pinned runtime at $runtime/smolvm"
rm -rf "$state"
mkdir -p "$state/tmp" "$state/home/.config" "$state/home/.cache" \
  "$state/rootfs" "$state/cacheroot" "$state/cache"
for stale in /sys/fs/cgroup/sr-*; do
  [ -d "$stale" ] && rmdir "$stale" 2>/dev/null || true
done

sh "$repo/images/debian13/build.sh" "$state/rootfs" "$state/cacheroot"

mkdir -p "$state/rootfs/opt/docker-corpus" "$state/rootfs/opt/smolrunners/bin" \
  "$state/rootfs/etc/ssl/certs"
cp -a "$repo/tests/fixtures/docker-corpus/." "$state/rootfs/opt/docker-corpus/"
cp "$fixture" "$state/rootfs/opt/smolrunners/bin/smolrunners-cache"
chmod 0755 "$state/rootfs/opt/smolrunners/bin/smolrunners-cache"
cp /etc/ssl/certs/ca-certificates.crt "$state/rootfs/etc/ssl/certs/ca-certificates.crt"

cat > "$state/cache/guest.sh" <<'GUEST'
#!/bin/sh
set -eu

fail() { printf 'cache_docker guest: %s\n' "$*" >&2; exit 1; }
note() { printf 'cache_docker guest: %s\n' "$*"; }
cache=/opt/smolrunners/bin/smolrunners-cache
c() { "$cache" --state /work/cache "$@"; }
. /opt/docker-corpus/lib.sh

note 'initialize the host-owned cache state'
c init > /work/cache/init.json
c quota --generations 16 --bytes 1000000000 > /dev/null

note 'cold daemon: pull, BuildKit build and seed disposable state'
start_daemon wipe
assert_data_root
docker pull -q alpine:3.24 > /dev/null

mkdir -p /work/build
cat > /work/build/Dockerfile <<'EOF'
FROM alpine:3.24
RUN echo corpus-build-v1 > /marker
CMD ["/bin/cat", "/marker"]
EOF
DOCKER_BUILDKIT=1 docker buildx build --builder default --progress=plain \
  --cache-to=type=inline \
  -t sr-docker-cache:v1 /work/build > /work/cold-build.log 2>&1 || \
  fail "cold build failed: $(tail -15 /work/cold-build.log)"
marker=$(docker run --rm sr-docker-cache:v1)
[ "$marker" = corpus-build-v1 ] || fail "unexpected cold marker: $marker"

docker rm -f sr-cold-canary >/dev/null 2>&1 || true
docker run -d --name sr-cold-canary alpine:3.24 sleep 3600 >/dev/null
docker volume create sr-cold-vol >/dev/null
mkdir -p /tmp/.docker
printf '%s\n' '{"auths":{"127.0.0.1:15000":{"auth":"c3ItY29sZDp0b2tlbg=="}}}' > /tmp/.docker/config.json
old_id=$(docker info --format '{{.ID}}')
[ -n "$old_id" ] || fail 'cold daemon had no identity'

note 'stage reusable Docker and BuildKit artifacts'
mkdir -p /work/cache/stage
docker save -o /work/cache/stage/image.tar alpine:3.24 sr-docker-cache:v1
tar_digest=$(sha256sum /work/cache/stage/image.tar | awk '{print $1}')
printf '%s\n' '{"backend":"buildkit-inline","image":"sr-docker-cache:v1","storage_driver":"vfs"}' \
  > /work/cache/stage/build-metadata.json
printf '%s\n' "{\"build_marker\":\"$marker\",\"image_digest\":\"$tar_digest\",\"cache_mode\":\"inline\"}" \
  > /work/cache/cold-output.json

note 'drain, flush and cleanly unmount the daemon'
docker rm -f sr-cold-canary >/dev/null
docker volume rm sr-cold-vol >/dev/null
rm -f /tmp/.docker/config.json
[ -z "$(docker ps -aq)" ] || fail 'containers survived the drain'
[ -z "$(docker volume ls -q)" ] || fail 'volumes survived the drain'
docker_dev=$(awk '$2 == "/var/lib/docker" {print $1}' /proc/mounts)
[ -n "$docker_dev" ] || fail 'cannot resolve the docker data-root device'
stop_daemon
pkill -f 'containerd --config' 2>/dev/null || true
sleep 2
sync
awk '$2 == "/var/lib/docker" {found = 1} END {exit !found}' /proc/mounts || \
  fail 'data root was not mounted before unmount'
unmount_count=0
while awk '$2 == "/var/lib/docker" {found = 1} END {exit !found}' /proc/mounts; do
  umount /var/lib/docker || fail "data-root unmount failed: $(awk '$2 == "/var/lib/docker"' /proc/mounts)"
  unmount_count=$((unmount_count + 1))
  [ "$unmount_count" -lt 8 ] || fail 'too many stacked data-root mounts'
done
note "data root unmounted after $unmount_count umount calls"
cat > /work/cache/drain.json <<EOF
{
  "schema_version": 1,
  "job": "cold",
  "drained": {"running_containers": 0, "services": 0, "volumes": 0},
  "removed": {
    "containers": ["sr-cold-canary"],
    "volumes": ["sr-cold-vol"],
    "credentials": ["/tmp/.docker/config.json auth"],
    "identities": ["$old_id"]
  },
  "flush": {"sync_complete": true, "build_cache_exported": true, "images_saved": true},
  "unmount": {"data_root": "/var/lib/docker", "unmounted": true, "remounted_clean": true},
  "data_root_ext4": true,
  "image_store": "vfs"
}
EOF
c docker-drain --job cold --repo acme/widgets --tier maintainer \
  --evidence /work/cache/drain.json > /work/cache/drain-out.json

note 'publish the Docker generation through the D1 generation store'
staged=$(c docker-stage --name warm-artifacts --source /work/cache/stage \
  --repo acme/widgets --tier maintainer)
generation=$(printf '%s' "$staged" | sed -n 's/.*"generation":"\([^"]*\)".*/\1/p')
[ -n "$generation" ] || fail "staging produced no generation: $staged"
printf '%s\n' "$staged" | grep -Eq '"files":([2-9]|[1-9][0-9]+)' || fail "staging produced too few files: $staged"
printf '%s\n' "$staged" | grep -Eq '"bytes":[1-9]' || fail "staging produced no bytes: $staged"
c docker-publish --gen "$generation" --repo acme/widgets > /work/cache/publish.json
c current --repo acme/widgets | grep -q "\"generation\":\"$generation\"" || fail 'current generation did not match'
c inspect --gen "$generation" | grep -q '"state":"published"' || fail 'generation was not published'

note 'remount a wiped data root and start a fresh daemon'
mount "$docker_dev" /var/lib/docker || fail 'data root remount failed'
find /var/lib/docker -mindepth 1 -maxdepth 1 -exec rm -rf {} + 2>/dev/null || true
start_daemon wipe
assert_data_root
[ -z "$(docker ps -aq)" ] || fail 'prior containers appeared after reuse'
[ -z "$(docker volume ls -q)" ] || fail 'prior volumes appeared after reuse'
[ ! -f /tmp/.docker/config.json ] || fail 'prior credentials appeared after reuse'
new_id=$(docker info --format '{{.ID}}')
[ "$new_id" != "$old_id" ] || fail 'daemon identity survived reuse'
printf '%s\n' '{"containers":0,"services":0,"volumes":0,"credentials":0,"daemon_identity":"'"$new_id"'"}' \
  > /work/cache/prior-state.json

note 'warm reuse: transfer, unpack and build are measured separately'
transfer=$(c docker-transfer --job warm --gen "$generation" --repo acme/widgets \
  --tier maintainer --target /work/reuse)
printf '%s\n' "$transfer" | grep -q '"granted":true' || fail 'transfer was not granted'
printf '%s\n' "$transfer" | grep -Eq '"files":([2-9]|[1-9][0-9]+)' || fail 'transfer produced too few files'
printf '%s\n' "$transfer" | grep -Eq '"bytes":[1-9]' || fail 'transfer produced no bytes'
printf '%s\n' "$transfer" | grep -Eq '"transfer_ms":[0-9]+' || fail 'transfer timing missing'
unpack_start=$(date +%s%3N)
docker load -i /work/reuse/image.tar > /dev/null
unpack_ms=$(( $(date +%s%3N) - unpack_start ))
build_start=$(date +%s%3N)
DOCKER_BUILDKIT=1 docker buildx build --builder default --progress=plain \
  --cache-from=type=inline \
  -t sr-docker-cache:v1 /work/build > /work/warm-build.log 2>&1 || \
  fail "warm build failed: $(tail -15 /work/warm-build.log)"
build_ms=$(( $(date +%s%3N) - build_start ))
warm_marker=$(docker run --rm sr-docker-cache:v1)
warm_digest=$(sha256sum /work/reuse/image.tar | awk '{print $1}')
printf '%s\n' "{\"build_marker\":\"$warm_marker\",\"image_digest\":\"$warm_digest\",\"cache_mode\":\"inline\"}" \
  > /work/cache/warm-output.json
c docker-complete --job warm --unpack-ms "$unpack_ms" --build-ms "$build_ms" \
  --output /work/cache/warm-output.json --prior /work/cache/prior-state.json \
  > /work/cache/complete-out.json

note 'verify identical outputs and D1-consistent publication'
c docker-verify --output /work/cache/cold-output.json \
  --prior /work/cache/warm-output.json --gen "$generation" > /work/cache/verify.json
python3 -c '
import json
doc = json.load(open("/work/cache/verify.json"))
assert doc["identical"] is True and doc["state"] == "published", doc
assert doc["current"] == "'"$generation"'", doc
'
stop_daemon
pkill -f 'containerd --config' 2>/dev/null || true
GUEST
chmod 0644 "$state/cache/guest.sh"

note 'boot the corpus guest on virtio-net with no published ports'
chmod -R a+rwX "$state"
smolvm machine run -d --name "$machine" -I "$state/rootfs/" \
  --net --net-backend virtio-net \
  -v /var/lib/docker \
  -v "$state/cache:/work/cache:rw" \
  -- /bin/sleep 3600 > "$state/boot.log" 2>&1 || \
  fail "guest boot failed: $(tail -15 "$state/boot.log")"
wait_for_machine "$machine"

smolvm machine exec --name "$machine" --timeout 20m -- sh /work/cache/guest.sh \
  > "$state/guest.log" 2>&1 || fail "guest workload failed: $(tail -40 "$state/guest.log")"
tail -40 "$state/guest.log"

note 'assert Docker outcomes appear in job history'
cache docker-history > "$state/docker-history.json"
jassert "$(cat "$state/docker-history.json")" 'len(doc) == 2'
python3 - "$state/docker-history.json" "$state/cache/store.sqlite" <<'PY'
import json
import sqlite3
import sys

rows = json.load(open(sys.argv[1]))
by_action = {row["payload"]["action"]: row for row in rows}
assert set(by_action) == {"drain", "reuse"}, by_action
drain = by_action["drain"]["payload"]
reuse = by_action["reuse"]["payload"]
assert drain["drain"]["containers_removed"] == ["sr-cold-canary"], drain
assert drain["drain"]["volumes_removed"] == ["sr-cold-vol"], drain
assert drain["drain"]["credentials_removed"] and drain["drain"]["identities_removed"], drain
assert drain["drain"]["unmounted"] is True and drain["drain"]["data_root_ext4"] is True, drain
assert drain["drain"]["image_store"] == "vfs", drain
phases = reuse["phases_ms"]
assert set(phases) == {"transfer", "unpack", "build"}, phases
assert all(isinstance(phases[key], int) for key in phases), phases
assert reuse["output"]["build_marker"] == "corpus-build-v1", reuse
assert reuse["prior_state"]["containers"] == 0, reuse
assert reuse["prior_state"]["credentials"] == 0, reuse

db = sqlite3.connect(sys.argv[2])
events = db.execute(
    "SELECT e.kind, e.payload_json FROM event e WHERE e.kind = 'docker-cache-outcome' ORDER BY e.id"
).fetchall()
assert len(events) == 2, events
standard = db.execute(
    "SELECT count(*) FROM event WHERE kind = 'cache-outcome'"
).fetchone()[0]
assert standard >= 2, standard
print("cache_docker: job history carries %d Docker outcomes and %d cache outcomes" % (len(events), standard))
PY

note 'assert no leftover sr-* resources'
smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
count=0
while [ "$count" -lt 60 ]; do
  smolvm machine status --name "$machine" >/dev/null 2>&1 || break
  sleep 1
  count=$((count + 1))
done
assert_clean
printf 'cache_docker: ok\n'
