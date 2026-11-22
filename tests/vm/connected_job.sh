#!/bin/sh
set -eu

if [ "$(uname -s)" != "Darwin" ]; then
  printf 'connected_job: the production coordinator is built from current source on the macOS host; the nested guest has no Rust or Go toolchain.\n' >&2
  printf 'connected_job: run this driver from the macOS repository checkout, not through make test-vm.\n' >&2
  exit 2
fi

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
runtime="$repo/.work/runtime"
host_home="$repo/.work/linux-host/home"
state="$repo/.work/tmp/connected-job-state"
gstate=/work/tmp/connected-job-state
machine_name=sr-dev
port=8793
image='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'
bin="$repo/target/debug/smolrunners"
adapter="$repo/tests/cli/bin/smolrunners-adapter"

fail() { printf 'connected_job: %s\n' "$*" >&2; exit 1; }

native_smolvm() {
  env -i PATH="$PATH" HOME="$host_home" TMPDIR="$host_home/tmp" \
    XDG_CONFIG_HOME="$host_home/.config" XDG_CACHE_HOME="$host_home/.cache" \
    "$runtime/darwin-arm64/smolvm" "$@"
}

guest() { sh "$repo/scripts/linux-host.sh" exec "$*"; }

guest_smolvm() {
  guest "env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin HOME=$gstate/home TMPDIR=$gstate/tmp XDG_CONFIG_HOME=$gstate/home/.config XDG_CACHE_HOME=$gstate/home/.cache SMOLVM_LIB_DIR=/opt/runtime/lib SMOLVM_AGENT_ROOTFS=/opt/runtime/agent-rootfs /opt/runtime/smolvm --error-format json --state-dir $gstate/host/smolvm $*"
}

guest_machines() {
  guest_smolvm 'machine ls --json 2>/dev/null' 2>/dev/null | sed -n 's/.*"name": *"\(sr-[^"]*\)".*/\1/p'
}

guest_clean_cgroups() {
  guest 'for c in /sys/fs/cgroup/sr-*; do [ -d "$c" ] || continue; for p in $(cat "$c/cgroup.procs" 2>/dev/null); do kill -9 "$p" 2>/dev/null || true; done; done; sleep 1; for c in /sys/fs/cgroup/sr-*; do [ -d "$c" ] && rmdir "$c" 2>/dev/null || true; done' >/dev/null 2>&1 || true
}

coord_pid=''
cleanup() {
  [ -n "$coord_pid" ] && kill -9 "$coord_pid" 2>/dev/null || true
  guest "pkill -9 -f smolrunners-adapter >/dev/null 2>&1 || true" >/dev/null 2>&1 || true
  for leftover in $(guest_machines); do
    guest_smolvm "machine stop --name $leftover >/dev/null 2>&1 || true" >/dev/null 2>&1 || true
    guest_smolvm "machine delete --name $leftover -f >/dev/null 2>&1 || true" >/dev/null 2>&1 || true
  done
  guest_clean_cgroups
}
trap cleanup EXIT

wait_for() {
  pattern=$1
  attempts=$2
  count=0
  while [ "$count" -lt "$attempts" ]; do
    if grep -q "$pattern" "$state/coord.log" 2>/dev/null; then
      return 0
    fi
    count=$((count + 1))
    sleep 1
  done
  printf 'connected_job: timeout waiting for %s\n%s\n' "$pattern" "$(tail -40 "$state/coord.log" 2>/dev/null)" >&2
  return 1
}

samples() {
  python3 - "$state/coord/coordinator.sqlite" <<'PY'
import sqlite3, sys
try:
    connection = sqlite3.connect(sys.argv[1])
    print(connection.execute("select count(*) from sample").fetchone()[0])
except Exception:
    print(0)
PY
}

start_coordinator() {
  "$bin" serve \
    --listen "127.0.0.1:$port" \
    --state-dir "$state/coord" \
    --scenario connected-a,connected-b \
    --scenarios-dir "$state/scenarios" \
    --adapter "$adapter" \
    --image "$image" \
    --runtime-dir /opt/runtime \
    --launcher /work/smolrunners/tests/vm/bin/smolrunners-launcher \
    --agent /work/smolrunners/tests/vm/bin/smolrunners \
    --host-vcpu 2 --host-memory-gib 4 \
    --profile fixture-1c2g --vcpu 1 --memory-gib 2 \
    --guest-exec "$repo/scripts/linux-host.sh" \
    --guest-smolvm "$runtime/darwin-arm64/smolvm" \
    --guest-machine "$machine_name" \
    --guest-home "$host_home" \
    --guest-state-dir "$gstate" \
    --collector /work/smolrunners/tests/vm/bin/smolrunners-collector \
    --executor /work/smolrunners/tests/fixtures/executor \
    --fixture-script "$gstate/fixture.sh" \
    >>"$state/coord.log" 2>&1 &
  coord_pid=$!
}

[ -x "$runtime/darwin-arm64/smolvm" ] || fail 'missing macOS runtime'
[ -x "$adapter" ] || fail "missing committed guest adapter at $adapter"

printf 'connected_job: building the production coordinator from current source\n'
export TMPDIR="$repo/.work/tmp"
( cd "$repo" && cargo build -p smolrunners --bin smolrunners ) || fail 'cargo build failed'
[ -x "$bin" ] || fail "missing freshly built binary at $bin"

printf 'connected_job: bringing up the nested host and syncing the tree\n'
sh "$repo/scripts/linux-host.sh" up || fail 'linux-host up failed'
sh "$repo/scripts/linux-host.sh" sync || fail 'linux-host sync failed'

printf 'connected_job: staging only the fixture, demand and failure inputs\n'
rm -rf "$state"
mkdir -p "$state/scenarios"
cat > "$state/fixture.sh" <<'EOF'
cpu 4
allocate 16 10
io 4 10
EOF
for name in a b; do
  number=101
  message=100
  job=job-connected-a
  if [ "$name" = b ]; then number=201; message=200; job=job-connected-b; fi
  cat > "$state/scenarios/connected-$name.json" <<EOF
{
  "name": "connected-$name",
  "session": {
    "session_id": "11111111-2222-3333-4444-55555555555$number",
    "owner_name": "acme",
    "statistics": { "total_assigned_jobs": 1, "total_available_jobs": 1 }
  },
  "acquire_results": [[$number]],
  "steps": [
    {
      "kind": "message",
      "message_id": $message,
      "statistics": { "total_assigned_jobs": 1, "total_available_jobs": 1 },
      "job_available": [
        { "request_id": $number, "job_id": "$job", "labels": ["smol-debian13-arm64-1c2g"], "workflow_run_id": 9901, "owner_name": "acme", "repository_name": "repo" }
      ],
      "job_assigned": [
        { "request_id": $number, "job_id": "$job", "labels": ["smol-debian13-arm64-1c2g"], "workflow_run_id": 9901, "owner_name": "acme", "repository_name": "repo" }
      ],
      "job_completed": [
        { "request_id": $number, "job_id": "$job", "labels": ["smol-debian13-arm64-1c2g"], "workflow_run_id": 9901, "owner_name": "acme", "repository_name": "repo" }
      ]
    },
    { "kind": "nil" }
  ],
  "after_end": "nil"
}
EOF
done
guest "rm -rf $gstate && mkdir -p $gstate/scenarios $gstate/home $gstate/tmp $gstate/host/smolvm && chown -R 60000:60000 $gstate/host" || fail 'guest staging dirs failed'
guest_clean_cgroups
native_smolvm machine cp "$state/scenarios/connected-a.json" "$machine_name:$gstate/scenarios/connected-a.json" || fail 'copy scenario a failed'
native_smolvm machine cp "$state/scenarios/connected-b.json" "$machine_name:$gstate/scenarios/connected-b.json" || fail 'copy scenario b failed'
native_smolvm machine cp "$state/fixture.sh" "$machine_name:$gstate/fixture.sh" || fail 'copy fixture failed'

printf 'connected_job: starting the production coordinator and host\n'
start_coordinator
wait_for 'RESERVE job=job-connected-a ' 120 || fail 'no durable reservation for job a'
wait_for 'ACQUIRE job=job-connected-a ' 120 || fail 'no persisted acquisition for job a'
wait_for 'LAUNCH job=job-connected-a machine=sr-' 150 || fail 'coordinator did not launch the owned VM'
guest "python3 - <<'PY'
import glob, json, sys
markers = [json.load(open(path)) for path in glob.glob('$gstate/host/markers/*.json')]
assert markers, 'no host ownership markers'
marker = markers[0]
assert marker.get('reservation_id'), marker
assert marker.get('execution_id'), marker
assert marker.get('storage_gib'), marker
print('connected_job: marker carries reservation=%s execution=%s storage=%sGiB' % (
    marker['reservation_id'], marker['execution_id'], marker['storage_gib']))
PY" || fail 'the host marker does not carry the execution identity'
wait_for 'TELEMETRY attempt=' 60 || fail 'no live telemetry during execution'
first=$(samples)
sleep 4
second=$(samples)
[ "$second" -gt "$first" ] || fail "telemetry did not change during execution ($first -> $second)"
printf 'connected_job: live telemetry changed during execution (%s -> %s samples)\n' "$first" "$second"

printf 'connected_job: killing the coordinator mid-execution\n'
pre_restart=$(samples)
kill -9 "$coord_pid"
wait "$coord_pid" 2>/dev/null || true
coord_pid=''
machine=$(sed -n 's/.*LAUNCH job=job-connected-a machine=\([^ ]*\).*/\1/p' "$state/coord.log" | head -1)
[ -n "$machine" ] || fail 'no owned machine recorded'
guest_smolvm "machine status --name $machine >/dev/null 2>&1" || fail "the owned VM $machine died with the coordinator"

printf 'connected_job: restarting the coordinator from durable state\n'
start_coordinator
wait_for 'REATTACH job=job-connected-a ' 120 || fail 'coordinator did not reattach to the running execution'
fourth=$pre_restart
attempt=0
while [ "$attempt" -lt 60 ] && [ "$fourth" -le "$pre_restart" ]; do
  sleep 0.25
  fourth=$(samples)
  attempt=$((attempt + 1))
done
[ "$fourth" -gt "$pre_restart" ] || fail "telemetry did not resume after restart ($pre_restart -> $fourth)"
printf 'connected_job: telemetry resumed after restart (%s -> %s samples)\n' "$pre_restart" "$fourth"
wait_for 'TEARDOWN machine=' 180 || fail 'coordinator did not tear down the same execution'
wait_for 'RELEASE job=job-connected-a ' 60 || fail 'coordinator did not release capacity'
wait_for 'JOB_DONE job=job-connected-a ' 60 || fail 'execution a did not finish'
guest_smolvm "machine status --name $machine >/dev/null 2>&1" && fail "machine $machine survived teardown"

printf 'connected_job: running a clean second job\n'
wait_for 'RESERVE job=job-connected-b ' 180 || fail 'no reservation for job b'
wait_for 'LAUNCH job=job-connected-b machine=sr-' 180 || fail 'coordinator did not launch job b'
wait_for 'JOB_DONE job=job-connected-b ' 180 || fail 'execution b did not finish'
wait_for 'RELEASE job=job-connected-b ' 60 || fail 'capacity from job b was not released'
wait_for 'COMPLETE' 60 || fail 'coordinator did not reach COMPLETE'

python3 - "$state/coord/coordinator.sqlite" <<'PY'
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
acquisitions = connection.execute("select count(*) from lifecycle_acquisition").fetchone()[0]
samples = connection.execute("select count(*) from sample").fetchone()[0]
assert acquisitions >= 1, "no durable acquisition was persisted"
assert samples > 0, "no production telemetry was ingested"
print("connected_job: durable acquisitions=%d samples=%d" % (acquisitions, samples))
PY

leftovers="$(guest_machines)"
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"
cgroups="$(guest "ls /sys/fs/cgroup 2>/dev/null | sed -n '/^sr-/p'" 2>/dev/null)"
[ -z "$cgroups" ] || fail "leftover cgroups: $cgroups"

printf 'connected_job: ok\n'
