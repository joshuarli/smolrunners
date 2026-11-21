#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/telemetry-history-state
collector="$repo/tests/vm/bin/smolrunners-collector"
executor="$repo/tests/fixtures/executor"

fail() { printf 'telemetry_history: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME=/root TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/config" XDG_CACHE_HOME="$state/cache" \
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
}
trap cleanup EXIT

rm -rf "$state"
mkdir -p "$state/tmp" "$state/config" "$state/cache"

[ -x "$collector" ] || fail 'missing collector fixture binary'
[ -x "$executor" ] || fail 'missing fixture executor'
nproc > "$state/vcpus"

printf 'telemetry_history: collecting live samples with the fixture phases\n'
"$collector" --interval-ms 40 --batch-lines 18 \
  > "$state/spool.ndjson" 2> "$state/collector.err" &
collector_pid=$!
sleep 0.4

cat > "$state/scenario.sh" <<'EOF'
cpu 3
allocate 24 2
io 4 4
EOF

cat > "$state/phase-runner.sh" <<EOF
#!/bin/sh
set -u
export ACTIONS_RUNNER_HOOK_JOB_STARTED=\${ACTIONS_RUNNER_HOOK_JOB_STARTED:-}
export ACTIONS_RUNNER_HOOK_JOB_COMPLETED=\${ACTIONS_RUNNER_HOOK_JOB_COMPLETED:-}
"$executor" "$state/scenario.sh" 2>&1 |
  while IFS= read -r line; do
    printf '%s %s\\n' "\$(date +%s%3N)" "\$line"
  done
EOF
chmod 0755 "$state/phase-runner.sh"

"$state/phase-runner.sh" > "$state/phases.log" 2>&1 \
  || fail "executor failed: $(tail -5 "$state/phases.log")"
grep -q 'executor: status=0' "$state/phases.log" \
  || fail "executor did not report success: $(tail -5 "$state/phases.log")"
sleep 0.3
stop_collector

printf 'telemetry_history: replaying ingestion with restart and link outage\n'
python3 - "$state/spool.ndjson" "$state/phases.log" "$state/vcpus" "$state/history.sqlite" \
  > "$state/assert.out" <<'PY'
import collections
import datetime
import hashlib
import json
import sqlite3
import sys

spool_path, phases_path, vcpus_path, db_path = sys.argv[1:5]
vcpus = int(open(vcpus_path).read().strip())

samples = []
with open(spool_path) as stream:
    for line in stream:
        line = line.strip()
        if line.startswith('{'):
            samples.append(json.loads(line))
assert samples, 'no telemetry samples were received'
assert all(sample['v'] == 1 for sample in samples), 'unexpected sample schema version'
scopes = {sample['scope'] for sample in samples}
assert {'guest', 'workload', 'host'} <= scopes, f'scopes: {sorted(scopes)}'
boot = samples[0]['boot']
generation = samples[0]['gen']
assert len({sample['boot'] for sample in samples}) == 1, 'boot id changed within the job'
assert len({sample['gen'] for sample in samples}) == 1, 'collector generation changed within the job'
sequences = collections.defaultdict(list)
for sample in samples:
    sequences[sample['scope']].append(sample['seq'])
for scope, values in sequences.items():
    values.sort()
    assert values[0] == 1, (scope, values[:3])
    assert values == list(range(1, values[-1] + 1)), (scope, 'sequence gap before the outage')
    assert values[-1] > 40, (scope, 'collector stopped early')

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
for phase in ('cpu', 'allocate', 'io'):
    assert (phase, 'begin') in markers and (phase, 'end') in markers, markers
phases = sorted({name for name, edge in markers if edge == 'begin'})


def unit_of(scope, name):
    for sample in samples:
        if sample['scope'] != scope:
            continue
        for metric in sample['metrics']:
            if metric['n'] == name:
                return metric['u']
    return None


def values(scope, name, start=None, end=None):
    out = []
    for sample in samples:
        if sample['scope'] != scope:
            continue
        if start is not None and sample['wall_ms'] < start:
            continue
        if end is not None and sample['wall_ms'] > end:
            continue
        for metric in sample['metrics']:
            if metric['n'] == name:
                out.append((sample, metric['v']))
    return out


cores_unit = unit_of('guest', 'cpu.busy_cores')
percent_unit = unit_of('guest', 'cpu.utilization_percent')
assert cores_unit == 'cores', cores_unit
assert percent_unit == 'percent', percent_unit

cpu_window = (markers[('cpu', 'begin')] - 500, markers[('cpu', 'end')] + 500)
busy = values('guest', 'cpu.busy_cores', *cpu_window)
percent = values('guest', 'cpu.utilization_percent', *cpu_window)
assert busy and max(value for _, value in busy) > 0.5, 'cpu phase showed no cores used'
assert percent and max(value for _, value in percent) > 10.0, 'cpu phase showed no percent'
paired = dict((id(sample), value) for sample, value in percent)
ratios = [
    paired[id(sample)] / value
    for sample, value in busy
    if value >= 0.5 and paired[id(sample)] > 0
]
assert ratios, 'no cpu phase sample paired cores with a percent'
mean_ratio = sum(ratios) / len(ratios)
allocated_vcpus = round(100.0 / mean_ratio)
assert allocated_vcpus >= 1 and abs(100.0 / allocated_vcpus - mean_ratio) < 1.0, \
    (allocated_vcpus, mean_ratio)
checked = 0
for sample, value in busy:
    if value < 0.5:
        continue
    expected = value / allocated_vcpus * 100.0
    actual = paired[id(sample)]
    assert abs(actual - expected) < 8.0, (actual, expected, allocated_vcpus)
    checked += 1
assert checked > 0, 'no cpu phase sample tied cores to allocated vCPUs'
busy_peak = max(value for _, value in busy)

observed_definition = set()
for sample in samples:
    for metric in sample['metrics']:
        observed_definition.add(f"{sample['scope']}:{metric['n']}:{metric['u']}")


def cohort_id(definition):
    canonical = 'v1\n' + '\n'.join(sorted(definition))
    return int.from_bytes(hashlib.sha256(canonical.encode()).digest()[:8], 'big') & (2**63 - 1)


cohort = cohort_id(observed_definition)
changed_definition = observed_definition | {'guest:cpu.hypothetical_new_metric:cores'}
assert cohort_id(changed_definition) != cohort, 'definition change did not start a new cohort'

outage_start = markers[('io', 'begin')] + 800
outage_end = outage_start + 1600
before_outage = len(samples)
samples = [sample for sample in samples
           if not (outage_start <= sample['wall_ms'] < outage_end)]
dropped = before_outage - len(samples)
assert dropped > 0, 'the link outage dropped no samples'

seen_gaps = []
for scope in scopes:
    ordered = sorted(sample['seq'] for sample in samples if sample['scope'] == scope)
    for earlier, later in zip(ordered, ordered[1:]):
        if later != earlier + 1:
            seen_gaps.append((scope, earlier, later))
assert seen_gaps, 'the link outage did not create a sequence gap'

last_by_scope = {}
annotated = []
for sample in samples:
    key = (sample['scope'], cohort)
    previous = last_by_scope.get(key)
    gap = False
    if previous is not None:
        if (previous['boot'], previous['gen']) != (sample['boot'], sample['gen']):
            gap = True
        elif sample['seq'] != previous['seq'] + 1:
            gap = True
        elif sample['mono_ns'] <= previous['mono_ns']:
            gap = True
    annotated.append((sample, gap))
    last_by_scope[key] = sample


def rfc3339_ms(milliseconds):
    base = datetime.datetime.fromtimestamp(milliseconds // 1000, datetime.timezone.utc)
    return base.strftime('%Y-%m-%dT%H:%M:%S.') + f'{milliseconds % 1000:03d}Z'


schema = """
CREATE TABLE sample (
    id INTEGER PRIMARY KEY,
    execution_attempt_id INTEGER,
    scope TEXT NOT NULL,
    metric TEXT NOT NULL,
    value REAL NOT NULL,
    unit TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    captured_at TEXT NOT NULL,
    cohort INTEGER NOT NULL,
    mono_ns INTEGER NOT NULL,
    wall_ms INTEGER NOT NULL,
    boot TEXT NOT NULL,
    gen INTEGER NOT NULL,
    gap INTEGER NOT NULL DEFAULT 0
);
"""

execution_id = 7
batch_size = 64


def ingest(connection, batch):
    cursor = connection.cursor()
    for sample, gap in batch:
        captured_at = rfc3339_ms(sample['wall_ms'])
        for metric in sample['metrics']:
            cursor.execute(
                'INSERT INTO sample (execution_attempt_id, scope, metric, value, unit, '
                'sequence, captured_at, cohort, mono_ns, wall_ms, boot, gen, gap) '
                'VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)',
                (execution_id, sample['scope'], metric['n'], float(metric['v']), metric['u'],
                 sample['seq'], captured_at, cohort, sample['mono_ns'], sample['wall_ms'],
                 sample['boot'], sample['gen'], int(gap)))
    connection.commit()


split = len(annotated) // 2
connection = sqlite3.connect(db_path)
connection.executescript(schema)
ingest(connection, annotated[:split])
connection.close()

connection = sqlite3.connect(db_path)
ingest(connection, annotated[split:])
labels = connection.execute(
    'SELECT DISTINCT execution_attempt_id, cohort, boot, gen FROM sample').fetchall()
assert labels == [(execution_id, cohort, boot, generation)], labels
expected_rows = sum(len(sample['metrics']) for sample in samples)
total_rows = connection.execute('SELECT COUNT(*) FROM sample').fetchone()[0]
assert total_rows == expected_rows, (total_rows, expected_rows)


def rate_between(before, after, name):
    first = next(metric['v'] for metric in before['metrics'] if metric['n'] == name)
    second = next(metric['v'] for metric in after['metrics'] if metric['n'] == name)
    seconds = (after['mono_ns'] - before['mono_ns']) / 1_000_000_000.0
    if seconds <= 0 or second < first:
        return None
    return (second - first) / seconds


guest_samples = [sample for sample in samples if sample['scope'] == 'guest']
consecutive = [
    (before, after)
    for before, after in zip(guest_samples, guest_samples[1:])
    if (before['boot'], before['gen']) == (after['boot'], after['gen'])
    and after['seq'] == before['seq'] + 1
    and after['mono_ns'] > before['mono_ns']
]
assert consecutive, 'no consecutive guest pair for a capture-time rate'
counter_name = 'cpu.busy_usec_total'
positive_rates = []
for before, after in consecutive:
    value = rate_between(before, after, counter_name)
    if value is not None and value > 0:
        positive_rates.append((before, after, value))
assert positive_rates, 'no positive capture-time rate'
rate_before, rate_after, capture_rate = positive_rates[len(positive_rates) // 2]
receipt_shifted = dict(rate_after)
receipt_shifted['wall_ms'] = rate_after['wall_ms'] + 999_999_999
assert rate_between(rate_before, receipt_shifted, counter_name) == capture_rate, \
    'a rate changed when receipt time changed'

in_outage = [sample for sample in samples
             if outage_start <= sample['wall_ms'] < outage_end]
assert not in_outage, 'link-outage samples were not removed'
gap_rows = connection.execute(
    'SELECT COUNT(*) FROM sample WHERE wall_ms >= ? AND wall_ms < ?',
    (outage_start, outage_end)).fetchone()[0]
assert gap_rows == 0, 'the link outage interval contains ingested rows'
zero_fills = connection.execute(
    'SELECT COUNT(*) FROM sample WHERE wall_ms >= ? AND wall_ms < ? AND value = 0',
    (outage_start, outage_end)).fetchone()[0]
assert zero_fills == 0, 'a gap was filled with zero-valued samples'

bucket_ns = 2_000_000_000
raw_buckets = collections.defaultdict(list)
for sample, gap in annotated:
    if sample['scope'] != 'guest':
        continue
    slot = sample['mono_ns'] // bucket_ns
    for metric in sample['metrics']:
        if metric['n'] == 'cpu.busy_cores':
            raw_buckets[slot].append({
                'mono': sample['mono_ns'],
                'seq': sample['seq'],
                'identity': (sample['boot'], sample['gen']),
                'gap': gap,
                'value': metric['v'],
            })

buckets = []
for slot in sorted(raw_buckets):
    entries = sorted(raw_buckets[slot], key=lambda entry: entry['mono'])
    values_in_bucket = [entry['value'] for entry in entries]
    coverage = 0
    gap_count = 0
    previous = None
    for entry in entries:
        if entry['gap']:
            gap_count += 1
        if (previous is not None and not entry['gap']
                and previous['identity'] == entry['identity']
                and entry['seq'] == previous['seq'] + 1
                and entry['mono'] > previous['mono']):
            coverage += entry['mono'] - previous['mono']
        previous = entry
    buckets.append({
        'slot': slot,
        'count': len(values_in_bucket),
        'coverage': coverage,
        'min': min(values_in_bucket),
        'max': max(values_in_bucket),
        'sum': sum(values_in_bucket),
        'first': values_in_bucket[0],
        'last': values_in_bucket[-1],
        'gap_count': gap_count,
        'has_gap': gap_count > 0,
    })
assert buckets, 'no buckets were produced'
for bucket in buckets:
    assert bucket['count'] > 0
    assert bucket['min'] <= bucket['max']
    assert bucket['sum'] >= bucket['max']
assert any(bucket['has_gap'] for bucket in buckets), 'no bucket flagged a gap'
peak = max(bucket['max'] for bucket in buckets)
assert peak > 0.5, 'buckets did not preserve the cpu peak'

cutoff = rfc3339_ms(markers[('allocate', 'begin')])
rows_before_retention = connection.execute('SELECT COUNT(*) FROM sample').fetchone()[0]
connection.execute('DELETE FROM sample WHERE captured_at < ?', (cutoff,))
connection.commit()
rows_after_retention = connection.execute('SELECT COUNT(*) FROM sample').fetchone()[0]
assert 0 < rows_after_retention < rows_before_retention, \
    (rows_before_retention, rows_after_retention)
assert peak > 0.5, 'compaction lost the preserved peak after retention'
connection.close()

print(f'telemetry_history: samples={len(samples)} dropped={dropped} '
      f'scopes={sorted(scopes)} nproc={vcpus} allocated_vcpus={allocated_vcpus}')
print(f'telemetry_history: phases={phases} gaps={len(seen_gaps)} cohort={cohort}')
print(f'telemetry_history: units cores={cores_unit} percent={percent_unit} '
      f'busy_peak={busy_peak:.3f} correlated_samples={checked}')
print(f'telemetry_history: restart labels={labels} rows={total_rows} '
      f'capture_rate={capture_rate:.3f}')
print(f'telemetry_history: outage gap_flagged='
      f'{any(b["has_gap"] for b in buckets)} zero_fills={zero_fills}')
print(f'telemetry_history: retention rows={rows_before_retention}->'
      f'{rows_after_retention} buckets={len(buckets)} peak={peak:.3f}')
PY
cat "$state/assert.out"

printf 'telemetry_history: checking for leftover sr-* resources\n'
leftovers=$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}' || true)
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"

printf 'telemetry_history: ok\n'
