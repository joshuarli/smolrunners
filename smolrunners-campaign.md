# smolrunners campaign

## Status

- **Goal:** build smolrunners, a self-hosted fleet manager that runs each GitHub Actions job in a disposable SmolVM guest on owned Linux hosts, with live and historical CPU, RAM, storage and lifecycle telemetry. The goal for this campaign is every local deliverable below, accepted on the nested Linux host. Live GitHub proof and native-hardware proof are listed under **Needs environment**.
- **Last accepted revision:** `1a2fa2b` (E1).
- **Accepted:** P0-ENV, C1, A1, H1, C2, I1, C3, C4, T1, H2, T2, H3, T3, U1, D1, R1, W1, E1. **Red:** none. **Parked:** none.
- **Next slice:** `I2` is in flight; then `D2`, `L1`, `L2` as prerequisites clear, followed by P2.
- **Resume prompt:** `Execute ~/repos/smolrunners/smolrunners-campaign.md from Status`. First command: run the parent steering checkpoint, integrate any finished lane, then launch the next ready P1 lanes.

## Charter

1. Decide and keep going. Write a decision into **Decisions** only when a later slice depends on it. Log any other choice in the commit body.
2. The owner's grant is this document. Proceed without waiting for approval within **Execution boundary**. Park any item that needs authority outside it. This plan does not supply missing permission.
3. Hard stops are few, and each one parks the item rather than ending the campaign:
   - an irreversible action, or an action outside this repository and its `.work/`;
   - any GitHub write, or any use of GitHub credentials beyond `gh` read-only queries;
   - a boundary this document forbids;
   - a loop that cannot check anything.

   Pushing is always forbidden.
4. Fix egregious problems you meet along the way in their own commit. They are exactly the thresholds in **Baseline → Egregious thresholds**. Record anything smaller under the item and leave it.
5. Parity and completeness come before optimization. Performance numbers are measured and logged until phase P3. Numbers from the nested host never gate.
6. Run slices from the disjoint ownership sets in **Sequence** in parallel, each in a worktree at `.work/wt/<item-id>`. Run everything else serially. Delegate through `orchestrate` and follow its [worktree lifecycle](/Users/josh/.claude/skills/orchestrate/SKILL.md#worktree-lifecycle) and [lane contract](/Users/josh/.claude/skills/orchestrate/references/campaigns.md#lane-contract). Lanes never edit `Cargo.toml` (root), `Cargo.lock`, `go.sum`, `sources.lock.json` or this document. They request those changes in their report and the parent applies them.
7. After each accepted slice, on resume, and after any slice fails twice, run the [parent steering checkpoint](/Users/josh/.claude/skills/orchestrate/references/campaigns.md#parent-steering-checkpoint). Then rewrite **Status** in place. Never append logs, checkpoints or handoffs. Git history is the log.
8. When a check fails, establish the cause before retrying (`root-cause`). Never repeat an identical failing command. Each item gets three attempts. After the third, commit the work to branch `park/<item-id>`, record the cause and retry condition in **Status**, and take the next ready item.
9. Every commit is a Conventional Commit whose scope is the workstream: `dev`, `core`, `adapter`, `host`, `image`, `telemetry`, `api`, `web`, `cache`, `alpine` or `fleet`. The item ID goes in the body. Code comments never cite this document or its item IDs.
10. Follow the repository `AGENTS.md` that `P0-ENV` creates, the owner's working contract, and the scratch rule: scratch goes in `.work/tmp`, with `TMPDIR` set to it. Never use `/tmp` or the system `$TMPDIR`.
11. Never count a mock, a fake broker, a container, a macOS run or a nested-host timing as live-GitHub or native-hardware proof. Local fakes prove local logic. The matching live proof stays on **Needs environment** with a runnable command.

## Decisions

### Owner (2026-11-21)

- **O1. No live GitHub in this campaign.** The executor makes no authenticated GitHub calls: no App, PAT, scale set, runner registration or workflow dispatch. Every gate that needs GitHub becomes a runnable `make live-*` target. Without credentials it exits 2 and names the missing input. These targets are listed in **Needs environment**. Anonymous reads of public repositories and releases, used to resolve pins, are allowed.
- **O2. x86_64 goes on Needs environment.** The local loop and all functional gates run on a nested Debian 13 arm64 KVM host under smolvm on this M5 Mac. Native x86_64 qualification and native-performance measurement run on the owner's hosts (threadripper, lisa) through **Needs environment**. The executor does not connect to them.
- **O3. The repository lives at `~/repos/smolrunners`.** It was moved from `~/d/smolrunners` during preflight, before any commit existed.
- **O4. Development-environment inputs come from `laputa-systems/buildlib`.** Pinned downloads are declared in `sources.lock.json` under the [build repository contract](/Users/josh/repos/laputa-systems/stuff/docs/build-repos.md#pinned-inputs). They are fetched and verified with `buildlib.locks.fetch_input` and run through the vendored `bootstrap/buildlib-run` shim.

### Prescribed

Each prescription carries its reason and what would reverse it.

- **D1. Pinned upstream versions.** The executor reverifies these pins in `P0-ENV`. It records exact checksums, commits and digests in `sources.lock.json` and `docs/upstream-evidence.md`, and does not float them afterwards.

  | Input | Pin |
  | --- | --- |
  | SmolVM | v1.36.0 release bundles. SHA-256 values come from the release's `checksums.sha256`: darwin-arm64 `46b953581d9f08b9e3f6629075a053aba7aa902fb76940c1ef0ef7f58166aae4` (45555824 B), linux-arm64 `6af238f7c91149d4661f64f01f160431595b691f07465e650b09fa0d7b9bff1e` (52348267 B), linux-x86_64 `ecc9ca4c71e9e8c5a8e7d24484692460c3cb54fd757fb7e80d71e79f56e00e5a` (54950302 B). |
  | Runner | `actions/runner` v2.338.2, linux-arm64 and linux-x64. SHA-256 values come from the release body. |
  | Scale-set SDK | `actions/scaleset` at `716908e507993947ada4273aecf2361c02e5836c`. Its `go.mod` requires Go 1.27.3. |
  | Go | 1.27.3 toolchain archives. |
  | Base images | Debian 13 (13.8 or later) and Alpine 3.24 (3.24.4 or later) OCI images, pinned by digest. |
  | Docker | Static binaries for Docker Engine 29.x, the bundled containerd, and Buildx, from `download.docker.com/linux/static/stable/<arch>/` and GitHub releases. |
  | Rust | Stable 1.99.1, pinned in `rust-toolchain.toml`. |
  | smolith crates | `laputa-smol` and `laputa-async-*` at exact `=0.3.0`, and `h12tiny-*` at exact `=0.9.0`, from the `mycrates` registry configured in `.cargo/config.toml`, as in `laputa-systems/xsh`. |

  *Reason:* the draft's anchors were verified on 2026-11-21. The v1.36.0 assets and checksums exist, the runner v2.338.2 linux-arm64 and linux-x64 assets exist, the scale-set commit exists, issue #133 is still open, and local Go 1.27.3 and Rust stable 1.99.1 are installed. *Reverse:* a pinned version that fails its qualification. Requalify the next release and record why.
- **D2. Use the upstream SmolVM release, not Niceforge's patched bundle.** No Niceforge patch is carried at the start. Patch `0018-smolvm-linux-pack-storage-seeds` is evaluated only in `X3-seed`. *Reason:* the release boots nested guests here (see **Baseline**), and the campaign needs no checkpoint or capture features. *Reverse:* a required local deliverable cannot pass without a patch. In that case, carry that one patch as a `laputa-systems/smolvm` fork commit, following the third-party rule in `~/.agents/instructions/owned-tools.md`.
- **D3. Install the SmolVM bundle readable by the per-VM UID.** After extracting a bundle and before its first boot, `scripts/install-runtime` runs `chmod -R a+rX` over the bundle directory, then verifies that `storage-template.ext4` and `overlay-template.ext4` have mode `0644` or wider. *Reason:* preflight reproduced the failure. A root-owned `0600` template makes the per-VM UID's open fail with `EACCES`, and libkrun then reports `krun_start_enter returned -22 (EINVAL)`. Applying `chmod a+rX` fixed it. File the `Upstream draft:` issue in the `laputa-systems/smolvm` fork, as the third-party rule requires. *Reverse:* an upstream release that installs readable templates.
- **D4. Repository layout and ownership.** Crates live in `crates/`:

  | Path | Contents |
  | --- | --- |
  | `crates/core` | The domain, the store, the ledger, the lifecycle, the adapter protocol codec, and telemetry aggregation. Pure logic and SQLite. |
  | `crates/smolrunners` | The single binary, with subcommands `serve`, `host`, and the CLI. |
  | `crates/launcher` | The narrow privileged launcher. |
  | `crates/collector` | The guest collector and bootstrap: a static musl binary that uses only `std` and `rustix`. |

  The remaining paths:

  | Path | Contents |
  | --- | --- |
  | `adapter/` | The Go scale-set adapter, as its own module. |
  | `images/` | The image recipes and manifests. |
  | `web/` | The Solid UI. |
  | `tests/vm/` | Nested-host acceptance scripts. |
  | `tests/fixtures/` | Fixture workloads and the fixture executor. |
  | `scripts/` | `setup-dev`, `fetch-inputs.py`, `install-runtime`, `linux-host.sh`. |
  | `docs/` | Project documentation. |

  *Reason:* the draft asks for a split by responsibility without many tiny packages, and the privileged code needs its own minimal binary. *Reverse:* a measured compile-time or ownership conflict.
- **D5. Dependencies.** Each language has an allowed set.

  | Language | Allowed dependencies |
  | --- | --- |
  | Rust | smolith crates (D1); `rusqlite` with `bundled`, so SQLite is at least 3.51.3 everywhere, and the store asserts `sqlite_version() >= 3.51.3` at open; `serde`, `serde_json` and `toml`; `lexopt` and `rustix` from the `laputa-systems` forks in `mycrates`; `sha2`. |
  | Go | `actions/scaleset` at the D1 commit and its own `go.sum`. |
  | Python | The standard library and `laputa-buildlib==0.2.0` through the shim. |
  | Web | Solid, Vite and one charting library. |

  Tokio is forbidden in `crates/`. Any other dependency is delegated under four bounds: an MIT, Apache-2.0, BSD or MPL-2.0 license; a release within the last 12 months; at most 15 s added to a cold `cargo build --release`; and at most 1 MiB added to the release `smolrunners` binary. Measure both numbers and put them in the commit body. Remove any dependency that becomes unused. *Reason:* the owner's contract and the draft's runtime preference. *Reverse:* a slice that cannot meet its acceptance within these bounds. Park it and name the dependency.
- **D6. buildlib scope.** `sources.lock.json` pins every HTTPS archive the development environment or image build downloads: SmolVM bundles, runner tarballs, Go, Docker static binaries, Node for `web/`, and the .NET SDK for the Alpine spike. Each entry has `name`, `version`, `url` or `urls`, `sha256`, `size`, `target` (a build-contract triple), `license` and `purpose`. OCI base images are pinned by digest in `images/*/image.toml` and are not fetched through buildlib, because a digest reference verifies itself. The project has no `pyproject.toml`: the shim's `bootstrap/python.lock` is the only buildlib pin, vendored unchanged from buildlib `30ad4a0` (release 0.2.0). `scripts/setup-dev` is idempotent and installs nothing system-wide. `docs/development.md` lists each tool and the slice that consumes it. *Reason:* the build-repos contract's shared-tooling policy, and the owner's request. *Reverse:* the buildlib contract changes; follow its new revision.
- **D7. Local GitHub boundary.** The adapter implements the SDK's `listener.Client` interface twice:
  - the real SDK client, used only by `make live-*`;
  - `adapter/internal/fakebroker`, a scripted test double driven by JSON scenarios in `adapter/testdata/scenarios/`. Each scenario names the SDK behavior it encodes: statistics-only first message, nil callback, `JobAvailable` → `AcquireJobs`, direct `JobAssigned`, redelivery, a 120-message burst against the 50-message truncation, a zero-capacity profile, an expired assignment, and a session-refresh failure.

  The guest's executor is configured per image. Product images run the official runner. The fixture image `images/debian-fixture` runs `tests/fixtures/executor`, which follows a script and also calls the job hooks. *Reason:* O1. *Reverse:* none within this campaign.
- **D8. Dev topology.** One nested Linux host is enough for single-host work. Multi-host slices start up to three nested machines (`sr-coord`, `sr-host-a`, `sr-host-b`) on one smolvm `--network sr-net`, because a guest cannot reach listeners on the Mac. The coordinator runs in `sr-coord`. Nested machines together use at most 12 vCPUs and 24 GiB of RAM. *Reason:* the Mac has 18 CPUs and 48 GiB, and the Niceforge note that guests cannot reach the Mac. *Reverse:* none.
- **D9. Adapter protocol.** The adapter speaks framed stdio: a length prefix, then JSON, at most 1 MiB per frame, with `schema_version: 1`. Every event carries an ID. The Go `Scale` callback blocks until the coordinator acknowledges that it has durably recorded the event and its admission decision. For accepted `JobAvailable` offers, the callback also waits for the `AcquireJobs` outcome to be persisted before it returns nil. The coordinator supervises the adapter and restarts it with jittered bounded backoff (1 s up to 60 s). *Reason:* the SDK deletes a message only after `Scale` returns nil (`listener/listener.go`). *Reverse:* an SDK contract change, detected by `adapter` tests at a new pin.
- **D10. Durability.** SQLite runs in WAL mode with `synchronous=FULL` for lifecycle, inbox and reservation writes. Effective pragmas are checked on every connection open, and a mismatch is a hard error. Telemetry is written in separate batched transactions, at most one per second per host. Backups use the backup API. A restored database admits no work until every host has reconciled. *Reason:* the draft, and SQLite's documented `NORMAL` loss window. *Reverse:* none.
- **D11. Labels.** A resource label has the form `smol-<distro><release>-<arch>-<vcpu>c<gib>g`, for example `smol-debian13-arm64-4c8g`. Its meaning is immutable. An alias label has the form `smol-profile-<name>` and resolves to a promoted profile revision. Each execution records the revision it resolved. Advertised architectures follow the host: `arm64` locally, `x64` once **Needs environment** NE2 passes. *Reason:* the draft. *Reverse:* none.
- **D12. Out of scope for this campaign**, with reasons:
  - The transparent Actions cache v2 adapter, because evaluating it needs live GitHub. Normal GitHub cache and artifact endpoints stay. `docs/compatibility.md` states that.
  - A lean replacement executor (Canopy- or Chimera-style) and live resizing, because the differential oracle needs live GitHub.
  - RAM forks.
  - Cross-host cache replication.
  - Remote builders.
  - Lazy image formats.
  - The enhanced checkout action and Git mirrors. Plain `actions/checkout` stays.
  - On-demand code-level profiling.
  - OTLP and Prometheus export: optional in the draft; a later campaign adds them if the owner asks.

  *Reverse:* an owner request.
- **D13. Alpine.**
  - Native-executor spike: a packaging fork of the official runner for `linux-musl-arm64`, built locally with .NET 10. The managed `linux-musl-x64` output is cross-published as an artifact and goes to NE2.
  - Interim Alpine-container-under-Debian path: an x64-only path in upstream, so it goes to NE2.
  - Native `smol-alpine…` labels stay unadvertised until NE1 passes their corpus.

  *Reason:* upstream's Alpine container path rejects non-x64, and O1 applies. *Reverse:* the spike exceeds its bound (`L2`). Then record the measured blocker in `docs/compatibility.md`, keep the Alpine image and collector, and mark native-Alpine executor completion as parked.

### Delegated

The executor decides each of these inside its bound and logs the choice in the commit body.

- Lifecycle enum names. The bound: every state in **Reference → Lifecycle** must be distinguishable.
- Telemetry storage layout. The bound: SQLite tables until a measured write rate above 2,000 rows/s or a 1 GiB history per 100 jobs. Only then move to chunk files.
- UI layout.
- The preset catalog contents. The bound: it includes at least these vCPU/GiB shapes, filtered to what the enrolled hosts can provide, plus validated custom shapes:
  - small: 1/2, 2/4;
  - balanced: 4/8, 8/16, 16/32, 32/64;
  - compute-heavy: 8/8, 16/16;
  - memory-heavy: 2/16, 4/32, 8/64.
- Error-code names. The bound: stable codes, JSON on stderr, and exit codes as `design-cli` describes.
- The fair-offer algorithm. The bound: the ledger never promises one resource twice, a feasible profile waits at most `max_wait` (default 10 min) behind smaller ones, and idle warm capacity is the only thing reclaimed.

## Baseline

The repository is empty, so nothing is red.

These timings were measured on 2026-11-21 on this M5 Mac (18 CPUs, 48 GiB, macOS 27.2).

| Step | Wall |
| --- | --- |
| `buildlib-run` shim, cold (python-build branch selected) | 4.3 s |
| `fetch_input` of the 45.6 MB smolvm darwin bundle, verified | 1.8 s |
| Nested Debian 13 arm64 host start under smolvm 1.36.0 `--nested` (after the image pull; `/dev/kvm` present; cgroup v2 controllers `cpuset cpu io memory pids`) | 7.5 s |
| Linux smolvm 1.36.0 guest inside the nested host: `machine run -I alpine:3.24`, cold, including the pull | 11.5 s |
| The same guest, warm | 3.7–4.1 s |

**Known pitfall:** read D3 before the first nested boot.

**Egregious thresholds.** Fix the cause in its own commit when any of these is crossed:

- `make test-fast` warm takes more than 120 s;
- one unit test takes more than 5 s;
- an item's acceptance command takes more than 10 min;
- a warm inner-guest `machine run … true` takes more than 15 s;
- an incremental `cargo build` after a one-line change takes more than 60 s;
- any owned VM, process, disk or port survives a passing test.

## Sequence

Phases run in order: P0 ground, P1 parity and completeness, P2 hardening, P3 optimization, P4 close. An item's prerequisites are written on the item. These ownership sets are disjoint and may run in parallel once their prerequisites are accepted:

| Set | Owns |
| --- | --- |
| **α core** | `crates/core/src/{store,ledger,lifecycle,proto}/` |
| **β adapter** | `adapter/` |
| **γ host** | `crates/smolrunners/src/host/`, `crates/launcher/`, `tests/vm/host_*` |
| **δ image** | `images/`, `tests/vm/image_*`, `tests/fixtures/` |
| **ε telemetry** | `crates/collector/`, `crates/core/src/telemetry/`, `tests/vm/telemetry_*` |
| **ζ api** | `crates/smolrunners/src/{api,cli}/`, `tests/cli/` |
| **η web** | `web/` |
| **θ cache** | `crates/core/src/cache/`, `crates/smolrunners/src/host/cache/` |
| **λ alpine** | `images/alpine*/`, `alpine/` |

Exceptions:

- θ shares `host/` with γ, so θ items run only while no γ item is open.
- λ shares `images/` with δ, so λ items run only while no δ item is open.
- Store migrations are owned by α. Other sets add tables only by requesting a migration from α.

The **Acceptance** column gives the command that ends the item. The narrowest affected suite also runs on the integrated tree. `make test-fast` runs `cargo test --workspace` and `go test ./...` in `adapter/`, with no VM. `make test-vm T=<name>` syncs the tree into the nested host and runs `tests/vm/<name>.sh` there.

### P0 — ground

| ID | Owns | Prereq | Work | Acceptance |
| --- | --- | --- | --- | --- |
| `P0-ENV` | `Makefile`, `sources.lock.json`, `bootstrap/`, `scripts/`, `.cargo/config.toml`, `rust-toolchain.toml`, root `Cargo.toml`, `adapter/go.mod`, `.gitignore`, `AGENTS.md`, `docs/development.md`, `docs/upstream-evidence.md`, `docs/architecture.md`, `docs/compatibility.md` | — | The workspace skeleton (D4) with empty crates that compile. The pinned inputs (D1, D6). `make setup`, `make fetch`, `make check`, `make test-fast` and `make test-vm`. `scripts/linux-host.sh up/sync/exec/status/down/destroy` for `sr-dev`, with state under `.work/linux-host`. `scripts/install-runtime` (D3). `scripts/x86-64-host.sh`, adapted from Niceforge's: it runs the pinned Debian 13 amd64 container with `/dev/kvm` for NE2 and is never run locally. Consumer boundary tests for `fetch-inputs.py`: wrong digest, wrong size, wrong target, truncated download, and cache reuse. `docs/upstream-evidence.md` has one row per input: SHA, date, source path, finding, evidence class, decision, and revalidation trigger. `AGENTS.md` has the commands and a verification table with warm costs. | `make check` (fetch-script tests, `cargo build`, `go build`) and `make test-vm T=smoke`. `smoke` boots a digest-pinned Alpine guest and a Debian guest through the pinned Linux runtime and leaves no `sr-*` resources. Record the timings in `AGENTS.md`. |

### P1 — parity and completeness

The oracles: the official runner and the SDK listener contract for runtime semantics, and the draft requirements in **Reference** for everything else. Each behavior row in **Acceptance** maps to a named test.

| ID | Set | Prereq | Work | Acceptance |
| --- | --- | --- | --- | --- |
| `C1` domain | α | P0-ENV | Newtypes for every GitHub identifier domain: protocol job ID string, REST job ID, runner request ID, workflow run ID, run attempt, `GITHUB_JOB` key, runner ID and name. Profile and immutable profile revision. Shape and preset catalog. Label grammar (D11). Validation with specific errors. vCPU, logical CPU, core and SMT distinctions. | `cargo test -p smolrunners-core domain` |
| `C2` store | α | C1 | Schema v1 and migrations for every entity in **Reference → Persistent state**. D10 pragmas checked per connection. SQLite version assertion. Explicit errors for `SQLITE_BUSY` and disk-full. A read-transaction bound. Backups through the backup API. A separate telemetry batch path. | `cargo test -p smolrunners-core store`, including a test that opens a `synchronous=NORMAL` connection and gets the error |
| `A1` adapter | β | C1 | The D9 protocol, with the Go side in `adapter/` and the Rust codec in `crates/core/src/proto/`. Both sides commit the same golden frames. The SDK `Scaler` uses `TotalAssignedJobs`. Acquisition is driven by coordinator decisions. Nil and statistics-only callbacks are handled. GitHub App key rotation, installation revocation, retry guidance and jittered session recovery are exercised, and a session failure never deletes a running VM. The fakebroker and every D7 scenario are included. | `cd adapter && go test ./...` and `cargo test -p smolrunners-core proto` |
| `C3` ledger | α | C2 | One reservation ledger. Host incarnation fencing. No RAM overcommit. Host headroom and measured VMM overhead fields. Capacity offers per profile scale set, backed by the global ledger. Aging and a reservation barrier. Specific unschedulable reasons. Cache locality is a placement preference capped at `max_wait`. Heartbeat expiry never frees capacity. | `cargo test -p smolrunners-core ledger`, including fixed-seed property tests: no double promise, bounded wait for a large profile behind 1,000 small arrivals, and an impossible shape not blocking feasible ones |
| `C4` lifecycle | α | C3, A1 | The durable inbox and state machine (**Reference → Lifecycle**). Idempotent redelivery. Acquisition persisted before ack. Uncertain outcomes are kept for reconciliation and never replayed. A crash-injection harness that kills the coordinator before and after each external effect. | `cargo test -p smolrunners-core lifecycle`, plus `go test ./... -run Scenario` driving the real coordinator binary through the fakebroker, with kill points |
| `H1` supervisor | γ | P0-ENV | A host agent that supervises the pinned smolvm CLI with `--error-format json`. An ownership marker is written before every effect. Exact teardown. An orphan reaper runs on restart. Environment is sanitized and policy variables are stripped. The privileged launcher sets up the per-job UID, cgroup and network. The bundle stays separate from the coordinator. | `make test-vm T=host_lifecycle`: two concurrent guests and clean teardown; a kill -9 of the agent mid-boot, then restart reaps exactly the owned resources; zero `sr-*` leftovers |
| `H2` boundary | γ | H1 | Before readiness, verify and record the VMM PID and incarnation, the UID, cgroup membership, the effective `cpu.max`, `memory.max` and `pids.max`, evidence that Landlock and seccomp were applied, and the private-range network policy for IPv4, IPv6 and DNS. A host that fails is ineligible, with a specific reason. A check that is unavailable in the nested host is recorded as a precise missing prerequisite. `smolrunners doctor` runs the same checks plus KVM access, architecture, cgroup delegation, storage and filesystem features (reflink, ID-mapping), clocks, FD limits and runtime compatibility. | `make test-vm T=host_boundary`: a positive case, plus negative cases (limit stripped, policy variable injected, host loopback reachable) that make the host ineligible; `smolrunners doctor --json` reports the same verdicts |
| `H3` storage | γ | H1 | Raw disk-backed writable state per job. Guest ext4 for the workspace, `/var/lib/docker` and `/var/lib/containerd`. Disk formats, backing chains and copy or allocation methods are recorded. Byte and inode admission, including sparse and reflink accounting. | `make test-vm T=host_storage`: two jobs never share writable state; admission refuses an over-budget job with a reason; the recorded format and backing chain match what smolvm reports and `stat` allocation shows |
| `I1` debian image | δ | P0-ENV | The `images/debian13` recipe: digest base, snapshot-pinned apt, the pinned Docker, containerd and Buildx static set, the runner, the collector, the bootstrap, and an image manifest of every component version. `images/debian-fixture` with `tests/fixtures/executor`. | `make test-vm T=image_debian`: boots; `bin/Runner.Listener --version` prints 2.338.2; the runner's bundled Node versions execute; the manifest matches the installed packages |
| `I2` docker | δ | I1, H3 | The local Docker corpus, items 1–2 and 6–8 of **Reference → Docker corpus**: an authorized local `registry:2` push and pull, BuildKit and Buildx cache, Compose, Testcontainers, privileged `docker:dind` with an inner daemon, cancellation, daemon failure, disk full, and two concurrent guests binding identical ports that are unreachable from the host and LAN. Data roots are verified on ext4. | `make test-vm T=image_docker` |
| `T1` collector | ε | P0-ENV | A 1 Hz guest collector from boot to finalization, reading procfs and cgroupfs. Each sample has monotonic and wall timestamps, a sequence number, the boot ID, and a collector generation. Raw cumulative counters, PSI totals, `memory.peak`, `memory.events`, swap, disk and network. A deliberate Docker cgroup parent. Bounded batches over the persistent guest-agent channel smolvm v1.36.0 provides (vsock or virtio-serial, delegated), never a process spawned per sample. A bounded host spool. Separate scopes for guest, workload and host. | `make test-vm T=telemetry_collect`: the fixture executor runs CPU, allocation and I/O phases plus a service container; series show each phase with correct units; service memory is counted; the collector never stops at a sample count |
| `T2` history | ε | T1, C2 | Batched ingestion. Rates computed from capture time. No join across resets or incarnations. Gaps are flagged, never zeroed. Peak-preserving buckets: count, coverage, min, max, sum, first and last, with gap flags. Retention and compaction. A versioned sample schema, where a definition change starts a new cohort. | `cargo test -p smolrunners-core telemetry` and `make test-vm T=telemetry_history` (a coordinator restart and a link outage mid-job relabel nothing) |
| `T3` hooks | ε | T1, I1 | `ACTIONS_RUNNER_HOOK_JOB_STARTED` and `ACTIONS_RUNNER_HOOK_JOB_COMPLETED`, as bounded local notifications with a 2 s timeout. The VM is never destroyed in the hook. Identity-pending intervals. Metadata attaches later without moving samples. A workflow-jobs REST client attaches step start and end times after completion, tested on recorded response fixtures; live use is NE1. | `make test-vm T=telemetry_hooks`, using the fixture executor's hook calls |
| `U1` api+cli | ζ | C4, T2 | An h12tiny HTTP API and the CLI over it. The full command list in **Reference → CLI** is in scope. Commands that need GitHub (`github connect` and the enrichment paths) are implemented against the D7 fakes, and their live use is NE1. JSON output uses a versioned schema with units and explicit missing values. `jobs logs --follow` reports "unavailable; see GitHub" until NE1 qualifies a relay. | `cargo test -p smolrunners --test cli` (golden JSON schema per command) and `make test-vm T=cli_e2e`: `profiles apply`, fakebroker demand, a VM job through the fixture executor, then `jobs metrics --follow --json` and `jobs report --format csv` |
| `E1` enrollment | ζ | U1, H1 | `host invite`/`join` with a single-use invite and a local key. Fingerprint check. mTLS. Revocation. Revisioned `drain`/`resume`. Capability matching. | `make test-vm T=fleet_two_hosts` on the D8 topology: jobs go only to matching hosts; drain finishes current work and admits nothing new; a revoked host gets no work; a killed host's capacity is never freed |
| `D1` content | θ | H3, C2 | A host-owned immutable content store. Private job scratch. Action archive cache through the runner's own mechanism. A toolchain cache baseline with a private view. Private cache generations with lineage, publication after quiescence, a GC pin, quotas, and a clean fallback with a visible reason. | `make test-vm T=cache_generations`: concurrent jobs never share mutable state; an interrupted publish leaves the prior generation valid; active generations survive GC; repositories and trust tiers cannot broaden authority; outcome fields appear in job history |
| `D2` docker state | θ | D1, I2 | Docker and BuildKit reuse with consistent publication: daemon drained, flushed and cleanly unmounted, containers, volumes and credentials removed, identities recorded. Transfer, unpack and build times are measured separately. | `make test-vm T=cache_docker`: no prior containers, services, volumes or credentials appear after reuse; cold and warm outputs are identical |
| `R1` sizing | ζ | T2, U1 | Comparable-run grouping. Recommendations that state the observation, a nearby profile, confidence and what would falsify them. Censored hard-limit and OOM runs. Separate verdict classes. A reproducible paired-trial definition. | `cargo test -p smolrunners-core sizing` on seeded histories (8-core burst at 25% mean → "trial", not "halve"; service-cap OOM ≠ VM shortage) and `make test-vm T=sizing_trial` (the fixture workload on 2 shapes, functional only) |
| `W1` web | η | U1 | Hosts and capacity, profiles, jobs with queue reasons, and a job detail page: live CPU and RAM against limits, I/O and network, host pressure, lifecycle and step overlays, and a GitHub link. Shares observation logic with the CLI through the API. | `make web-test`, then `agent-browser` against `make test-vm T=cli_e2e`'s live coordinator: the job page shows the same peak and limit values as `jobs inspect --json` |
| `L1` alpine image | λ | I2, T1 | `images/alpine324`, booting with the collector, guest Docker, and the I2 Docker capabilities. | `make test-vm T=image_alpine` |
| `L2` musl runner | λ | L1 | The D13 spike: the official Listener and Worker for `linux-musl-arm64` on .NET 10, with musl Node where host steps select it. Cross-publish `linux-musl-x64`. **Bound:** 3 attempts and 1 working session of build effort. Then record the blocker (D13 reversal). | `make test-vm T=alpine_runner`: `Runner.Listener --version` runs natively on Alpine; host-step Node runs a JS action entrypoint; `ldd` shows no glibc |

### P2 — hardening

| ID | Set | Prereq | Work | Acceptance |
| --- | --- | --- | --- | --- |
| `F1` faults | serial | all P1 | The local fault corpus: controller crash, host-agent crash, VM crash, collector failure, message redelivery, lost ack, stale JIT material, assignment expiry, a canceled queued or running job (fakebroker), clock and counter resets, storage exhaustion, WAL disk pressure, the 120-message burst, interrupted generation publication, cache connections across VM reuse, image and runner rotation with idle-capacity drain, and a restore from backup that admits nothing until every host has reconciled. | `make test-faults`: every case reports surviving durable state and owned resources; no duplicate execution; no unbounded resources; drained idle capacity retires |
| `F2` docs | serial | F1 | `docs/architecture.md`, `docs/compatibility.md` (Debian versus `ubuntu-latest` gaps with remedies; unsupported features; Alpine status), and `docs/operations.md` (install, foreground and service-manager supervision, backup and restore, updates). The contract list in **Execution boundary** is written up. | `mdtool check docs/` |

### P3 — optimization and polish

Run P3 only after P2 is accepted. Measurements follow `benchmark-checklist` and `~/.agents/instructions/performance.md`. Results from the nested host are recorded as functional and never gate. The gating numbers are NE2 and NE5.

| ID | Prereq | Work | Acceptance |
| --- | --- | --- | --- |
| `X1-bench` | F2 | Harness `make bench`. It measures readiness for cold, prepared and warm images, separating download, unpack, boot and executor start. It also measures guest baseline memory, listener memory, per-job storage, telemetry overhead with collection on versus off, and capture-to-display latency. Each report records hardware, versions, cache state and contention. | `make bench` writes `docs/measurements/<date>-nested.md` with distributions and sample counts |
| `X2-pool` | X1 | A bounded pool of independently booted, unregistered warm VMs. Each is charged its full reservation and discarded when the image or profile generation changes. | `make test-vm T=host_pool` plus the `make bench` delta |
| `X3-seed` | X1 | Evaluate Niceforge patch 0018 (Linux storage seeds) against v1.36.0, using the fork path in D2. Adopt it only if it beats fresh raw disks on `make bench` by more than both the observed spread and 10%. | The `make bench` comparison is recorded in the measurements doc |

### P4 — close

| ID | Work | Acceptance |
| --- | --- | --- |
| `Z1` dead code | Use `dead-code-archaeology`. Prove liveness with `cargo build` under `-D dead_code -D unused`, `staticcheck -checks U1000` in `adapter/`, and a caller search for every fixture and script. Delete what is dead. The fakebroker and fixture executor stay as test infrastructure. | `make check test-fast` |
| `Z2` review | Run `ultrareview` over the campaign diff, or the `review` skill when `ultrareview` is unavailable. Fix every priority-1 and priority-2 finding. | The review rerun reports no priority-1 or priority-2 findings |
| `Z3` report | Rewrite **Status** with accepted features, the **Needs environment** commands still open, and owner follow-ups. | — |

## Acceptance

Each deliverable is local and passes on the integrated tree. Distinct success and failure conditions are kept.

| Deliverable | Command | Passes when | Fails when |
| --- | --- | --- | --- |
| Pinned dev environment | `make setup fetch check` | Every input verifies by size and SHA-256. A wrong digest, size or target aborts and leaves no partial file. | Any unpinned download, or a partial file left behind |
| Clean disposable guests | `make test-vm T=host_lifecycle` | The second job sees no files, credentials, processes, ports or writable layers from the first. | Any leftover `sr-*` resource |
| Enforced boundary before readiness | `make test-vm T=host_boundary` | The JIT handoff point is reached only after every required check has recorded positive evidence. | A warning alone lets a host become eligible |
| Exactly-once local ownership under redelivery | `make test-faults` | Redelivered, burst and lost-ack scenarios never create two VMs for one acquisition and never ack an unpersisted acquisition. | Any duplicate, or an ack before commit |
| Shared capacity | `cargo test -p smolrunners-core ledger` | No resource is promised twice. Large-profile wait stays within `max_wait`. | Starvation or a double promise |
| Docker inside the VM | `make test-vm T=image_docker` | Containers are actually created and built, including the inner `docker:dind`, and identical service ports do not conflict. | `docker info`-only evidence |
| Live and historical metrics | `make test-vm T=telemetry_history` | Phases appear. Gaps are distinct from zero. Restart and outage preserve labels. Units show cores used and percent of allocated vCPUs. | Rates computed from receipt time, or interpolation across a gap |
| Long jobs | `make test-vm T=telemetry_long` (a 95-minute fixture job, run once at the P1 milestone) | Full coverage or explicit gaps. | A sample-count cutoff |
| Sizing | `make test-vm T=sizing_trial` | The recommendation includes comparable runs and a reproducible trial. | A recommendation from mean utilization alone |
| Cache authority and lifetime | `make test-vm T=cache_generations` | All D1 conditions hold. | Cross-repository or cross-trust read or write, or GC of an active generation |
| Operator surface | `make test-vm T=cli_e2e`, plus the W1 browser check | The CLI and the web show identical facts. | Disagreement between the CLI and the web |
| Alpine image | `make test-vm T=image_alpine` | Boots with the collector and Docker. | A Debian environment under an Alpine name |
| Fault recovery | `make test-faults` | Every case passes the F1 condition. | Process restart alone is offered as proof |

The full suite (`make check test-fast` plus every `tests/vm/*.sh`) runs when P1 is accepted, when P2 is accepted, and at close.

## Needs environment

Each `make live-*` target is implemented locally and exits 2, naming the missing input, until its environment exists. The owner runs these.

| ID | Environment | Command | Required result |
| --- | --- | --- | --- |
| NE1 | A private test repository plus a GitHub App installed only on it. `SMOLRUNNERS_GITHUB_APP_FILE` names the app ID, installation ID and key file, outside the repository. | `make live-pilot` | A label-routed job's success or failure appears in GitHub. The same execution shows live CPU and RAM against limits. A second job starts clean. Nothing leaks. Run URLs and identities are recorded. |
| | | `make live-scaleset-probe` | Two scale sets, recording what GitHub does for zero-capacity demand (issue #133), direct assignments versus offers, explicit acquisition and redelivery. This selects the fair-offer policy's discovery input: webhook plus REST reconciliation, or polling. |
| | | `make live-corpus` | The Debian compatibility corpus on real GitHub: Docker corpus items 3–5 (`container:` jobs, services with health checks and DNS, Docker actions); checkout v7.0.1; JS and composite actions; shell and environment, path, state and output commands; artifacts (upload v7.0.2 and download v8.0.2, including direct upload); cache v6.1.0 with all four `cache-mode` values; OIDC; masking; post-actions; cancellation; timeouts; matrices; reusable workflows. Action commits are recorded. |
| | | `make live-faults` | Broker or session restart, HTTP/2 blackhole, a burst of more than 50 jobs, rate limits, and a 90-minute real job. |
| | | `make live-alpine` | The native Alpine corpus (D13) before any `smol-alpine…` label is advertised. |
| NE2 | Native x86_64 KVM host: threadripper or lisa | `scripts/x86-64-host.sh exec 'make check test-vm-all bench'` | The local suite on x64. The `linux-musl-x64` runner runs. The interim Alpine container path passes with NE1. Native benchmark numbers become the gating baseline. |
| NE3 | A second physical owned host | `make test-vm T=fleet_two_hosts HOSTS=<a>,<b>` | Independent failure, disconnect and revocation fencing across machines. |
| NE4 | Native arm64 hardware, not nested | `make check test-vm-all bench` | Required before arm64 profile labels are advertised for production. |
| NE5 | A quiet host where `perf-gate-ready` exits 0 | `make bench` | The draft budgets: observer overhead of at most 1% median runtime regression; p95 capture-to-display latency of at most 3 s. |
| NE6 | Owner, in `laputa-systems/stuff`, once smolrunners has a remote | Admit the repository to `fleet/catalog.toml`, and enroll it in `fleet/policy/buildlib.toml` with entrypoints `scripts/setup-dev` and `scripts/fetch-inputs.py` and `vendors_bootstrap = true`. | `repomod` `buildlib-usage` passes. |
| NE7 | A host that can lose power | Pull power during a `make test-faults` acquisition burst | No acknowledged acquisition is lost (D10). |

## Execution boundary and non-goals

**Authorized:**
- creating, editing and deleting files in this repository, and committing on `main` and lane branches;
- writing under `.work/`;
- starting and deleting smolvm machines named `sr-*`, with state under this repository's `.work/`;
- anonymous HTTPS downloads of pinned inputs, plus resolving new pins;
- reading public GitHub repositories, releases and issues;
- using the `mycrates` and stuff PyPI registries for reads;
- forking a third-party project into `laputa-systems` and filing `Upstream draft:` issues there, as the third-party rule allows (D3 only, unless another blocker appears);
- running `gh` read-only.

**Forbidden:**
- pushing;
- authenticated GitHub writes: Apps, PATs, runner registration, workflow dispatch, issues or comments outside `laputa-systems` forks;
- touching Niceforge's checkout, `.work` or machines;
- deleting any smolvm machine not named `sr-*`;
- installing system packages on the Mac;
- connecting to threadripper, lisa or any other host;
- buying or creating paid resources;
- publishing to any registry;
- editing other repositories, except the D3 fork;
- formatters, unless `AGENTS.md` adds one;
- exceeding D8's resource ceiling;
- running proactive cleanup while `agent-disk-pressure status` reports 16 GiB or more free.

**Contracts this campaign creates.** None is released, so each may change freely until close. They are listed at close for the owner:
- the CLI and API v1 JSON schemas;
- the adapter protocol v1 (D9);
- the store schema v1;
- the label grammar (D11);
- the profile TOML format;
- the guest collector sample schema v1.

**Non-goals:**
- a new Actions YAML interpreter, or translation into Niceforge workflows;
- a Niceforge runtime dependency;
- Kubernetes, an external queue, a distributed database or a TSDB;
- systemd as an application API;
- Firecracker or container backends;
- the D12 deferrals.

## Reference

### Product contract

- Linux hosts and Linux runners only, with the architecture explicit everywhere. SmolVM is the substrate.
- Debian stable is the default image family. Alpine is a required second family. An Alpine container inside a Debian VM is interim and never counts as native Alpine.
- Docker runs inside the job VM. GitHub job and service containers, Docker actions, builds, Compose and Testcontainers, and real nested DinD are all covered.
- CPU and RAM are configured independently, with a preset catalog that does not need a separate image per shape.
- Live and historical metrics are first-class.
- Niceforge is read-only prior art.
- Deployment is one coordinator with SQLite and one or more host agents, which may share a machine. Supervision works in the foreground and under common service managers.
- **Ownership boundary:**
  - GitHub owns triggers, matrices, approvals, concurrency, assignment, secrets, tokens, OIDC and results.
  - The guest executor owns step semantics.
  - smolrunners owns capacity, profiles, VM lifetime, images, measurements, history and operator access.
- The fleet scale-set listener (the Go adapter) and the guest's `Runner.Listener` are different components. The adapter removes no .NET component from the guest.

### Persistent state

These entities are kept separate:

- the GitHub installation, scope and runner group;
- the profile and its immutable revision;
- the image and runtime identity, with promotion history;
- the host: identity, incarnation, capacity, capabilities, admission state and last contact;
- the reservation and the VM instance, each identified independently of a job;
- the runner: identity, JIT lifecycle and scale-set membership;
- the workflow run, the attempt, the job, the request ID and the execution attempt;
- samples, events, timelines, log references and missing-data observations.

The host keeps durable local state, so that it can clean up its own VMs after a restart.

### Lifecycle

Executions move through these states: reserved, provisioning, guest-ready, registering, idle, busy, finalizing, terminating and removed, plus failed or uncertain observations. The normal path:

1. Record demand.
2. Reserve capacity and storage. For an accepted offer, persist the acquisition outcome before the ack.
3. Create a fresh guest.
4. Start sampling before the executor.
5. Deliver fresh JIT material once the guest is ready, kept out of logs and images.
6. Join the runner and job identities when assignment is observed.
7. Measure through every phase, including services and cleanup.
8. After the runner reports completion: flush, deregister, destroy the exact owned resources, then release capacity.

Rules that hold throughout:

- Never replay an uncertain job.
- A lost coordinator stops admission and marks telemetry stale, but does not kill a healthy job.
- Local deadlines and the reaper run independently of the control connection.
- One writer holds each host's resource authority, fenced by incarnation.
- Every effect has an ownership marker, even before an ID exists.
- Updates follow: pin, rebuild, canary, drain stale idle capacity, promote. Respect GitHub's update window and deprecation deadlines.
- GitHub App credentials stay in the coordinator and adapter. Guests receive only short-lived material.
- Record the runner's bundled .NET target. v2.338.2 targets .NET 8, which went out of support on 2026-11-10, and the .NET 10 PR #4757 is still open.

### Docker corpus

1. Shell `docker run` with registry push and pull.
2. BuildKit and Buildx with cache.
3. A GitHub `container:` job (NE1).
4. Service containers with health checks and DNS (NE1).
5. Dockerfile and image Docker actions (NE1).
6. Compose and Testcontainers.
7. A privileged `docker:dind` service with an inner daemon.
8. Cancellation, daemon failure, disk full, restart, and identical ports in concurrent jobs.

Never mount the host's Docker socket. Never route job Docker commands to SmolVM's host Docker Engine API. Prefer the virtio-net profile. Disable automatic port publication. Inspect the selected image store rather than assuming `overlay2`.

### Telemetry semantics

- **Scopes:** guest/VM (CPU per vCPU, MemTotal and MemAvailable and memory breakdown, swap, PSI, OOM, disk and network), job workload (cgroup CPU and quota, memory current, peak and events, descendants including Docker services), and physical host (VMM cgroup CPU and memory, host pressure and contention).
- CPU is shown as cores used and as percent of allocated vCPUs, with busy time, iowait, steal and throttling kept separate. The host model is recorded.
- Memory is shown as limit, available headroom, usage, cgroup charge and peak, file cache, swap and OOM, each labeled. Never label anything "minimum safe RAM".
- Guest metrics from privileged jobs are diagnostic and never grant capacity.
- Workflow logs come from GitHub. Infrastructure, guest diagnostic and workflow logs are kept separate, and nothing collects environment dumps.

### Sizing

- Group comparable runs by repository, workflow and job, matrix, image, executor and profile revision, hardware class and cache state. Include failures and OOMs.
- State the observation, the proposal, the confidence, and what would falsify it.
- Any of these blocks a downsizing conclusion: missing coverage, a changed definition, mixed hardware, or an unexplained host OOM.
- Trials are paired, interleaved and repeatable, and run only on corpora that are safe to repeat.
- Label allocated and consumed integrals separately.
- Changes never resize active jobs.

### CLI

```text
init, doctor
github connect
host invite/join
hosts list/inspect/drain/resume
profiles list/apply/inspect/recommend
images build/qualify/promote
caches list/inspect/prune --dry-run
jobs list/inspect/top/metrics --follow --json/logs --follow/explain/report --format csv/compare/debug
```

Every mutating command shows its scope and has noninteractive flags. `jobs debug` is authenticated, time-bounded and VM-scoped.

### Prior art

Patterns only. Licenses are preserved per path.

- **`actions/scaleset`** — the job demand and acquisition contract.
- **GARM** — lifecycle, orphan cleanup and drain.
- **runnerd** — sizing, admission and starvation. Its measured-overcommit default is not adopted.
- **Namespace** — the profile and report vocabulary, and cache-volume generations and lineage.
- **Falcondev** (`scope.rs`) and **Firerunner** (MIT paths only) — cache scope and generation binding.
- **Canopy and Chimera** — protocol fixtures only. Neither is a drop-in runner: their inspected sources lack the ephemeral-JIT contract, complete secret masking, renewal through finalization and job outputs.
- **Niceforge** at `8ca7fa94`. Paths verified 2026-11-21:

  | Area | Paths |
  | --- | --- |
  | Enrollment | `apps/cli/src/runner-enroll.ts`, `apps/controller/src/runner-enrollment-server.ts` |
  | Admission | `packages/database/src/runner-admission.ts` |
  | Native supervision | `apps/controller/src/native-{resource,process,scratch,lease}.ts` |
  | Sampler | `apps/controller/src/resource-sampler.ts` |
  | Nested host | `scripts/linux-host.sh` |
  | Patches | `third_party/patches/0017-smolvm-guest-usage.patch`, `third_party/patches/0018-smolvm-linux-pack-storage-seeds.patch` |

  Do not copy its 15 s cadence, its 256-sample cutoff, rates computed from controller timing, or its 500 ms lease.
