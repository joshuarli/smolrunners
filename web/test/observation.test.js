import assert from 'node:assert/strict';
import test from 'node:test';

import {
  GIB,
  activityOverlays,
  alignedChart,
  chartData,
  chartPoints,
  formatBytes,
  groupSeries,
  hostAllowance,
  hostCapacity,
  isActive,
  lifecycleEvents,
  limitsFromProfile,
  peakOf,
  queueReason,
  reportRow,
  selectChartSeries,
} from '../src/observation.js';

const series = [
  { metric: 'cpu.busy_cores', scope: 'guest', unit: 'cores', value: 1.5, sequence: 2, captured_at: 200 },
  { metric: 'cpu.busy_cores', scope: 'guest', unit: 'cores', value: 3.25, sequence: 1, captured_at: 100 },
  { metric: 'cpu.busy_cores', scope: 'guest', unit: 'cores', value: 0.5, sequence: 3, captured_at: 300 },
  { metric: 'mem.used_bytes', scope: 'guest', unit: 'bytes', value: 2 * GIB, sequence: 1, captured_at: 100 },
  { metric: 'mem.used_bytes', scope: 'guest', unit: 'bytes', value: 5 * GIB, sequence: 2, captured_at: 200 },
  { metric: 'disk.read_bytes_delta', scope: 'guest', unit: 'bytes', value: 4096, sequence: 1, captured_at: 100 },
];

test('peaks match the report values the CLI prints from the same API', () => {
  const rows = [
    { id: 7, peak_cpu_cores: 3.25, peak_memory_bytes: 5 * GIB, samples: 6 },
    { id: 8, peak_cpu_cores: 0, peak_memory_bytes: 0, samples: 0 },
  ];
  const row = reportRow(rows, 7);
  assert.equal(peakOf(series, 'cpu.busy_cores'), row.peak_cpu_cores);
  assert.equal(peakOf(series, 'mem.used_bytes'), row.peak_memory_bytes);
  assert.equal(reportRow(rows, '8').id, 8);
  assert.equal(reportRow(rows, 99), null);
});

test('chart points follow sample sequence, not arrival order', () => {
  const groups = groupSeries(series);
  const cpu = groups.get('cpu.busy_cores');
  assert.deepEqual(cpu.map((point) => point.value), [3.25, 1.5, 0.5]);
  assert.deepEqual(chartPoints(cpu), [[100, 3.25], [200, 1.5], [300, 0.5]]);
  assert.equal(peakOf([], 'cpu.busy_cores'), null);
  assert.equal(peakOf(series, 'net.rx_bytes_total'), null);
});

test('limits come from the profile the CLI inspects', () => {
  assert.deepEqual(limitsFromProfile({ vcpu: 4, memory_gib: 8 }), {
    vcpu: 4,
    memoryGib: 8,
    memoryBytes: 8 * GIB,
  });
  assert.equal(limitsFromProfile(null), null);
  assert.equal(formatBytes(5 * GIB), '5.00 GiB');
  assert.equal(formatBytes(null), '<missing>');
});

test('chart columns align on the primary sample sequence', () => {
  const aligned = alignedChart(series, ['mem.used_bytes', 'cpu.busy_cores']);
  assert.deepEqual(aligned.x, [100, 200]);
  assert.deepEqual(aligned.columns, [[2 * GIB, 5 * GIB], [3.25, 1.5]]);
});

test('chart data is built from a resolved bundle, not a resource getter', () => {
  const bundle = { series };
  const [x, cpu] = chartData(bundle, ['cpu.busy_cores']);
  assert.deepEqual(x, [100, 200, 300]);
  assert.deepEqual(cpu, [3.25, 1.5, 0.5]);
  assert.throws(() => chartData(() => bundle, ['cpu.busy_cores']), /resource getter/);
  assert.deepEqual(chartData({ series: [] }, ['cpu.busy_cores']), [[], []]);
});

test('selected chart series keep only metrics with samples', () => {
  const selected = selectChartSeries(series, ['cpu.busy_cores', 'mem.used_bytes', 'net.rx_bytes_delta']);
  assert.deepEqual(selected.map((entry) => entry.metric), ['cpu.busy_cores', 'mem.used_bytes']);
});

test('lifecycle and activity overlays use only API facts', () => {
  const events = lifecycleEvents({
    demand_at_ms: 100,
    start_at_ms: 200,
    finish_at_ms: 400,
    admission_reason: 'profile matched advertised labels',
    host_id: 3,
    result: 'success',
  });
  assert.deepEqual(events.map((event) => event.label), ['demand', 'start', 'finish']);
  const phases = [
    { metric: 'cpu.busy_cores', value: 0, sequence: 1, captured_at: 100 },
    { metric: 'cpu.busy_cores', value: 2, sequence: 2, captured_at: 200 },
    { metric: 'cpu.busy_cores', value: 1, sequence: 3, captured_at: 300 },
    { metric: 'cpu.busy_cores', value: 0, sequence: 4, captured_at: 400 },
  ];
  assert.deepEqual(activityOverlays(phases), [{ from: 200, to: 400, label: 'active step' }]);
  assert.deepEqual(activityOverlays([]), []);
});

test('queue reasons and capacity reflect admission state', () => {
  assert.equal(queueReason({ state: 'reserved', admission_reason: 'no eligible host' }), 'no eligible host');
  assert.equal(queueReason({ state: 'done', result: 'success', admission_reason: 'matched' }), 'success');
  const total = hostCapacity([
    { admission_state: 'eligible', total_vcpu: 4, total_memory_gib: 8 },
    { admission_state: 'drained', total_vcpu: 4, total_memory_gib: 8 },
    { admission_state: 'eligible', total_vcpu: 8, total_memory_gib: 16 },
  ]);
  assert.deepEqual(total, { hosts: 2, vcpu: 12, memoryGib: 24 });
});

test('every non-terminal coordinator state keeps the job page polling', () => {
  for (const state of ['provisioning', 'idle', 'busy', 'reserved', 'queued']) {
    assert.equal(isActive(state), true, state);
  }
  for (const state of ['done', 'failed', 'cancelled', 'error', undefined, '']) {
    assert.equal(isActive(state), false, String(state));
  }
});

test('host allowance comes from the limits frozen on the execution', () => {
  const job = { limits: { guest_vcpu: 2, guest_memory_gib: 2, storage_gib: 8, host_cpu_max: '225000 100000', host_memory_max_bytes: 2684354560, host_pids_max: 512 } };
  assert.deepEqual(hostAllowance(job), { cpuMax: '225000 100000', memoryMaxBytes: 2684354560, pidsMax: 512, storageGib: 8 });
  assert.equal(hostAllowance({}), null);
});
