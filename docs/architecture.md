# Architecture

smolrunners is one coordinator with SQLite and one or more host agents that may share a machine.

| Component | Language | Responsibility |
| --- | --- | --- |
| `crates/core` | Rust | domain, store, ledger, lifecycle, adapter protocol codec, telemetry aggregation, cache, sizing |
| `crates/smolrunners` | Rust | `serve`, `host` and the operator CLI over an h12tiny HTTP API |
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
