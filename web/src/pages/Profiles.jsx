import { createResource, For, Match, Switch } from 'solid-js';

import { api } from '../api.js';

export default function Profiles() {
  const [reply] = createResource(() => api.profiles());
  const profiles = () => reply()?.data?.profiles ?? [];

  return (
    <>
      <h1>Profiles</h1>
      <Switch>
        <Match when={reply.error}><p class="error">Could not load profiles: {String(reply.error)}</p></Match>
        <Match when={reply.loading && !reply()}><p class="muted">Loading profiles…</p></Match>
        <Match when={reply()}>
          <table>
            <thead><tr><th>Name</th><th>Revision</th><th>Family</th><th>Shape</th><th>Image</th><th>Promoted</th></tr></thead>
            <tbody>
              <For each={profiles()}>{(profile) => (
                <tr>
                  <td class="mono">{profile.name}</td>
                  <td class="mono" title={profile.revision_id}>#{profile.revision_number} {profile.revision_id.slice(0, 12)}…</td>
                  <td>{profile.family ?? '—'}</td>
                  <td>{profile.vcpu} vCPU / {profile.memory_gib} GiB</td>
                  <td>{profile.distro} {profile.release} {profile.arch}</td>
                  <td><span classList={{ badge: true, promoted: profile.promoted }}>{profile.promoted ? 'promoted' : 'candidate'}</span></td>
                </tr>
              )}</For>
            </tbody>
          </table>
          <p class="muted">A profile revision is content addressed; a promoted revision is what alias labels resolve to.</p>
        </Match>
      </Switch>
    </>
  );
}
