# smolrunners operator UI

Solid + Vite operator surface. Every fact comes from the coordinator's
`/api/v1/*` endpoints, the same ones `smolrunners` CLI commands use; there is
no separate observation path. uPlot is the single charting library.

## Build and test

```sh
cd web
npm ci
npm test
npm run build
```

`npm run check` runs both. `make web-test` should run
`cd web && npm ci && npm test && npm run build`.

`vite.config.js` proxies `/api` to `SMOLRUNNERS_API_URL` (default
`http://127.0.0.1:8791`) in both `dev` and `preview`, because the coordinator
sends no CORS headers. Start the UI with `npm run dev` (port 5173) or
`npm run preview` (port 4173).

## Browser parity check against `cli_e2e`

The coordinator started by `tests/vm/cli_e2e.sh` binds `127.0.0.1:8791` inside
`sr-dev` and is killed by that script's EXIT trap. It leaves its state at
`/work/tmp/cli-e2e-state/api` in the guest. The nested host must expose that
port to the Mac before the browser can reach it. `smolvm --auto-ports` works
for small replies but truncates the ~1 MiB `jobs/.../metrics` body, so use an
explicit mapping and bind the coordinator to `0.0.0.0` instead.

Set `REPO` to the checkout (the parent checkout has `.work/runtime`; in a
worktree, symlink it: `ln -sfn <parent>/.work/runtime "$REPO/.work/runtime"`).

```sh
export REPO=/Users/josh/repos/smolrunners
export TMPDIR="$REPO/.work/tmp"
export SMOLRUNNERS_LINUX_HOST_CPUS=4 SMOLRUNNERS_LINUX_HOST_MEMORY_MIB=8192
export HOST_HOME="$REPO/.work/linux-host/home"

sh "$REPO/scripts/linux-host.sh" up
sh "$REPO/scripts/linux-host.sh" down
env -i PATH="$PATH" HOME="$HOST_HOME" TMPDIR="$TMPDIR" \
  "$REPO/.work/runtime/darwin-arm64/smolvm" machine update --name sr-dev \
  --no-auto-ports -p 8791:8791
sh "$REPO/scripts/linux-host.sh" up
make -C "$REPO" test-vm T=cli_e2e

sh "$REPO/scripts/linux-host.sh" exec 'setsid nohup \
  /work/smolrunners/tests/cli/bin/smolrunners serve \
  --listen 0.0.0.0:8791 \
  --state-dir /work/tmp/cli-e2e-state/api \
  >/work/tmp/coord-live.log 2>&1 </dev/null &'

cd "$REPO/web" && npm run dev
```

Then open `http://127.0.0.1:5173/#/jobs/4` in `agent-browser` and compare the
`data-value` attributes on `peak-cpu`, `cpu-limit`, `peak-memory` and
`memory-limit` against `jobs report --json` and `profiles inspect --json` run
inside `sr-dev` with `SMOLRUNNERS_API_URL=http://127.0.0.1:8791`. Job ids are
assigned by the coordinator; `jobs list --json` gives the current one.

Observed on the worktree `sr-dev` (job 4):

| Field | Browser | CLI |
| --- | --- | --- |
| peak-cpu | 2.0869565217391304 | 2.0869565217391304 |
| cpu-limit | 4 | 4 |
| peak-memory | 201072640 | 201072640 |
| memory-limit | 8589934592 | 8589934592 |

Step overlays are inferred from CPU activity samples in the API. Authoritative
GitHub step times are only available through the REST enrichment path (NE1).
