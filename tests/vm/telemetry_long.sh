#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/telemetry-long-state
machine=sr-telemetry-long
seconds=${1:-120}
cap=$((seconds * 2 + 10000))

fail() { printf 'telemetry_long: %s\n' "$*" >&2; exit 1; }
note() { printf 'telemetry_long: %s\n' "$*"; }

case "$seconds" in ''|*[!0-9]*) fail "invalid duration: $seconds" ;; esac
[ "$seconds" -ge 10 ] || fail 'duration must be at least 10 seconds'

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --state-dir "$state/smolvm" "$@"
}

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
}
trap cleanup EXIT

rm -rf "$state"
mkdir -p "$state/tmp" "$state/home/.config" "$state/home/.cache" \
  "$state/rootfs" "$state/cache" "$state/spool"

[ -x "$repo/tests/vm/bin/smolrunners-collector" ] || fail 'missing collector fixture binary'
[ -x "$runtime/smolvm" ] || fail 'missing pinned runtime'

note "building debian13 rootfs for a ${seconds}s job"
sh "$repo/images/debian13/build.sh" "$state/rootfs" "$state/cache"

mkdir -p "$state/rootfs/opt/smolrunners/bin"
cp "$repo/tests/vm/bin/smolrunners-collector" \
  "$state/rootfs/opt/smolrunners/bin/smolrunners-collector"
chmod 0755 "$state/rootfs/opt/smolrunners/bin/smolrunners-collector"

note 'booting the guest with the collector as the workload'
smolvm machine run -d --name "$machine" -I "$state/rootfs/" --net \
  -v "$repo:/work/smolrunners:ro" \
  -v "$state/spool:/spool:rw" \
  -- /opt/smolrunners/bin/smolrunners-collector --interval-ms 60 --batch-lines 18 \
  > "$state/boot.log" 2>&1

booted=0
attempt=0
while [ "$attempt" -lt 60 ]; do
  if smolvm machine status --name "$machine" >/dev/null 2>&1; then booted=1; break; fi
  attempt=$((attempt + 1))
  sleep 1
done
[ "$booted" -eq 1 ] || fail 'machine did not reach running state'

smolvm machine logs -f -n "$machine" > "$state/spool/raw.log" 2>&1 &
log_pid=$!

half=$((seconds / 2))
[ "$half" -ge 5 ] || half=5
cat > "$state/spool/scenario.sh" <<EOF
cpu $half
allocate 32 $half
io 4 6
EOF

cat > "$state/spool/phase-runner.sh" <<'EOF'
#!/bin/sh
set -u
export ACTIONS_RUNNER_HOOK_JOB_STARTED=${ACTIONS_RUNNER_HOOK_JOB_STARTED:-}
export ACTIONS_RUNNER_HOOK_JOB_COMPLETED=${ACTIONS_RUNNER_HOOK_JOB_COMPLETED:-}
/work/smolrunners/tests/fixtures/executor /spool/scenario.sh 2>&1 |
  while IFS= read -r line; do
    printf '%s %s\n' "$(date +%s%3N)" "$line"
  done
EOF
chmod 0755 "$state/spool/phase-runner.sh"

started=$(date +%s)
note 'running the long fixture workload'
smolvm machine exec --name "$machine" -- sh /spool/phase-runner.sh \
  > "$state/phases.log" 2>&1 || fail "workload failed: $(tail -5 "$state/phases.log")"
grep -q 'executor: status=0' "$state/phases.log" \
  || fail "workload did not report success: $(tail -5 "$state/phases.log")"
elapsed=$(( $(date +%s) - started ))
[ "$elapsed" -ge $((seconds / 2)) ] || fail "workload ended too early: ${elapsed}s of ${seconds}s"

sleep 1
stop_logger
sh "$repo/tests/vm/telemetry_spool.sh" "$state/spool/series.ndjson" "$cap" \
  < "$state/spool/raw.log"

note 'checking coverage, gaps and metric completeness'
python3 - "$state/spool/series.ndjson" "$seconds" "$state/spool/series.ndjson.overflow" \
  > "$state/report.txt" <<'PY'
import datetime
import json
import os
import sys

spool_path, seconds_path, overflow_path = sys.argv[1:4]
required_seconds = int(seconds_path)

samples = []
with open(spool_path) as stream:
    for line in stream:
        line = line.strip()
        if not line.startswith('{'):
            continue
        sample = json.loads(line)
        assert sample.get('v') == 1, f"unexpected schema version: {sample.get('v')}"
        samples.append(sample)

assert samples, 'no telemetry samples were received'
by_capture = {}
for sample in samples:
    key = (sample.get('boot'), sample.get('gen'), sample.get('seq'))
    by_capture.setdefault(key, []).append(sample)

sequences = sorted({key[2] for key in by_capture})
assert len(sequences) >= 2, 'too few capture sequences to assess coverage'
span = sequences[-1] - sequences[0] + 1
coverage = len(sequences) / span
gaps = []
for previous, following in zip(sequences, sequences[1:]):
    if following == previous + 1:
        continue
    before = [s for s in samples if s.get('seq') == previous]
    after = [s for s in samples if s.get('seq') == following]
    gaps.append({
        'from_seq': previous,
        'to_seq': following,
        'missing': following - previous - 1,
        'from_ms': min(s['wall_ms'] for s in before),
        'to_ms': max(s['wall_ms'] for s in after),
    })

missing_metric = []
for key, rows in by_capture.items():
    names = {metric['n'] for sample in rows for metric in sample.get('metrics', [])}
    for name in ('cpu.busy_cores', 'mem.used_bytes'):
        if name not in names:
            missing_metric.append({'seq': key[2], 'metric': name})

first_ms = min(s['wall_ms'] for s in samples)
last_ms = max(s['wall_ms'] for s in samples)
wall_seconds = (last_ms - first_ms) / 1000.0

report = {
    'samples': len(samples),
    'captures': len(sequences),
    'span_sequences': span,
    'wall_seconds': round(wall_seconds, 3),
    'coverage': round(coverage, 6),
    'gaps': gaps,
    'missing_metrics': missing_metric,
    'overflow': os.path.exists(overflow_path),
}
print(json.dumps(report, indent=2))

assert not report['overflow'], 'spool hit its line cap; coverage cannot be assessed'
assert wall_seconds >= required_seconds * 0.5, (
    f"observation window {wall_seconds:.1f}s is too short for a {required_seconds}s job"
)
assert not missing_metric, f"captures missing required metrics: {missing_metric[:5]}"
assert coverage >= 0.95, f"coverage {coverage:.4f} below 0.95; gaps: {gaps[:10]}"
print('telemetry_long: coverage full or explicitly accounted; no sample-count cutoff')
PY

cat "$state/report.txt"
note 'ok'
