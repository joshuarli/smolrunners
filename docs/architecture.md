# Architecture

smolrunners is one coordinator with SQLite and one or more host agents that may share a machine.

| Component | Language | Responsibility |
| --- | --- | --- |
| `crates/core` | Rust | domain, store, ledger, lifecycle, adapter protocol codec, telemetry aggregation, cache, sizing |
| `crates/smolrunners` | Rust | `serve`, `host` and the operator CLI over a `std::net` HTTP API |
| `crates/launcher` | Rust | narrow privileged per-job UID, cgroup and network setup |
| `crates/collector` | Rust | static musl 1 Hz guest sampler (`std` and `rustix` only) |
| `adapter/` | Go | scale-set `listener.Client`, framed stdio protocol, fake broker |
| `images/` | recipes | Debian 13 and Alpine 3.24 guests with the pinned runner, Docker and collector |
| `web/` | Solid + Vite | operator UI over the same API as the CLI |

The substrate is the pinned SmolVM release. The fleet scale-set adapter and the guest
`Runner.Listener` are separate components; the adapter removes no .NET component from the guest.

GitHub owns triggers, matrices, approvals, concurrency, assignment, secrets, tokens, OIDC and
results. The guest executor owns step semantics. smolrunners owns capacity, profiles, VM lifetime,
images, measurements, history and operator access.

The nested development host is one `sr-dev` machine under smolvm on the Mac. Multi-host work uses
`sr-coord`, `sr-host-a` and `sr-host-b` on one `sr-net` network, within 12 vCPUs and 24 GiB.

## Ownership record

Each host job writes one marker under the host state directory before creating a VM. It carries
the machine and cgroup names, owner PID and process start time, allocated VM UID, image, network
policy, reservation and execution IDs, storage budget, and the effective cpu/memory/pids limits.
The boundary report is evaluated from that marker, so a later change to host configuration cannot
rewrite what a running execution was checked against. The reaper uses the recorded start time to
avoid treating a reused PID as the original owner.

## Connected coordinator

`smolrunners serve --pilot CONFIG.toml` is the running product. It supervises the Go adapter
(`docs/adapter-protocol.md`), records every adapter event in the lifecycle store before
acknowledging it, and keeps capacity in the ledger. Its executions live in the same `State` the
HTTP API serves, so `/api/v1/jobs`, the CLI and the UI read the records and samples the coordinator
writes.

An acknowledgement never waits for a VM. On a `capacity` event the coordinator reserves capacity,
writes the execution record with its runner name and effective limits, then acks. A worker thread
per execution requests a fresh JIT config, writes it to a root-only 0600 file and starts a detached
`host provision`. The host agent deletes the file on read, delivers the config into the guest over
stdin, runs the entrypoint (`run-job`, which starts the official runner) and tears the guest down
when the runner exits or a stop is requested. The worker tails the agent's log and samples the
guest through `host exec`. After `REMOVED` it releases the reservation, deregisters the runner and
records the result. GitHub's `job_started` and `job_completed` join the execution by runner name.

A restarted coordinator re-reads each live execution's host log. Running guests keep going under
their host agents, and sampling resumes. Stops (operator cancel, idle timeout, scale-down,
completion grace) are decided by `coordinator/plan.rs` and sent as host stop requests.

The profile in the pilot config is the single source for the guest shape, the host
`cpu.max`/`memory.max` (`HostAllowance`: shape plus VMM overhead, the same cost the ledger
reserves), and the limits frozen on each execution record.

Host state is private. Markers, control files, runner logs and UID claims are root-only (0700). Each
VM's SmolVM state, `HOME` and `TMPDIR` live under `vms/<machine>`, owned by that VM's UID with mode
0700.

The transport deviation above is deliberate: the pinned `h12tiny` crates compiled empty without
protocol features, and enabling `http1` exceeded the cold-build bound. The API uses `std::net`
directly. With `--tls-ca`, `--tls-cert` and `--tls-key`, the listener wraps those streams in
rustls and requires a verified client certificate; peer identity comes from the certificate's
common name, never from a request body. Without those flags the listener stays plain HTTP on
loopback and reports `transport: plain-http` with `mtls: false`. The fleet gate runs the TLS
path; local operator commands and the connected coordinator use plain HTTP.
