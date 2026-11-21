import { createResource, For, Match, Switch } from 'solid-js';

import { api } from '../api.js';
import { formatBytes, hostCapacity } from '../observation.js';

export default function Hosts() {
  const [reply] = createResource(() => api.hosts());
  const hosts = () => reply()?.data?.hosts ?? [];
  const capacity = () => hostCapacity(hosts());

  return (
    <>
      <h1>Hosts &amp; capacity</h1>
      <Switch>
        <Match when={reply.error}>
          <p class="error">Could not load hosts: {String(reply.error)}</p>
        </Match>
        <Match when={reply.loading && !reply()}>
          <p class="muted">Loading hosts…</p>
        </Match>
        <Match when={reply()}>
          <div class="cards">
            <div class="card"><span class="label">Eligible hosts</span><strong>{capacity().hosts}</strong></div>
            <div class="card"><span class="label">Advertised vCPUs</span><strong>{capacity().vcpu}</strong></div>
            <div class="card"><span class="label">Advertised memory</span><strong>{formatBytes(capacity().memoryGib * 1073741824)}</strong></div>
          </div>
          <table>
            <thead><tr><th>ID</th><th>Host</th><th>Arch</th><th>Admission</th><th>vCPU</th><th>Memory</th><th>Incarnation</th><th>Last contact</th></tr></thead>
            <tbody>
              <For each={hosts()}>{(host) => (
                <tr>
                  <td>{host.id}</td>
                  <td class="mono">{host.hostname} <span class="muted">({host.host_key})</span></td>
                  <td>{host.arch}</td>
                  <td><span classList={{ badge: true, drained: host.admission_state !== 'eligible' }}>{host.admission_state}</span></td>
                  <td>{host.total_vcpu}</td>
                  <td>{formatBytes(host.total_memory_gib * 1073741824)}</td>
                  <td>{host.incarnation}</td>
                  <td>{host.last_contact_ms ?? <span class="muted">missing: {reply().missing?.last_contact_ms ?? 'no contact'}</span>}</td>
                </tr>
              )}</For>
            </tbody>
          </table>
          <p class="muted">Capacity counts only hosts in the eligible admission state.</p>
        </Match>
      </Switch>
    </>
  );
}
