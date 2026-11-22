# Operations

This document describes the local, nested-host deployment used by the campaign. It is not a
production installation guide: live GitHub enrollment and native-hardware qualification remain
external prerequisites (see `smolrunners-campaign.md`, **Needs environment**).

## Install

Setup checks the host and prepares scratch space. It does not install system packages.

```sh
cd ~/repos/smolrunners
make setup
make fetch
make check
```

Every pinned input is declared in `sources.lock.json` and fetched through
`bootstrap/buildlib-run` by `scripts/fetch-inputs.py`. Do not download an input with `curl` into
the source tree: image recipes are the only exception, and they verify each OCI manifest and
layer digest before extraction.

`TMPDIR` for repository commands is `.work/tmp`. All runtime state, downloaded inputs and
worktrees live under `.work/`, which is ignored through `.git/info/exclude`.

## Run in the foreground

```sh
export TMPDIR="$PWD/.work/tmp"
export SMOLRUNNERS_STATE_DIR="$PWD/.work/api-state"
export SMOLRUNNERS_API_URL=http://127.0.0.1:8787
cargo build --workspace
./target/debug/smolrunners serve --listen 127.0.0.1:8787 --state-dir "$SMOLRUNNERS_STATE_DIR"
```

`serve` starts the HTTP API on the given address. The state directory holds `state.json` for
control-plane records and `telemetry.sqlite` for sample history. Both are created on first open.
`SMOLRUNNERS_STATE_DIR` and `SMOLRUNNERS_API_URL` provide the defaults when the flags or
environment are absent.

For fleet enrollment, add `--tls-ca CA.pem --tls-cert server.pem --tls-key server.key`. On that
path the listener requires a verified client certificate and derives host identity from it; a
caller cannot declare its identity in the body. Generate a test CA and certificates with
`tests/cli/generate-certs.sh`; no private keys are committed. Without the TLS flags the listener
is plain HTTP and reports `mtls: false` — use it only for loopback operator commands.

The connected coordinator is `serve --pilot CONFIG.toml`; `docs/pilot.md` is the runbook for a
live host. `make test-vm T=connected_job` runs it inside the nested host against the fake broker.

The nested Linux host is separate from the coordinator:

```sh
sh scripts/linux-host.sh up
sh scripts/linux-host.sh sync
sh scripts/linux-host.sh status
sh scripts/linux-host.sh exec 'sh tests/vm/smoke.sh'
sh scripts/linux-host.sh down
```

Only smolvm machines named `sr-*` belong to this repository. Never delete a machine with another
name. `make test-vm T=<name>` performs `up`, `sync` and `exec` in one step.

## Supervision

No service-manager unit is shipped. Run `serve` under the process supervisor of your choice with
these requirements:

- a stable `state_dir` across restarts, on a filesystem that supports SQLite WAL;
- one coordinator process per state directory — concurrent writers are not supported;
- a restart policy that does not kill the process group: host agents run in their own process
  groups so guests survive a coordinator restart, and the restarted coordinator reattaches to them;
- no automatic cleaning of `.work/` while a host or guest is running.

Capture stderr: startup errors, including SQLite pragma verification failures, are emitted there.
Exit code 2 from a CLI command means a usage or validation failure.

## Backup and restore

Back up the state directory consistently. The durable store configures SQLite WAL with `FULL`
synchronous mode and verifies the effective pragmas on open. The core library exposes
`store::backup_to`; prefer an online backup over copying the database file while a process is
running. A file copy is only safe when no coordinator or collector is writing.

A restore is not yet an admission gate. The fault corpus proves that restored hosts are stale
until they re-heartbeat, but it does not enforce admission from the coordinator binary. Until
that path is connected, do not restore into a live deployment and expect automatic fencing.

## Updates

1. Stop the coordinator and any running jobs; confirm no `sr-*` machines remain.
2. Back up the state directory.
3. Update the checkout, then run `make setup`, `make fetch`, `make check`.
4. Run `make test-fast` and the VM gates relevant to the changed components.
5. Start the coordinator with the same state directory.

Inputs, the Rust toolchain and the OCI images are pinned. Update a pin only by changing
`sources.lock.json` and `docs/upstream-evidence.md` together; image digests live in each
`images/*/manifest.json`.

## Acceptance caveats

`make test-vm` cross-builds the guest binaries from current source (`scripts/build-vm-binaries.sh`)
and `scripts/linux-host.sh sync` ships them with the tree. Before 2026-11-22 the sync skipped the
gitignored binary directories, so VM gate records from before then may have exercised stale
binaries.
