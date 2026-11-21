import { createSignal, onCleanup, Switch, Match } from 'solid-js';

import Hosts from './pages/Hosts.jsx';
import Profiles from './pages/Profiles.jsx';
import Jobs from './pages/Jobs.jsx';
import JobDetail from './pages/JobDetail.jsx';

function currentHash() {
  const hash = window.location.hash.replace(/^#/, '');
  return hash.length ? hash : '/hosts';
}

export default function App() {
  const [route, setRoute] = createSignal(currentHash());
  const listener = () => setRoute(currentHash());
  window.addEventListener('hashchange', listener);
  onCleanup(() => window.removeEventListener('hashchange', listener));

  const path = () => route().split('/').filter(Boolean);
  const isJobDetail = () => path()[0] === 'jobs' && path().length === 2;

  return (
    <div class="shell">
      <header class="topbar">
        <a class="brand" href="#/hosts">smolrunners</a>
        <nav>
          <a classList={{ active: path()[0] === 'hosts' }} href="#/hosts">Hosts &amp; capacity</a>
          <a classList={{ active: path()[0] === 'profiles' }} href="#/profiles">Profiles</a>
          <a classList={{ active: path()[0] === 'jobs' && !isJobDetail() }} href="#/jobs">Jobs</a>
        </nav>
        <span class="source">observations via /api/v1</span>
      </header>
      <main>
        <Switch fallback={<Hosts />}>
          <Match when={path()[0] === 'hosts'}><Hosts /></Match>
          <Match when={path()[0] === 'profiles'}><Profiles /></Match>
          <Match when={isJobDetail()}><JobDetail id={path()[1]} /></Match>
          <Match when={path()[0] === 'jobs'}><Jobs /></Match>
        </Switch>
      </main>
    </div>
  );
}
