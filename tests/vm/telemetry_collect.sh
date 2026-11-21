#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/telemetry-collect-state
machine=sr-telemetry
spool_cap=20000

fail() { printf 'telemetry_collect: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --state-dir "$state/smolvm" "$@"
}

exec_in_guest() { smolvm machine exec --name "$machine" "$@"; }

stop_logger() {
  [ -n "${log_pid:-}" ] || return 0
  for child in $(ps -eo pid,ppid 2>/dev/null | awk -v parent="$log_pid" '$2 == parent {print $1}'); do
    kill -9 "$child" 2>/dev/null || true
  done
  kill -9 "$log_pid" 2>/dev/null || true
  log_pid=''
}

cleanup() {
  stop_logger
  smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
  smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
  for cgroup in /sys/fs/cgroup/smolrunners-workload.slice; do
    [ -d "$cgroup" ] && rmdir "$cgroup" 2>/dev/null || true
  done
}
trap cleanup EXIT

rm -rf "$state"
mkdir -p "$state/tmp" "$state/home/.config" "$state/home/.cache" \
  "$state/rootfs" "$state/cache" "$state/spool"

[ -x "$repo/tests/vm/bin/smolrunners-collector" ] || fail 'missing collector fixture binary'
[ -x "$runtime/smolvm" ] || fail 'missing pinned runtime'

printf 'telemetry_collect: building debian13 rootfs\n'
sh "$repo/images/debian13/build.sh" "$state/rootfs" "$state/cache"

mkdir -p "$state/rootfs/opt/smolrunners/bin"
cp "$repo/tests/vm/bin/smolrunners-collector" \
  "$state/rootfs/opt/smolrunners/bin/smolrunners-collector"
chmod 0755 "$state/rootfs/opt/smolrunners/bin/smolrunners-collector"

printf 'telemetry_collect: booting guest with the collector as the workload\n'
smolvm machine run -d --name "$machine" -I "$state/rootfs/" --net \
  -v "$repo:/work/smolrunners:ro" \
  -v "$runtime/agent-rootfs:/alpine-root:ro" \
  -v "$state/spool:/spool:rw" \
  -- /opt/smolrunners/bin/smolrunners-collector --interval-ms 60 --batch-lines 18 \
  > "$state/boot.log" 2>&1

count=0
while [ "$count" -lt 60 ]; do
  smolvm machine status --name "$machine" >/dev/null 2>&1 && break
  sleep 1
  count=$((count + 1))
done
[ "$count" -lt 60 ] || fail 'machine did not reach running state'

smolvm machine logs -f -n "$machine" > "$state/spool/raw.log" 2>&1 &
log_pid=$!

printf 'telemetry_collect: starting Docker with a deliberate cgroup parent\n'
exec_in_guest \
  -e HOME=/tmp -e DOCKER_CONFIG=/tmp/.docker \
  -e SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
  -- sh -c '
    set -eu
    mkdir -p /sys/fs/cgroup/smolrunners-workload.slice /work/docker /work/docker-exec /var/run /tmp/.docker
    dockerd --data-root=/work/docker --exec-root=/work/docker-exec \
      --host=unix:///var/run/docker.sock --pidfile=/work/docker.pid \
      --cgroup-parent=smolrunners-workload.slice --exec-opt native.cgroupdriver=cgroupfs \
      --storage-driver=vfs --feature containerd-snapshotter=false \
      --iptables=false --ip6tables=false > /work/dockerd.log 2>&1 &
    ready=0
    attempt=0
    while [ "$attempt" -lt 40 ]; do
      if docker info > /dev/null 2>&1; then ready=1; break; fi
      attempt=$((attempt + 1))
      sleep 1
    done
    [ "$ready" -eq 1 ] || { echo "docker daemon did not become ready" >&2; tail -20 /work/dockerd.log >&2; exit 1; }
    tar -C /alpine-root -cf - . | docker import \
      --change "CMD [\"/bin/sleep\",\"3600\"]" - nginx:alpine > /dev/null
  ' > "$state/docker.log" 2>&1 || fail "docker setup failed: $(tail -5 "$state/docker.log")"

cat > "$state/spool/scenario.sh" <<'EOF'
cpu 3
allocate 24 3
service 18080
io 4 6
EOF

cat > "$state/spool/phase-runner.sh" <<'EOF'
#!/bin/sh
set -u
export ACTIONS_RUNNER_HOOK_JOB_STARTED=${ACTIONS_RUNNER_HOOK_JOB_STARTED:-}
export ACTIONS_RUNNER_HOOK_JOB_COMPLETED=${ACTIONS_RUNNER_HOOK_JOB_COMPLETED:-}
/work/smolrunners/tests/fixtures/executor /spool/scenario.sh nginx:alpine 2>&1 |
  while IFS= read -r line; do
    printf '%s %s\n' "$(date +%s%3N)" "$line"
  done
EOF
chmod 0755 "$state/spool/phase-runner.sh"

printf 'telemetry_collect: running fixture phases with timestamps\n'
exec_in_guest \
  -e HOME=/tmp -e DOCKER_CONFIG=/tmp/.docker \
  -e SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
  -- sh /spool/phase-runner.sh > "$state/phases.log" 2>&1 \
  || fail "executor failed: $(tail -10 "$state/phases.log")"
grep -q 'executor: status=0' "$state/phases.log" \
  || fail "executor did not report success: $(tail -10 "$state/phases.log")"

printf 'telemetry_collect: finalizing collection\n'
sleep 3
smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
stop_logger
sleep 1

sh "$repo/tests/vm/telemetry_spool.sh" "$state/spool/series.ndjson" "$spool_cap" \
  < "$state/spool/raw.log"

kept=$(wc -l < "$state/spool/series.ndjson" | tr -d ' ')
[ "$kept" -le "$spool_cap" ] || fail "spool exceeded its cap: $kept"

printf 'telemetry_collect: checking bounded-spool overflow reason\n'
printf '%s\n' '{"a":1}' '{"a":2}' '{"a":3}' '{"a":4}' '{"a":5}' '{"a":6}' \
  | sh "$repo/tests/vm/telemetry_spool.sh" "$state/spool/overflow-test.ndjson" 5
grep -q 'reason=spool_line_cap' "$state/spool/overflow-test.ndjson.overflow" \
  || fail 'overflow reason was not recorded'
[ "$(wc -l < "$state/spool/overflow-test.ndjson" | tr -d ' ')" -eq 5 ] \
  || fail 'bounded spool did not keep exactly the cap'

printf 'telemetry_collect: asserting telemetry series\n'
python3 - "$state/spool/series.ndjson" "$state/phases.log" > "$state/assert.out" <<'PY'
import json
import statistics
import sys

spool_path, phases_path = sys.argv[1:3]
samples = []
with open(spool_path) as stream:
    for line in stream:
        line = line.strip()
        if line.startswith('{'):
            samples.append(json.loads(line))
assert samples, 'no telemetry samples were received'

scopes = {sample['scope'] for sample in samples}
assert {'guest', 'workload', 'host'} <= scopes, f'scopes: {scopes}'
assert len({sample['boot'] for sample in samples}) == 1, 'boot id changed within a run'
assert all(sample['v'] == 1 for sample in samples), 'unexpected schema version'
assert all(sample['gen'] == 1 for sample in samples), 'unexpected collector generation'

by_scope = {}
for sample in samples:
    by_scope.setdefault(sample['scope'], []).append(sample)
for scope, group in by_scope.items():
    sequences = sorted(sample['seq'] for sample in group)
    assert sequences[0] == 1, (scope, sequences[:5])
    assert sequences == list(range(sequences[0], sequences[-1] + 1)), (scope, 'sequence gap')
    assert sequences[-1] > 256, (scope, f'stopped at sample {sequences[-1]}')


def unit_of(sample, name):
    for metric in sample['metrics']:
        if metric['n'] == name:
            return metric['u']
    return None


def values(scope, name, start=None, end=None):
    result = []
    for sample in by_scope.get(scope, []):
        if start is not None and sample['wall_ms'] < start:
            continue
        if end is not None and sample['wall_ms'] > end:
            continue
        for metric in sample['metrics']:
            if metric['n'] == name:
                result.append(metric['v'])
    return result


markers = {}
with open(phases_path) as stream:
    for line in stream:
        parts = line.strip().split(None, 1)
        if len(parts) != 2:
            continue
        try:
            timestamp = int(parts[0])
        except ValueError:
            continue
        words = parts[1].split()
        if len(words) >= 3 and words[0] == 'phase':
            markers[(words[1], words[2])] = timestamp

for phase in ('cpu', 'allocate', 'service', 'io'):
    assert (phase, 'begin') in markers and (phase, 'end') in markers, (phase, markers)


def window(phase, pad=800):
    return markers[(phase, 'begin')] - pad, markers[(phase, 'end')] + pad


assert unit_of(samples[0], 'cpu.busy_cores') == 'cores'
assert unit_of(samples[0], 'disk.read_bytes_total') == 'bytes'

cpu_start, cpu_end = window('cpu')
busy = values('guest', 'cpu.busy_cores', cpu_start, cpu_end)
assert busy and max(busy) > 0.5, f'cpu phase busy_cores: {busy[:5]}...{busy[-5:] if busy else []}'
percent = values('guest', 'cpu.utilization_percent', cpu_start, cpu_end)
assert percent and unit_of(by_scope['guest'][0], 'cpu.utilization_percent') == 'percent'
assert max(percent) > 10.0, 'cpu phase utilization percent did not rise'

allocate_at = markers[('allocate', 'begin')]
baseline = statistics.median(values('guest', 'mem.available_bytes', allocate_at - 3000, allocate_at - 200))
available = values('guest', 'mem.available_bytes', *window('allocate'))
assert available and baseline - min(available) > 8 * 1024 * 1024, \
    f'allocate phase available_bytes: baseline={baseline} minimum={min(available) if available else None}'

io_at = markers[('io', 'begin')]
writes = values('guest', 'disk.write_bytes_delta', io_at, markers[('io', 'end')] + 800)
assert writes and max(writes) > 2 * 1024 * 1024, \
    f'io phase write delta: {writes[:5]}...{writes[-5:] if writes else []}'

service_at = markers[('service', 'begin')] + 1000
before_service = values('workload', 'cgroup.memory.current', 0, markers[('service', 'begin')] - 500)
after_service = values('workload', 'cgroup.memory.current', service_at, None)
assert after_service, 'no workload memory samples after the service started'
assert max(after_service) > max(before_service or [0]) + 1024 * 1024, \
    f'service memory not counted in workload scope: before={max(before_service or [0])} after={max(after_service)}'
descendants = values('workload', 'cgroup.descendants', service_at, None)
assert descendants and max(descendants) >= 1, 'service container cgroup was not seen in workload scope'
assert unit_of(by_scope['workload'][0], 'cgroup.memory.current') == 'bytes'

print(f'assertions: samples={len(samples)} scopes={sorted(scopes)} '
      f'last_sequence={max(s["seq"] for s in samples)} kept={len(samples)}')
PY
cat "$state/assert.out"

leftovers=$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}')
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"

printf 'telemetry_collect: ok\n'
