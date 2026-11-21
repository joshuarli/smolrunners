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
| The pinned official runner targets .NET 8, retired 2026-11-10. | `docs/upstream-evidence.md` records the target for `runner-linux-arm64`; upstream PR #4757 moves it to .NET 10 and is open. | Retarget and qualify a pinned .NET 10 runner. | Local musl build measured under D13; native execution parked. |
| Native Alpine runner needs musl C++ runtime, ICU and musl Node inputs. | `Native Alpine executor (D13)` below. | Parent pins the Alpine package closure and a musl Node build, then L2 retries. | Parked. |
| Debian-versus-Ubuntu system package and toolchain differences in real workflows. | No local corpus; the fixture workloads run on pinned Debian 13. | Owner runs the live corpus and records per-workflow deltas. | NE1. |
| x86_64 image and runner behavior. | Pinned x86_64 inputs are fetched but unqualified locally. | Native x86_64 host qualification. | NE2. |
| Full Docker networking and overlayfs parity in the Alpine container path. | `image_alpine.sh` proves boot, collection and a simple container with `vfs` and iptables disabled. | Separate DinD/network parity evidence. | Local gap. |

Unsupported features are the D12 deferrals in `smolrunners-campaign.md`: the transparent cache v2
adapter, a lean replacement executor, live resizing, RAM forks, cross-host cache replication,
remote builders, lazy image formats, the enhanced checkout action, on-demand code-level profiling,
and OTLP/Prometheus export.

The official runner v2.338.2 targets .NET 8, which went out of support on 2026-11-10. The .NET 10
change is upstream PR #4757 and is still open; the bundled target is recorded in job history.

## Native Alpine executor (D13): parked after the L2 spike

The D13 native-executor spike was attempted and exceeded its bound; per D13 the reversal is
recorded here and native-Alpine executor completion is parked. The Alpine 3.24 image and its
static collector stay. Native `smol-alpine…` labels remain unadvertised, and the interim
Alpine-container-under-Debian path stays on NE2.

What was measured, on the nested Debian 13 arm64 host with the pinned inputs:

- The upstream source at the v2.338.2 commit `d1eb12d4b115c301201de27d667284e1a80f2ab5`,
  retargeted from `net8.0` to `net10.0` and given the `linux-musl-arm64` RID, builds with the
  pinned .NET SDK 10.0.404 (`dotnet-sdk-linux-arm64`). `dotnet publish -r linux-musl-arm64
  --self-contained true` exits 0 and produces a `Runner.Listener` apphost. Retargeting needed
  `TreatWarningsAsErrors=false` because .NET 10 raises NU1510 for packages the runner still
  references; the first publish failed on that warning before the second succeeded.
- The published apphost is genuinely musl-linked: inside a guest booted from the pinned
  Alpine 3.24.4 rootfs, `ldd` reports `/lib/ld-musl-aarch64.so.1` and
  `libc.musl-aarch64.so.1` and no glibc loader.
- It does not run natively: `Runner.Listener --version` exits 127 with
  `Error loading shared library libstdc++.so.6` and `libgcc_s.so.1`. Those musl C++ runtime
  libraries, and ICU for globalization, are absent from the pinned Alpine base rootfs. Inner
  guests have no network (`apk update` → `DNS: transient error`, no route), so `apk add
  libstdc++ libgcc icu-libs` fails (exit nonzero). Baking them in requires new Alpine `.apk`
  HTTPS downloads, which are not in `sources.lock.json`; D6 requires every image-build
  archive to be pinned there and L2 may not edit that file.
- No pinned musl Node exists. The only pinned Node inputs (`node-linux-arm64`,
  `node-linux-x64`) are the official nodejs.org glibc builds; their binary contains
  `ld-linux-aarch64.so.1` and `GLIBC_2.x` and does not execute on Alpine. Supplying
  "musl Node where host steps select it" needs either a new pinned musl Node input or
  Alpine's `nodejs` package, both unpinned inputs outside L2's authority.

The bound was 3 attempts and one working session of build effort. Two publish attempts were
used (one failed on NU1510, one succeeded) plus runtime probes; the remaining work required
unpinned inputs L2 cannot add, so the spike was stopped at the bound rather than grinding
past it. No `linux-musl-x64` artifact is shipped; it is parked with the spike.

Retry condition: the parent adds SHA-256/size pins to `sources.lock.json` for a musl Node
binary and for the musl C++ runtime and ICU libraries (or upstream produces a runner build
with no such host dependencies), then L2 rebuilds and qualifies both `linux-musl-arm64` and
the cross-published `linux-musl-x64` output.
