#!/bin/sh
set -eu

runtime=/opt/runtime
state=/work/tmp/host-boundary-state
binaries=/work/smolrunners/tests/vm/bin
agent=$binaries/smolrunners-doctor
launcher=$binaries/smolrunners-launcher
image='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'

fail() { printf 'host_boundary: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --error-format json --state-dir "$state/smolvm" "$@"
}

leftovers() {
  [ -d "$state/smolvm" ] || return 0
  smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}'
}

background_pids=''
cleanup() {
  for pid in $background_pids; do
    kill "$pid" 2>/dev/null || true
  done
  for machine in $(leftovers); do
    smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
    smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
  done
  for cgroup in /sys/fs/cgroup/sr-*; do
    [ -d "$cgroup" ] && rmdir "$cgroup" 2>/dev/null || true
  done
}
trap cleanup EXIT

reset_state() {
  rm -rf "$state"
  mkdir -p "$state/home" "$state/tmp" "$state/home/.config" "$state/home/.cache"
  for stale in /sys/fs/cgroup/sr-*; do
    [ -d "$stale" ] && rmdir "$stale" 2>/dev/null || true
  done
}

host() {
  "$agent" host --state-dir "$state" --runtime-dir "$runtime" --launcher "$launcher" "$@"
}

doctor() {
  "$agent" doctor --state-dir "$state" --runtime-dir "$runtime" --json
}

wait_for() {
  pattern=$1
  file=$2
  limit=$3
  count=0
  while [ "$count" -lt "$limit" ]; do
    grep -q "$pattern" "$file" 2>/dev/null && return 0
    sleep 1
    count=$((count + 1))
  done
  fail "timeout waiting for $pattern in $file"
}

verdict() {
  awk -v want="$2" '
    $1 == "\"name\":" && $2 == "\"" want "\"," { found=1; next }
    found && $1 == "\"verdict\":" { gsub(/[",]/, "", $2); print $2; exit }
  ' "$1"
}

assert_verdict() {
  file=$1
  name=$2
  expected=$3
  actual=$(verdict "$file" "$name")
  [ "$actual" = "$expected" ] || fail "$name: expected $expected, got ${actual:-missing} in $file"
}

assert_schema() {
  grep -q '"schema_version": 1' "$1" || fail "missing versioned schema in $1"
  grep -q '"kind": "doctor"' "$1" || fail "missing doctor kind in $1"
}

assert_clean() {
  remaining="$(leftovers)"
  [ -z "$remaining" ] || fail "leftover machines: $remaining"
  markers="$(ls -A "$state/markers" 2>/dev/null || true)"
  [ -z "$markers" ] || fail "leftover markers: $markers"
  cgroups="$(ls /sys/fs/cgroup 2>/dev/null | awk '/^sr-/ {print}')"
  [ -z "$cgroups" ] || fail "leftover cgroups: $cgroups"
}

[ -x "$agent" ] || fail "missing doctor binary at $agent"
[ -x "$launcher" ] || fail "missing launcher binary at $launcher"
[ -x "$runtime/smolvm" ] || fail "missing pinned runtime at $runtime/smolvm"

printf 'host_boundary: positive case\n'
reset_state
"$agent" doctor --state-dir "$state" --runtime-dir "$runtime" --json >"$state/doctor-empty.json"
assert_schema "$state/doctor-empty.json"

host --uid 60000 provision --name good --image "$image" >"$state/good.log" 2>&1 &
good_pid=$!
background_pids="$background_pids $good_pid"
wait_for '^READY sr-' "$state/good.log" 120
good_name=$(awk '/^CLAIM /{print $2}' "$state/good.log" | head -1)
grep -q '^CLAIM sr-' "$state/good.log" || fail "no ownership claim recorded"
grep -q '^INELIGIBLE ' "$state/good.log" && fail "eligible host was marked ineligible"

doctor >"$state/good-doctor.json"
assert_schema "$state/good-doctor.json"
grep -q '"eligible": true' "$state/good-doctor.json" || fail "doctor marked good host ineligible"
for name in vmm_pid per_job_uid cgroup_membership cpu_max memory_max pids_max landlock seccomp network_ipv4 network_ipv6 network_dns policy_environment kvm_access architecture cgroup_delegation clocks fd_limits runtime_compatibility; do
  assert_verdict "$state/good-doctor.json" "$name" pass
done
for name in reflink id_mapping; do
  actual=$(verdict "$state/good-doctor.json" "$name")
  case "$actual" in
    pass|missing) ;;
    *) fail "$name: expected pass or precise missing prerequisite, got ${actual:-missing}" ;;
  esac
done

host stop --name "$good_name"
wait_for '^REMOVED sr-' "$state/good.log" 120
wait "$good_pid"
background_pids=''
assert_clean

printf 'host_boundary: negative case, limit stripped\n'
reset_state
host --uid 60000 --cpu-max 'max 100000' --memory-max max --pids-max max \
  provision --name stripped --image "$image" >"$state/stripped.log" 2>&1 || true
grep -q '^INELIGIBLE ' "$state/stripped.log" || fail "stripped limits did not make host ineligible: $(cat "$state/stripped.log")"
grep -q 'kernel default' "$state/stripped.log" || fail "stripped limits have no specific reason"
grep -q '^READY ' "$state/stripped.log" && fail "ineligible host reached READY"
doctor >"$state/stripped-doctor.json" || true
assert_schema "$state/stripped-doctor.json"
grep -q '"eligible": false' "$state/stripped-doctor.json" || fail "doctor did not report ineligibility"
for name in cpu_max memory_max pids_max; do
  assert_verdict "$state/stripped-doctor.json" "$name" fail
done
assert_clean

printf 'host_boundary: negative case, policy variable injected\n'
reset_state
SMOLRUNNERS_TOKEN=leak host --uid 60000 provision --name leaked --image "$image" \
  >"$state/leaked.log" 2>&1 || true
grep -q '^INELIGIBLE ' "$state/leaked.log" || fail "injected policy variable did not make host ineligible: $(cat "$state/leaked.log")"
grep -q 'SMOLRUNNERS_TOKEN' "$state/leaked.log" || fail "injected policy variable is not named in the reason"
grep -q '^READY ' "$state/leaked.log" && fail "ineligible host reached READY"
doctor >"$state/leaked-doctor.json" || true
assert_schema "$state/leaked-doctor.json"
grep -q '"eligible": false' "$state/leaked-doctor.json" || fail "doctor did not report ineligibility"
assert_verdict "$state/leaked-doctor.json" policy_environment fail
assert_clean

printf 'host_boundary: negative case, host loopback reachable\n'
reset_state
host --uid 60000 --net-allow 127.0.0.1/32 provision --name loopback --image "$image" \
  >"$state/loopback.log" 2>&1 || true
grep -q '^INELIGIBLE ' "$state/loopback.log" || fail "reachable host loopback did not make host ineligible: $(cat "$state/loopback.log")"
grep -q 'host loopback' "$state/loopback.log" || fail "host loopback reason missing"
grep -q '^READY ' "$state/loopback.log" && fail "ineligible host reached READY"
doctor >"$state/loopback-doctor.json" || true
assert_schema "$state/loopback-doctor.json"
grep -q '"eligible": false' "$state/loopback-doctor.json" || fail "doctor did not report ineligibility"
for name in network_ipv4 network_ipv6 network_dns; do
  assert_verdict "$state/loopback-doctor.json" "$name" fail
done
assert_clean

printf 'host_boundary: ok\n'
