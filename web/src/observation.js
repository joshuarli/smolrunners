export const GIB = 1073741824;

export function toEpochMs(capturedAt) {
  if (typeof capturedAt === 'number') return capturedAt;
  const parsed = Date.parse(capturedAt);
  return Number.isFinite(parsed) ? parsed : 0;
}

export function groupSeries(series) {
  const groups = new Map();
  for (const row of series ?? []) {
    if (!groups.has(row.metric)) groups.set(row.metric, []);
    groups.get(row.metric).push({
      metric: row.metric,
      scope: row.scope,
      unit: row.unit,
      value: row.value,
      sequence: row.sequence,
      capturedAt: toEpochMs(row.captured_at),
    });
  }
  for (const points of groups.values()) {
    points.sort((a, b) => a.sequence - b.sequence || a.capturedAt - b.capturedAt);
  }
  return groups;
}

export function peakOf(series, metric) {
  const points = groupSeries(series).get(metric);
  if (!points || points.length === 0) return null;
  return points.reduce((peak, point) => Math.max(peak, point.value), 0);
}

export function reportRow(rows, id) {
  return (rows ?? []).find((row) => String(row.id) === String(id)) ?? null;
}

export function limitsFromProfile(profile) {
  if (!profile) return null;
  return {
    vcpu: profile.vcpu,
    memoryGib: profile.memory_gib,
    memoryBytes: profile.memory_gib * GIB,
  };
}

export function formatBytes(bytes) {
  if (bytes === null || bytes === undefined) return '<missing>';
  if (!Number.isFinite(bytes)) return String(bytes);
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${units[index]}`;
}

export function formatNumber(value, digits = 2) {
  if (value === null || value === undefined || !Number.isFinite(value)) return '<missing>';
  return value.toFixed(digits);
}

export function chartPoints(points) {
  return (points ?? []).map((point) => [point.capturedAt, point.value]);
}

export function alignedChart(series, metrics) {
  const groups = groupSeries(series);
  const primary = groups.get(metrics[0]) ?? [];
  const bySequence = new Map(
    metrics.map((metric) => [
      metric,
      new Map((groups.get(metric) ?? []).map((point) => [point.sequence, point.value])),
    ]),
  );
  return {
    x: primary.map((point) => point.capturedAt),
    columns: metrics.map((metric) => primary.map((point) => bySequence.get(metric).get(point.sequence) ?? null)),
  };
}

export function chartData(bundle, metrics) {
  if (bundle === null || typeof bundle !== 'object') {
    throw new TypeError('chartData needs a resolved job bundle, not a resource getter');
  }
  const aligned = alignedChart(bundle.series, metrics);
  return [aligned.x, ...aligned.columns];
}

export function selectChartSeries(series, metrics) {
  const groups = groupSeries(series);
  return metrics
    .map((metric) => ({ metric, points: groups.get(metric) ?? [] }))
    .filter((entry) => entry.points.length > 0);
}

export function lifecycleEvents(job) {
  const events = [];
  if (job?.demand_at_ms) events.push({ at: job.demand_at_ms, label: 'demand', detail: job.admission_reason });
  if (job?.start_at_ms) events.push({ at: job.start_at_ms, label: 'start', detail: `host ${job.host_id ?? '<missing>'}` });
  if (job?.finish_at_ms) events.push({ at: job.finish_at_ms, label: 'finish', detail: job.result ?? '<missing>' });
  return events.sort((a, b) => a.at - b.at);
}

export function activityOverlays(series, threshold = 0.01) {
  const points = groupSeries(series).get('cpu.busy_cores') ?? [];
  const overlays = [];
  let start = null;
  for (const point of points) {
    if (point.value > threshold && start === null) start = point.capturedAt;
    if (point.value <= threshold && start !== null) {
      overlays.push({ from: start, to: point.capturedAt, label: 'active step' });
      start = null;
    }
  }
  if (start !== null && points.length) {
    overlays.push({ from: start, to: points[points.length - 1].capturedAt, label: 'active step' });
  }
  return overlays;
}

export function queueReason(job) {
  if (!job) return '<missing>';
  if (job.state === 'reserved' && job.admission_reason) return job.admission_reason;
  return job.result ?? job.admission_reason ?? '<missing>';
}

export function hostCapacity(hosts) {
  return (hosts ?? []).reduce(
    (total, host) => {
      if (host.admission_state !== 'eligible') return total;
      total.vcpu += host.total_vcpu;
      total.memoryGib += host.total_memory_gib;
      total.hosts += 1;
      return total;
    },
    { hosts: 0, vcpu: 0, memoryGib: 0 },
  );
}
