#!/bin/sh
set -eu

runtime=/opt/runtime
state=/work/tmp/host-lifecycle-state
binaries=/work/smolrunners/tests/vm/bin
agent=$binaries/smolrunners
launcher=$binaries/smolrunners-launcher
image='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'

fail() { printf 'host_lifecycle: %s\n' "$*" >&2; exit 1; }

smolvm() {
  vm=''
  previous=''
  for argument in "$@"; do
    [ "$previous" = --name ] && vm=$argument
    previous=$argument
  done
  [ -n "$vm" ] || { printf 'smolvm helper: --name is required
' >&2; return 2; }
  vmdir="$state/vms/$vm"
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$vmdir/home" TMPDIR="$vmdir/tmp" \
    XDG_CONFIG_HOME="$vmdir/home/.config" XDG_CACHE_HOME="$vmdir/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --error-format json --state-dir "$vmdir/smolvm" "$@"
}

leftovers() {
  ls -A "$state/vms" 2>/dev/null || true
}

host() {
  "$agent" host --state-dir "$state" --runtime-dir "$runtime" --launcher "$launcher" "$@"
}

base_args="--state-dir $state --runtime-dir $runtime --launcher $launcher"

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

wait_for_ready() {
  count=0
  while [ "$count" -lt 90 ]; do
    grep -q '^READY sr-' "$1" 2>/dev/null && return 0
    grep -q 'host:' "$1" 2>/dev/null && fail "agent failed: $(cat "$1")"
    sleep 1
    count=$((count + 1))
  done
  fail "timeout waiting for READY in $1"
}

wait_for_claim() {
  count=0
  while [ "$count" -lt 60 ]; do
    grep -q '^CLAIM sr-' "$1" 2>/dev/null && return 0
    sleep 1
    count=$((count + 1))
  done
  fail "timeout waiting for CLAIM in $1"
}

wait_for_removed() {
  count=0
  while [ "$count" -lt 90 ]; do
    grep -q '^REMOVED sr-' "$1" 2>/dev/null && return 0
    sleep 1
    count=$((count + 1))
  done
  fail "timeout waiting for REMOVED in $1"
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

wait_for_gone() {
  count=0
  while [ "$count" -lt 60 ]; do
    [ -d "$state/vms/$1" ] || return 0
    sleep 1
    count=$((count + 1))
  done
  fail "machine $1 still present"
}

assert_clean() {
  remaining="$(leftovers)"
  [ -z "$remaining" ] || fail "leftover machines: $remaining"
  markers="$(ls -A "$state/markers" 2>/dev/null)"
  [ -z "$markers" ] || fail "leftover markers: $markers"
  cgroups="$(ls /sys/fs/cgroup 2>/dev/null | awk '/^sr-/ {print}')"
  [ -z "$cgroups" ] || fail "leftover cgroups: $cgroups"
  admissions="$(ls "$state/storage" 2>/dev/null | awk '/admission/ {print}')"
  [ -z "$admissions" ] || fail "storage admissions outlived their VMs: $admissions"
  claims="$(ls -A "$state/uids" 2>/dev/null)"
  [ -z "$claims" ] || fail "uid claims outlived their VMs: $claims"
}

[ -x "$agent" ] || fail "missing agent binary at $agent"
[ -x "$launcher" ] || fail "missing launcher binary at $launcher"
[ -x "$runtime/smolvm" ] || fail "missing pinned runtime at $runtime/smolvm"

rm -rf "$state"
mkdir -p "$state"
for stale in /sys/fs/cgroup/sr-*; do
  [ -d "$stale" ] && rmdir "$stale" 2>/dev/null || true
done

printf 'host_lifecycle: checking environment sanitization\n'
environment="$(SMOLRUNNERS_TOKEN=leak GITHUB_TOKEN=leak ACTIONS_SECRET=leak SR_POLICY=leak \
  TOKEN=leak PASSWORD=leak host env)"
printf '%s\n' "$environment" | grep -Eq '^(SMOLRUNNERS_|SR_|GITHUB_|ACTIONS_|RUNNER_|TOKEN|PASSWORD|SECRET)' \
  && fail "policy variable leaked into smolvm environment"
printf '%s\n' "$environment" | grep -q "^SMOLVM_LIB_DIR=$runtime/lib" \
  || fail "SMOLVM_LIB_DIR missing from sanitized environment"
printf '%s\n' "$environment" | grep -q "^SMOLVM_AGENT_ROOTFS=$runtime/agent-rootfs" \
  || fail "SMOLVM_AGENT_ROOTFS missing from sanitized environment"

printf 'host_lifecycle: booting two concurrent guests\n'
"$agent" host $base_args --uid 60000 provision --name first --image "$image" >"$state/first.log" 2>&1 &
first_pid=$!
background_pids="$background_pids $first_pid"
"$agent" host $base_args --uid 60000 provision --name second --image "$image" >"$state/second.log" 2>&1 &
second_pid=$!
background_pids="$background_pids $second_pid"

wait_for_ready "$state/first.log"
wait_for_ready "$state/second.log"
first_name="$(awk '/^CLAIM /{print $2}' "$state/first.log" | head -1)"
second_name="$(awk '/^CLAIM /{print $2}' "$state/second.log" | head -1)"
[ "$first_name" != "$second_name" ] || fail "guests share a machine name"
wait_for_machine "$first_name"
wait_for_machine "$second_name"
[ "$(leftovers | wc -l | tr -d ' ')" -eq 2 ] \
  || fail "expected exactly two running guests"
for private in markers control logs; do
  [ "$(stat -c %a "$state/$private")" = 700 ] || fail "$private is not private to the host agent"
done
[ "$(stat -c %a "$state")" = 711 ] || fail "host state root is not 0711"
first_uid="$(stat -c %u "$state/vms/$first_name")"
second_uid="$(stat -c %u "$state/vms/$second_name")"
[ "$first_uid" != "$second_uid" ] || fail "two guests share a VM uid"
[ "$(stat -c %a "$state/vms/$first_name")" = 700 ] || fail "vm state for $first_name is not private"
setpriv --reuid "$first_uid" --regid "$first_uid" --clear-groups ls "$state/vms/$second_name" >/dev/null 2>&1 \
  && fail "the first VM uid can read the second VM's state"
setpriv --reuid "$first_uid" --regid "$first_uid" --clear-groups ls "$state/markers" >/dev/null 2>&1 \
  && fail "a VM uid can list host markers"
grep -q '^2147483648$' "/sys/fs/cgroup/$first_name/memory.max" \
  || fail "launcher did not apply memory.max to $first_name"
grep -q '^2147483648$' "/sys/fs/cgroup/$second_name/memory.max" \
  || fail "launcher did not apply memory.max to $second_name"

printf 'host_lifecycle: clean teardown\n'
host stop --name "$first_name"
host stop --name "$second_name"
wait_for_removed "$state/first.log"
wait_for_removed "$state/second.log"
wait "$first_pid"
wait "$second_pid"
background_pids=''
wait_for_gone "$first_name"
wait_for_gone "$second_name"
assert_clean

printf 'host_lifecycle: kill -9 mid-boot and reap on restart\n'
"$agent" host $base_args --uid 60000 --hold-after-create-ms 30000 provision --name crash \
  --image "$image" >"$state/crash.log" 2>&1 &
crash_pid=$!
background_pids="$background_pids $crash_pid"
wait_for_claim "$state/crash.log"
crash_name="$(awk '/^CLAIM /{print $2}' "$state/crash.log" | head -1)"
wait_for_machine "$crash_name"
kill -9 "$crash_pid"
wait "$crash_pid" 2>/dev/null || true
background_pids=''

reap_output="$(host reap)"
printf '%s\n' "$reap_output" | grep -q "^REAPED $crash_name$" \
  || fail "reaper did not remove the owned machine $crash_name: $reap_output"
wait_for_gone "$crash_name"
assert_clean

printf 'host_lifecycle: ok\n'
