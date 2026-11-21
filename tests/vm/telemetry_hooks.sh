#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/telemetry-hooks-state
collector="$repo/tests/vm/bin/smolrunners-collector"
executor="$repo/tests/fixtures/executor"
machine=sr-hooks-vm
ALPINE='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'

fail() { printf 'telemetry_hooks: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --state-dir "$state/smolvm" "$@"
}

stop_collector() {
  [ -n "${collector_pid:-}" ] || return 0
  kill -9 "$collector_pid" 2>/dev/null || true
  wait "$collector_pid" 2>/dev/null || true
  collector_pid=''
}

cleanup() {
  stop_collector
  smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
  smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
}
trap cleanup EXIT

rm -rf "$state"
mkdir -p "$state/tmp" "$state/home/.config" "$state/home/.cache" "$state/hooks"

[ -x "$collector" ] || fail 'missing collector fixture binary'
[ -x "$executor" ] || fail 'missing fixture executor'
[ -x "$runtime/smolvm" ] || fail 'missing pinned runtime'
command -v timeout >/dev/null 2>&1 || fail 'timeout is required for bounded hooks'

printf 'telemetry_hooks: booting the sentinel guest the hooks must not destroy\n'
smolvm machine run -d --name "$machine" -I "$ALPINE" -- /bin/sleep 300 \
  > "$state/boot.log" 2>&1 || fail "guest boot failed: $(tail -5 "$state/boot.log")"
count=0
while [ "$count" -lt 30 ]; do
  smolvm machine status --name "$machine" >/dev/null 2>&1 && break
  sleep 1
  count=$((count + 1))
done
[ "$count" -lt 30 ] || fail 'guest did not reach running state'

HOOK_LOG="$state/hooks.log"
export HOOK_LOG

cat > "$state/hooks/started.sh" <<'EOF'
#!/bin/sh
printf 'started begin %s\n' "$(date +%s%3N)" >> "$HOOK_LOG"
sleep 5
printf 'started end %s\n' "$(date +%s%3N)" >> "$HOOK_LOG"
EOF

cat > "$state/hooks/completed.sh" <<'EOF'
#!/bin/sh
printf 'completed begin %s %s\n' "$(date +%s%3N)" "${1:-}" >> "$HOOK_LOG"
printf 'completed end %s\n' "$(date +%s%3N)" >> "$HOOK_LOG"
EOF
chmod 0755 "$state/hooks/started.sh" "$state/hooks/completed.sh"

if grep -Eq 'smolvm|machine[[:space:]]+(delete|stop)|destroy' "$state/hooks/started.sh" \
  "$state/hooks/completed.sh"; then
  fail 'a hook contains a VM teardown command'
fi

cat > "$state/workflow_jobs.json" <<'EOF'
{
  "total_count": 2,
  "jobs": [
    {
      "id": 5551001,
      "run_id": 99001,
      "name": "build",
      "status": "completed",
      "conclusion": "success",
      "started_at": "2026-11-21T12:00:00Z",
      "completed_at": "2026-11-21T12:00:30Z",
      "steps": [
        {"number": 1, "name": "Set up job", "status": "completed",
         "started_at": "2026-11-21T12:00:00Z", "completed_at": "2026-11-21T12:00:03Z"},
        {"number": 2, "name": "Checkout", "status": "completed",
         "started_at": "2026-11-21T12:00:03.500Z", "completed_at": "2026-11-21T12:00:08Z"},
        {"number": 3, "name": "Build", "status": "completed",
         "started_at": "2026-11-21T12:00:08Z", "completed_at": "2026-11-21T12:00:27.250Z"},
        {"number": 4, "name": "Pending step", "status": "skipped",
         "started_at": null, "completed_at": null}
      ]
    },
    {
      "id": 5551002,
      "run_id": 99001,
      "name": "test",
      "status": "in_progress",
      "conclusion": null,
      "started_at": "2026-11-21T12:00:31Z",
      "completed_at": null,
      "steps": [
        {"number": 1, "name": "Set up job", "status": "completed",
         "started_at": "2026-11-21T12:00:31Z", "completed_at": "2026-11-21T12:00:33Z"}
      ]
    }
  ]
}
EOF

printf 'telemetry_hooks: collecting samples while the executor fires both hooks\n'
"$collector" --interval-ms 40 --batch-lines 18 \
  > "$state/spool.ndjson" 2> "$state/collector.err" &
collector_pid=$!
sleep 0.4

cat > "$state/scenario.sh" <<'EOF'
cpu 2
allocate 16 2
EOF

run_start=$(date +%s%3N)
ACTIONS_RUNNER_HOOK_JOB_STARTED="$state/hooks/started.sh" \
ACTIONS_RUNNER_HOOK_JOB_COMPLETED="$state/hooks/completed.sh" \
  sh -c 'exec "$1" "$2"' sh "$executor" "$state/scenario.sh" \
  > "$state/executor.log" 2>&1 || fail "executor failed: $(tail -10 "$state/executor.log")"
run_end=$(date +%s%3N)
printf '%s %s\n' "$run_start" "$run_end" > "$state/run-times"
grep -q 'executor: status=0' "$state/executor.log" \
  || fail "executor did not report success: $(tail -10 "$state/executor.log")"
sleep 0.3
stop_collector

printf 'telemetry_hooks: checking the sentinel guest survived both hooks\n'
smolvm machine status --name "$machine" >/dev/null 2>&1 \
  || fail 'a hook destroyed the VM'

sh "$repo/tests/vm/telemetry_spool.sh" "$state/series.ndjson" 20000 \
  < "$state/spool.ndjson"

printf 'telemetry_hooks: asserting hook, relabel and step-overlay behavior\n'
python3 - "$state/series.ndjson" "$state/hooks.log" "$state/workflow_jobs.json" \
  "$state/run-times" > "$state/assert.out" <<'PY'
import datetime
import json
import sys

spool_path, hooks_path, jobs_path, times_path = sys.argv[1:5]
run_start, run_end = (int(value) for value in open(times_path).read().split())

lines = [line.strip() for line in open(hooks_path) if line.strip()]
started_begin = [line for line in lines if line.startswith('started begin ')]
started_end = [line for line in lines if line.startswith('started end ')]
completed_begin = [line for line in lines if line.startswith('completed begin ')]
completed_end = [line for line in lines if line.startswith('completed end ')]
assert len(started_begin) == 1, lines
assert not started_end, 'the slow started hook was not cut off at the 2 s bound'
assert len(completed_begin) == 1 and len(completed_end) == 1, lines

started_ms = int(started_begin[0].split()[2])
completed_fields = completed_begin[0].split()
assert len(completed_fields) == 4 and completed_fields[3] == '0', completed_begin[0]
completed_begin_ms = int(completed_fields[2])
completed_end_ms = int(completed_end[0].split()[2])
assert 0 <= completed_end_ms - completed_begin_ms <= 2000
assert run_end - run_start <= 20000
assert started_ms <= completed_begin_ms <= completed_end_ms <= run_end

samples = []
for line in open(spool_path):
    line = line.strip()
    if line.startswith('{'):
        samples.append(json.loads(line))
assert samples, 'no telemetry samples were received'

interval = (started_ms, completed_end_ms)
metadata = {'run_id': 99001, 'job_id': 5551001, 'run_attempt': 2}
pending = []
for sample in samples:
    value = sample['metrics'][0]['v'] if sample['metrics'] else None
    pending.append({
        'seq': sample['seq'],
        'mono': sample['mono_ns'],
        'wall': sample['wall_ms'],
        'value': value,
        'identity': None,
    })
snapshot = [(row['seq'], row['mono'], row['wall'], row['value']) for row in pending]


def in_interval(row):
    return interval[0] <= row['wall'] <= interval[1]


relabeled = 0
for row in pending:
    if row['identity'] is None and in_interval(row):
        row['identity'] = dict(metadata)
        relabeled += 1
assert relabeled > 0, 'no identity-pending sample was relabeled'
assert all(row['identity'] == metadata for row in pending if in_interval(row))
assert all(row['identity'] is None for row in pending if not in_interval(row))
assert [(row['seq'], row['mono'], row['wall'], row['value']) for row in pending] == snapshot, \
    'relabeling moved a sample'
for row in pending:
    if row['identity'] is None and in_interval(row):
        row['identity'] = dict(metadata)
assert [(row['seq'], row['mono'], row['wall'], row['value']) for row in pending] == snapshot


def parse_ts(value):
    moment = datetime.datetime.fromisoformat(value.replace('Z', '+00:00'))
    return int(moment.timestamp() * 1000)


recorded = json.load(open(jobs_path))
overlays = []
for job in recorded['jobs']:
    if job.get('completed_at') is None:
        continue
    for step in job['steps']:
        if step.get('started_at') and step.get('completed_at'):
            overlays.append({
                'job_id': job['id'],
                'number': step['number'],
                'name': step['name'],
                'start': parse_ts(step['started_at']),
                'end': parse_ts(step['completed_at']),
            })
assert len(overlays) == 3, overlays
assert all(overlay['job_id'] == 5551001 for overlay in overlays)
assert [overlay['number'] for overlay in overlays] == [1, 2, 3]
assert [overlay['name'] for overlay in overlays] == ['Set up job', 'Checkout', 'Build']
assert all(overlay['end'] > overlay['start'] for overlay in overlays)
assert overlays[1]['start'] % 1000 == 500
assert overlays[2]['end'] % 1000 == 250
assert not any(overlay['name'] == 'Pending step' for overlay in overlays)

job = next(item for item in recorded['jobs'] if item['id'] == 5551001)
window = (parse_ts(job['started_at']), parse_ts(job['completed_at']))
assert all(window[0] <= overlay['start'] and overlay['end'] <= window[1] for overlay in overlays)
narrow = (parse_ts('2026-11-21T12:00:04Z'), parse_ts('2026-11-21T12:00:30Z'))
assert sum(1 for overlay in overlays
           if narrow[0] <= overlay['start'] and overlay['end'] <= narrow[1]) == 1

print(f'telemetry_hooks: hooks started={started_ms} completed={completed_begin_ms}->'
      f'{completed_end_ms} elapsed={run_end - run_start}ms slow_hook_cut_off=yes')
print(f'telemetry_hooks: relabeled={relabeled}/{len(pending)} samples interval={interval} '
      f'metadata={metadata} samples_unchanged=yes')
print(f'telemetry_hooks: step_overlays={len(overlays)} from the completed job only '
      f'window={window}')
PY
cat "$state/assert.out"

smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true

printf 'telemetry_hooks: checking for leftover sr-* resources\n'
leftovers=$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}' || true)
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"

printf 'telemetry_hooks: ok\n'
