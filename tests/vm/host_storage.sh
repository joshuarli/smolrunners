#!/bin/sh
set -eu

runtime=/opt/runtime
state=/work/tmp/host-storage-state
binaries=/work/smolrunners/tests/vm/bin
agent=$binaries/smolrunners-storage
image='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'

fail() { printf 'host_storage: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --error-format json --state-dir "$state/smolvm" "$@"
}

host() {
  "$agent" host --state-dir "$state" --runtime-dir "$runtime" \
    --launcher "$binaries/smolrunners-launcher" "$@"
}

leftovers() {
  [ -d "$state/smolvm" ] || return 0
  smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}'
}

cleanup() {
  for machine in $(leftovers); do
    smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
    smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
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
    smolvm machine status --name "$1" >/dev/null 2>&1 && return 0
    sleep 1
    count=$((count + 1))
  done
  fail "timeout waiting for machine $1"
}

check_record() {
  job=$1
  name=$2
  report="$state/$job.report.json"
  host storage record --job "$job" --machine "$name" >/dev/null
  host storage report --job "$job" >"$report"
  smolvm machine status --name "$name" --json >"$state/$name.status.json"
  data_dir=$(smolvm machine data-dir --name "$name")
  raw="$data_dir/storage.raw"
  seed=$(cat "$data_dir/storage.seed")
  stat_size=$(stat -c %s "$raw")
  stat_blocks=$(stat -c %b "$raw")
  stat_blksize=$(stat -c %B "$raw")
  inode_count=$(dumpe2fs -h "$raw" 2>/dev/null | awk -F: '/^Inode count:/ {gsub(/ /,"",$2); print $2}')
  filesystem_uuid=$(dumpe2fs -h "$raw" 2>/dev/null | awk -F: '/^Filesystem UUID:/ {gsub(/^ +/,"",$2); print $2}')
  python3 - "$report" "$state/$name.status.json" "$raw" "$seed" \
    "$stat_size" "$stat_blocks" "$stat_blksize" "$inode_count" "$filesystem_uuid" <<'PY'
import json
import sys

report, status, raw, seed, size, blocks, blksize, inodes, uuid = sys.argv[1:10]
doc = json.load(open(report))
state = json.load(open(status))
allocated = int(blocks) * int(blksize)
expected_roles = {"workspace": "/workspace", "docker": "/var/lib/docker", "containerd": "/var/lib/containerd"}
seen = {}
for volume in doc["volumes"]:
    role = volume["role"]
    seen[role] = volume
    assert expected_roles.get(role) == volume["guest_path"], (role, volume["guest_path"])
    assert volume["format"] == "ext4", (role, volume["format"])
    assert volume["disk_format"] == "raw", (role, volume["disk_format"])
    assert volume["backing_chain"][1] == seed, (role, "seed")
    assert volume["backing_chain"][2] == raw, (role, "raw")
    assert volume["disk_path"] == raw, (role, "disk_path")
    assert volume["apparent_bytes"] == int(size), (role, "size")
    assert volume["allocated_bytes"] == allocated, (role, "allocated")
    assert volume["inode_capacity"] == int(inodes), (role, "inodes")
    assert volume["filesystem_uuid"] == uuid, (role, "uuid")
assert set(seen) == set(expected_roles), seen
assert doc["storage_gb"] == state["storage_gb"], (doc["storage_gb"], state["storage_gb"])
assert allocated < int(size), "storage image is not sparse"
print("host_storage: record matches smolvm and stat for " + doc["job"])
PY
  verify="$state/$job.verify.json"
  host storage verify --job "$job" >"$verify"
  grep -q '"verified": true' "$verify" || fail "verify failed for $job: $(cat "$verify")"
}

[ -x "$agent" ] || fail "missing storage fixture at $agent"
[ -x "$runtime/smolvm" ] || fail "missing pinned runtime at $runtime/smolvm"
command -v dumpe2fs >/dev/null 2>&1 || fail "missing dumpe2fs; ext4 format evidence is required"

rm -rf "$state"
mkdir -p "$state/home" "$state/tmp" "$state/home/.config" "$state/home/.cache"
for stale in /sys/fs/cgroup/sr-*; do
  [ -d "$stale" ] && rmdir "$stale" 2>/dev/null || true
done

printf 'host_storage: over-budget admission is refused with a reason\n'
set +e
over="$(host storage admit --job overbudget --bytes 999999999999999 --inodes 999999999 2>&1)"
over_code=$?
set -e
[ "$over_code" -ne 0 ] || fail "over-budget job was admitted: $over"
printf '%s\n' "$over" | grep -q 'exceeds available' || fail "refusal has no byte/inode reason: $over"
printf '%s\n' "$over" | grep -q '"admitted": false' || fail "refusal has no structured verdict: $over"

printf 'host_storage: admitting two jobs\n'
host storage admit --job job-a --bytes 2147483648 --inodes 131072 >"$state/job-a.admission.json"
host storage admit --job job-b --bytes 2147483648 --inodes 131072 >"$state/job-b.admission.json"
grep -q '"admitted": true' "$state/job-a.admission.json" || fail "job-a not admitted"
grep -q '"admitted": true' "$state/job-b.admission.json" || fail "job-b not admitted"
probe_dir="$state/reflink-probe"
mkdir -p "$probe_dir"
printf 'probe' >"$probe_dir/source"
if cp --reflink=always "$probe_dir/source" "$probe_dir/target" 2>/dev/null; then
  expected_allocation=reflink
else
  expected_allocation=sparse
  printf 'host_storage: prerequisite: reflink unavailable on ext4 /work; sparse accounting is recorded instead\n'
fi
grep -q "\"allocation\": \"$expected_allocation\"" "$state/job-a.admission.json" \
  || fail "allocation method does not match filesystem capability"

printf 'host_storage: provisioning two jobs\n'
host storage provision --job job-a --machine sr-storage-a --image "$image" \
  --storage 2 --overlay 1 >"$state/job-a.provision.json"
host storage provision --job job-b --machine sr-storage-b --image "$image" \
  --storage 2 --overlay 1 >"$state/job-b.provision.json"
wait_for_machine sr-storage-a
wait_for_machine sr-storage-b

printf 'host_storage: three ext4 writable paths per guest\n'
for name in sr-storage-a sr-storage-b; do
  mounts=$(smolvm machine exec --name "$name" -- sh -c 'cat /proc/mounts' 2>&1)
  for path in /workspace /var/lib/docker /var/lib/containerd; do
    printf '%s\n' "$mounts" | awk -v p="$path" '$2 == p && $3 == "ext4" {found=1} END {exit !found}' \
      || fail "$name has no ext4 mount at $path"
  done
done

printf 'host_storage: writing distinct markers\n'
for pair in 'sr-storage-a:a' 'sr-storage-b:b'; do
  name=${pair%:*}
  tag=${pair#*:}
  smolvm machine exec --name "$name" -- sh -c \
    "echo $tag > /workspace/marker; echo $tag > /var/lib/docker/marker; echo $tag > /var/lib/containerd/marker"
done
a_seen=$(smolvm machine exec --name sr-storage-a -- sh -c \
  'cat /workspace/marker /var/lib/docker/marker /var/lib/containerd/marker' 2>&1 | tr '\n' ' ')
b_seen=$(smolvm machine exec --name sr-storage-b -- sh -c \
  'cat /workspace/marker /var/lib/docker/marker /var/lib/containerd/marker' 2>&1 | tr '\n' ' ')
[ "$a_seen" = 'a a a ' ] || fail "job-a sees foreign writable state: $a_seen"
[ "$b_seen" = 'b b b ' ] || fail "job-b sees foreign writable state: $b_seen"

printf 'host_storage: recorded format and backing chain\n'
check_record job-a sr-storage-a
check_record job-b sr-storage-b
python3 - "$state/job-a.report.json" "$state/job-b.report.json" <<'PY'
import json
import sys

a = json.load(open(sys.argv[1]))
b = json.load(open(sys.argv[2]))
a_raw = a["volumes"][0]["disk_path"]
b_raw = b["volumes"][0]["disk_path"]
assert a_raw != b_raw, "two jobs share the same raw disk"
assert a["machine"] != b["machine"], "two jobs share a machine"
assert a["volumes"][0]["backing_chain"][2] != b["volumes"][0]["backing_chain"][2]
print("host_storage: writable disks are distinct per job")
PY

printf 'host_storage: clean teardown\n'
for name in sr-storage-a sr-storage-b; do
  smolvm machine stop --name "$name" >/dev/null 2>&1 || true
  smolvm machine delete --name "$name" -f >/dev/null 2>&1 || true
done
wait_for_gone() {
  count=0
  while [ "$count" -lt 60 ]; do
    smolvm machine status --name "$1" >/dev/null 2>&1 || return 0
    sleep 1
    count=$((count + 1))
  done
  fail "machine $1 still present"
}
wait_for_gone sr-storage-a
wait_for_gone sr-storage-b
assert_clean

printf 'host_storage: ok\n'
