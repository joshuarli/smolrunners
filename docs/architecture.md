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

## Current integration boundary

The API's control records live in `state.json`, while sample history lives in SQLite. The core
store, ledger and lifecycle are exercised by component gates but are not yet the running
coordinator's authority, and `serve` does not supervise the adapter or execute jobs. The `INT1`
milestone connects those paths so one local job runs through the production coordinator and host
with live telemetry and recovery after a coordinator restart. Until it is accepted, component
gates are evidence that a piece works, not that the product is connected.

The transport deviation above is deliberate: the pinned `h12tiny` crates compiled empty without
protocol features, and enabling `http1` exceeded the cold-build bound. The API uses `std::net`
directly. With `--tls-ca`, `--tls-cert` and `--tls-key`, the listener wraps those streams in
rustls and requires a verified client certificate; peer identity comes from the certificate's
common name, never from a request body. Without those flags the listener stays plain HTTP on
loopback and reports `transport: plain-http` with `mtls: false`. The fleet gate runs the TLS
path; local operator commands and the connected coordinator use plain HTTP.
