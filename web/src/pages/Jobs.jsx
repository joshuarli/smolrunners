import { createResource, For, Match, Switch } from 'solid-js';

import { api } from '../api.js';
import { formatBytes, formatNumber, queueReason, reportRow } from '../observation.js';

export default function Jobs() {
  const [bundle] = createResource(() => Promise.all([api.jobs(), api.report()]));
  const jobs = () => bundle()?.[0]?.data?.jobs ?? [];
  const rows = () => bundle()?.[1]?.data?.rows ?? [];

  return (
    <>
      <h1>Jobs</h1>
      <Switch>
        <Match when={bundle.error}><p class="error">Could not load jobs: {String(bundle.error)}</p></Match>
        <Match when={bundle.loading && !bundle()}><p class="muted">Loading jobs…</p></Match>
        <Match when={bundle()}>
          <table>
            <thead><tr><th>ID</th><th>Name</th><th>State</th><th>Profile</th><th>Queue reason</th><th>Peak CPU</th><th>Peak RAM</th><th>Samples</th></tr></thead>
            <tbody>
              <For each={jobs()}>{(job) => {
                const row = () => reportRow(rows(), job.id);
                return (
                  <tr>
                    <td><a href={`#/jobs/${job.id}`}>{job.id}</a></td>
                    <td class="mono">{job.name}</td>
                    <td><span class="badge">{job.state}</span></td>
                    <td>{job.profile}</td>
                    <td class="reason">{queueReason(job)}</td>
                    <td>{formatNumber(row()?.peak_cpu_cores, 3)} cores</td>
                    <td>{formatBytes(row()?.peak_memory_bytes)}</td>
                    <td>{row()?.samples ?? 0}</td>
                  </tr>
                );
              }}</For>
            </tbody>
          </table>
          <p class="muted">Peaks come from the same <code>/api/v1/jobs/report</code> data that <code>jobs report</code> prints.</p>
        </Match>
      </Switch>
    </>
  );
}
