import { createEffect, createResource, For, Match, onCleanup, Show, Switch } from 'solid-js';

import { api } from '../api.js';
import Chart from '../components/Chart.jsx';
import {
  activityOverlays,
  chartData,
  formatBytes,
  formatNumber,
  lifecycleEvents,
  limitsFromProfile,
  reportRow,
} from '../observation.js';

const COLORS = ['#38bdf8', '#a78bfa', '#34d399', '#fbbf24'];

async function loadJob(id) {
  const [jobReply, reportReply, profilesReply, metricsReply, explainReply] = await Promise.all([
    api.job(id),
    api.report(),
    api.profiles(),
    api.metrics(id),
    api.explain(id),
  ]);
  const job = jobReply.data;
  const currentProfile = (profilesReply.data.profiles ?? []).find((entry) => entry.name === job.profile) ?? null;
  const profile = job.allocated_vcpu && job.allocated_memory_gib
    ? { vcpu: job.allocated_vcpu, memory_gib: job.allocated_memory_gib }
    : currentProfile;
  return {
    job,
    profile,
    row: reportRow(reportReply.data.rows ?? [], id),
    series: metricsReply.data.series ?? [],
    sampleCount: metricsReply.data.samples ?? 0,
    explain: explainReply.data,
    missing: { ...jobReply.missing, ...metricsReply.missing },
  };
}

const ACTIVE_STATES = new Set(['queued', 'reserved', 'busy']);

function timeLabel(ms) {
  return Number.isFinite(ms) ? new Date(ms).toISOString() : '<missing>';
}

export default function JobDetail(props) {
  const [bundle, { refetch }] = createResource(() => props.id, loadJob);
  let pollTimer;
  const stopPolling = () => {
    if (pollTimer !== undefined) {
      clearInterval(pollTimer);
      pollTimer = undefined;
    }
  };
  createEffect(() => {
    if (ACTIVE_STATES.has(bundle()?.job?.state)) {
      if (pollTimer === undefined) {
        pollTimer = setInterval(() => refetch(), 1000);
      }
    } else {
      stopPolling();
    }
  });
  onCleanup(stopPolling);
  const limits = () => limitsFromProfile(bundle()?.profile);
  const overlays = () => activityOverlays(bundle()?.series ?? []);
  const events = () => lifecycleEvents(bundle()?.job ?? {});
  const overlayTimes = () => overlays().map((overlay) => ({ ...overlay, x0: overlay.from, x1: overlay.to }));

  return (
    <>
      <Switch>
        <Match when={bundle.error}><p class="error">Could not load job: {String(bundle.error)}</p></Match>
        <Match when={bundle.loading && !bundle()}><p class="muted">Loading job…</p></Match>
        <Match when={bundle()}>
          <div class="job-head">
            <div>
              <a class="back" href="#/jobs">← jobs</a>
              <h1>{bundle().job.name} <span class="muted">#{bundle().job.id}</span></h1>
              <p>
                <span class="badge">{bundle().job.state}</span>
                <span class="muted">profile {bundle().job.profile}{bundle().job.profile_revision_id ? ` rev ${bundle().job.profile_revision_id}` : ''}</span>
                <span class="muted">host {bundle().job.host_id ?? '<missing>'}</span>
              </p>
              <p class="reason">Queue reason: {bundle().job.admission_reason}</p>
            </div>
            <Show when={bundle().job.github_url} fallback={<span class="muted">GitHub run: not attached</span>}>
              <a class="github" href={bundle().job.github_url} target="_blank" rel="noreferrer">Open GitHub run ↗</a>
            </Show>
          </div>

          <section class="panel">
            <div class="panel-head"><h2>Peaks against limits</h2><span class="muted">{bundle().sampleCount} samples</span></div>
            <div class="cards">
              <div class="card" data-testid="peak-cpu" data-value={bundle().row?.peak_cpu_cores ?? 0}>
                <span class="label">Peak CPU</span><strong>{formatNumber(bundle().row?.peak_cpu_cores, 3)} cores</strong>
              </div>
              <div class="card" data-testid="cpu-limit" data-value={limits()?.vcpu ?? 0}>
                <span class="label">CPU limit</span><strong>{limits()?.vcpu ?? '<missing>'} vCPU</strong>
              </div>
              <div class="card" data-testid="peak-memory" data-value={bundle().row?.peak_memory_bytes ?? 0}>
                <span class="label">Peak RAM</span><strong>{formatBytes(bundle().row?.peak_memory_bytes)}</strong>
              </div>
              <div class="card" data-testid="memory-limit" data-value={limits()?.memoryBytes ?? 0}>
                <span class="label">RAM limit</span><strong>{formatBytes(limits()?.memoryBytes)}</strong>
              </div>
            </div>
            <p class="muted">Peaks are the values <code>jobs report</code> reads from the same coordinator; limits are the profile <code>jobs inspect</code> resolves to.</p>
            <Show when={Object.keys(bundle().missing).length > 0}>
              <ul class="missing">
                <For each={Object.entries(bundle().missing)}>{([field, reason]) => <li>missing {field}: {reason}</li>}</For>
              </ul>
            </Show>
          </section>

          <div class="charts">
            <Chart
              title="CPU used vs allocated vCPUs"
              series={[{ label: 'cores used', color: COLORS[0] }]}
              data={chartData(bundle(), ['cpu.busy_cores'])}
              limit={limits()?.vcpu}
              overlays={overlayTimes()}
            />
            <Chart
              title="Memory used vs profile limit"
              series={[{ label: 'guest used', color: COLORS[1] }, { label: 'workload cgroup', color: COLORS[2] }]}
              data={chartData(bundle(), ['mem.used_bytes', 'cgroup.memory.current'])}
              limit={limits()?.memoryBytes}
            />
            <Chart
              title="I/O throughput"
              series={[{ label: 'disk read', color: COLORS[0] }, { label: 'disk write', color: COLORS[1] }]}
              data={chartData(bundle(), ['disk.read_bytes_delta', 'disk.write_bytes_delta'])}
            />
            <Chart
              title="Network throughput"
              series={[{ label: 'rx', color: COLORS[2] }, { label: 'tx', color: COLORS[3] }]}
              data={chartData(bundle(), ['net.rx_bytes_delta', 'net.tx_bytes_delta'])}
            />
            <Chart
              title="Guest cgroup-root pressure (some, cumulative)"
              series={[{ label: 'guest cpu', color: COLORS[0] }, { label: 'guest memory', color: COLORS[1] }]}
              data={chartData(bundle(), ['host.pressure.cpu.some_total', 'host.pressure.memory.some_total'])}
            />
          </div>

          <section class="panel">
            <div class="panel-head"><h2>Lifecycle</h2></div>
            <ul class="timeline">
              <For each={events()}>{(event) => (
                <li><span class="mono">{timeLabel(event.at)}</span> <strong>{event.label}</strong> <span class="muted">{event.detail}</span></li>
              )}</For>
            </ul>
          </section>

          <section class="panel">
            <div class="panel-head"><h2>Observed activity steps</h2></div>
            <ul class="timeline">
              <For each={overlays()}>{(overlay) => (
                <li><span class="mono">{timeLabel(overlay.from)} → {timeLabel(overlay.to)}</span> <strong>{overlay.label}</strong></li>
              )}</For>
            </ul>
            <p class="muted">Step bands are inferred from CPU activity samples; authoritative GitHub step times require the REST enrichment path (NE1).</p>
          </section>
        </Match>
      </Switch>
    </>
  );
}
