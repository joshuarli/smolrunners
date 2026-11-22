# Compatibility

Debian stable is the default image family; Alpine 3.24 is the required second family. An Alpine
container inside a Debian VM is interim and is never counted as native Alpine. Linux hosts and
Linux runners only, with the architecture explicit everywhere.

Locally, advertised architectures follow the nested arm64 host. `x64` labels stay unadvertised
until NE2 passes on native x86_64 hardware. Native `smol-alpine…` labels stay unadvertised until
NE1 passes the native Alpine corpus.

The transparent Actions cache v2 adapter is out of scope because evaluating it needs live GitHub.
Normal GitHub cache and artifact endpoints remain.

## Debian 13 versus `ubuntu-latest`

There is no measured Debian-versus-`ubuntu-latest` corpus in this campaign. O1 forbids live
GitHub jobs, so the comparison is a prerequisite for owner qualification, not a local pass. The
gaps already established from pinned inputs and local evidence are:

| Gap | Local evidence | Remedy | Status |
| --- | --- | --- | --- |
| The pinned official runner targets .NET 8, retired 2026-11-10. | `docs/upstream-evidence.md` records the target for `runner-linux-arm64`; upstream PR #4757 moves it to .NET 10 and is open. | Retarget and qualify a pinned .NET 10 runner. | Local `net10.0` `linux-musl-arm64` runner runs natively on Alpine (L2); live corpus is NE1. |
| Native Alpine runner needs musl C++ runtime, ICU and musl Node inputs. | `Native Alpine executor (D13)` below. | Parent pins the Alpine package closure and a musl Node build, then L2 retries. | Pinned and installed; the local native smoke passes (L2). |
| Debian-versus-Ubuntu system package and toolchain differences in real workflows. | No local corpus; the fixture workloads run on pinned Debian 13. | Owner runs the live corpus and records per-workflow deltas. | NE1. |
| x86_64 image and runner behavior. | Pinned x86_64 inputs are fetched but unqualified locally. | Native x86_64 host qualification. | NE2. |
| Full Docker networking and overlayfs parity in the Alpine container path. | `image_alpine.sh` proves boot, collection and a simple container with `vfs` and iptables disabled. | Separate DinD/network parity evidence. | Local gap. |

Unsupported features are the D12 deferrals in `smolrunners-campaign.md`: the transparent cache v2
adapter, a lean replacement executor, live resizing, RAM forks, cross-host cache replication,
remote builders, lazy image formats, the enhanced checkout action, on-demand code-level profiling,
and OTLP/Prometheus export.

The official runner v2.338.2 targets .NET 8, which went out of support on 2026-11-10. The .NET 10
change is upstream PR #4757 and is still open; the bundled target is recorded in job history.

## Native Alpine executor (D13): local smoke passes, corpus remains NE1

The first D13 spike published a musl-linked `Runner.Listener` but it exited 127 for lack of
`libstdc++.so.6`, `libgcc_s.so.1` and ICU, and no pinned musl Node was available. The parent
then pinned the complete Alpine closure: `alpine-libgcc-aarch64`,
`alpine-libstdcxx-aarch64`, `alpine-icu-libs-aarch64`, `alpine-icu-data-en-aarch64`, and the
Alpine `nodejs` 24.22.3 apk plus its 13 shared-library and certificate dependencies; all are
justified in `docs/upstream-evidence.md`. nodejs.org publishes no `linux-arm64-musl` build
for the pinned Node 22.23.5, so the musl Node comes from Alpine's package. The retry adds one
further pin, `runner-src-musl-arm64` (SHA-256
`a0fa836ec067173a6de1d5afc1ae7c9f5e23cd576884385d6d020c7a529c6e5e`, 16,015,837 B), for the
runner source at the v2.338.2 commit `d1eb12d4b115c301201de27d667284e1a80f2ab5`.

`images/alpine324/install-runtime.sh` extracts the apks into the rootfs before the offline
inner guest boots. `images/alpine324/build-runner.sh` fetches the pinned .NET SDK 10.0.404
and pinned runner source, retargets `net8.0` to `net10.0`, adds the `linux-musl-arm64` RID,
disables `TreatWarningsAsErrors` (NU1510 on .NET 10), writes deterministic `BuildConstants`
for version 2.338.2, and publishes `Runner.Listener`, `Runner.Worker` and
`Runner.PluginHost` with `-r linux-musl-arm64 --self-contained true`. The SDK runs with
`DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1` because the minimal nested Debian build host has
no ICU; that affects only the build process, not the published apphosts or the guest.

`make test-vm T=alpine_runner` is now a real gate. On 2026-11-22 it exited 0 and showed:

- `Runner.Listener --version` prints `2.338.2` and exits 0 natively in the Alpine guest.
- `ldd` on `Runner.Listener`, `Runner.Worker`, `Runner.PluginHost` and the musl Node path
  reports `/lib/ld-musl-aarch64.so.1`, with no glibc loader or `GLIBC_` symbols.
- `Runner.Worker` reaches managed argument validation (exit 1, no loader error).
- Node `v24.22.3` from `/opt/runner/externals/node24/bin/node` executes a JS action
  entrypoint fixture and prints the expected marker.

No library remains missing. This is a packaging and runtime smoke, not native-Alpine
qualification: no runner registration, job, or JS action through a live host step was run
(O1). Native `smol-alpine…` labels stay unadvertised until NE1 passes the live Alpine corpus,
and the interim Alpine-container-under-Debian path stays on NE2. The cross-published
`linux-musl-x64` artifact is not built or shipped locally; it stays on NE2.
