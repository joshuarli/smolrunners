# Live pilot on one x86_64 host

This runbook takes one owned Debian x86_64 KVM host from nothing to running real GitHub Actions jobs
in smolrunners guests. The pilot is deliberately narrow: one host, the Debian 13 image, one fixed
profile, concurrency one, fresh guests with no shared writable caches, and an API on loopback only.

## What you need

- A Debian host on x86_64 with `/dev/kvm`, cgroup v2, root access, outbound HTTPS, and the
  packages `curl python3 tar dpkg-dev apt e2fsprogs zstd`.
- A disposable private test repository.
- A GitHub App installed on that repository with **Administration: read and write** (repository
  self-hosted runners) and **Metadata: read**. Note its client ID and installation ID, and
  download its private key.

## 1. Build the bundle on the Mac

```sh
make pilot-bundle ARCH=x86_64
```

This cross-builds static x86_64 binaries (`smolrunners`, launcher, collector, cache, Go adapter),
fetches and verifies every pinned x86_64 input, and writes
`.work/pilot/smolrunners-pilot-x86_64.tar.gz`. Copy it to the host.

## 2. Set up the host

```sh
tar -xzf smolrunners-pilot-x86_64.tar.gz
sudo sh x86_64/pilot-host-setup.sh
```

The setup script checks the architecture, KVM, cgroup v2 and tools, and re-verifies the bundled
inputs. It installs binaries to `/opt/smolrunners/bin` and the SmolVM runtime to
`/opt/smolrunners/runtime`, builds the Debian rootfs at `/var/lib/smolrunners/images/debian13`, and
creates `/var/lib/smolrunners/state` (mode 0711). It writes no credentials and does not contact
GitHub.

## 3. Write the credentials and configuration

```sh
sudo install -d -m 0700 /etc/smolrunners
sudo install -m 0600 app-private-key.pem /etc/smolrunners/app.pem
```

`/etc/smolrunners/github.json` (holds no secret; the adapter refuses key files readable by group or
others):

```json
{
  "registration_url": "https://github.com/OWNER/REPO",
  "scale_set_name": "smol-debian13-x64-4c8g",
  "labels": ["smol-debian13-x64-4c8g"],
  "runner_group": "Default",
  "max_runners": 1,
  "auth": {
    "github_app": {
      "client_id": "Iv1.0000000000000000",
      "installation_id": 12345678,
      "private_key_file": "/etc/smolrunners/app.pem"
    }
  }
}
```

`/etc/smolrunners/pilot.toml`:

```toml
state_dir = "/var/lib/smolrunners/coordinator"
listen = "127.0.0.1:8787"
max_concurrency = 1

[adapter]
mode = "github"
program = "/opt/smolrunners/bin/smolrunners-adapter"
github_config = "/etc/smolrunners/github.json"

[host]
key = "threadripper"
arch = "x64"
vcpu = 32
memory_gib = 64
disk_gib = 500
agent = "/opt/smolrunners/bin/smolrunners"
launcher = "/opt/smolrunners/bin/smolrunners-launcher"
runtime_dir = "/opt/smolrunners/runtime"
state_dir = "/var/lib/smolrunners/state"

[profile]
name = "smol-debian13-x64-4c8g"
labels = ["smol-debian13-x64-4c8g"]
vcpu = 4
memory_gib = 8
storage_gib = 40
overlay_gib = 10
image = "/var/lib/smolrunners/images/debian13/"
entrypoint = "/opt/smolrunners/bin/run-job"

[vmm_overhead]
memory_mib = 512
vcpu_milli = 250
```

Set `[host]` to the machine's real size. The coordinator refuses to start if `listen` is not a
loopback address, or if the host cannot hold `max_concurrency` guests of the profile plus VMM
overhead. The profile is the single source of the guest shape, the host `cpu.max`/`memory.max`
(guest plus overhead: `425000 100000` and 8.5 GiB here), the ledger reservation, and the limits the
API reports.

## 4. Run the coordinator

```sh
sudo /opt/smolrunners/bin/smolrunners serve --pilot /etc/smolrunners/pilot.toml
```

From a repository checkout on the host, `sudo make live-pilot SMOLRUNNERS_PILOT_CONFIG=/etc/smolrunners/pilot.toml`
does the same. The coordinator gets or creates the scale set and opens a message session. When
GitHub reports an assigned job, it reserves capacity, requests a fresh JIT config and starts a
guest. The host agent delivers the config over stdin and runs the official runner through
`run-job`; the config never touches disk outside a root-only 0600 file that the agent deletes on
read. When the runner exits, the guest is torn down, then the reservation is released and the
runner deregistered.

The audit log is `/var/lib/smolrunners/coordinator/coordinator.log`. Each guest's runner output is
in `/var/lib/smolrunners/state/logs/<machine>.runner.log`.

## 5. Watch a job

On the host:

```sh
SMOLRUNNERS_API_URL=http://127.0.0.1:8787 /opt/smolrunners/bin/smolrunners jobs list
SMOLRUNNERS_API_URL=http://127.0.0.1:8787 /opt/smolrunners/bin/smolrunners jobs metrics ID --follow
SMOLRUNNERS_API_URL=http://127.0.0.1:8787 /opt/smolrunners/bin/smolrunners jobs cancel ID
```

`--follow` prints one line per second with state, sample count, current CPU cores and memory, and
the profile limit, and exits when the job is terminal. For the browser, forward the port over SSH
(`ssh -L 8787:127.0.0.1:8787 HOST`) and run the UI from a checkout:
`cd web && SMOLRUNNERS_API_URL=http://127.0.0.1:8787 npx vite preview`.

## 6. The trial

Add `.github/workflows/pilot.yml` to the test repository:

```yaml
name: pilot
on: workflow_dispatch
jobs:
  pilot:
    runs-on: smol-debian13-x64-4c8g
    steps:
      - uses: actions/checkout@v4
      - name: CPU phase
        run: timeout 60 sh -c 'for i in 1 2 3 4; do (while :; do :; done) & done; wait' || true
      - name: Memory phase
        run: dd if=/dev/zero of=/dev/shm/fill bs=1M count=4096 && sleep 20 && rm /dev/shm/fill
      - name: Docker build and run
        run: |
          printf 'FROM debian:13\nRUN echo built\n' > Dockerfile
          docker build -t pilot .
          docker run --rm pilot echo ran
      - uses: actions/upload-artifact@v4
        with:
          name: pilot
          path: Dockerfile
```

Then run, in order: one dispatch; a clean second dispatch; a dispatch with a step that runs
`exit 1`; and a dispatch cancelled from the GitHub UI while the CPU phase runs. After each one,
`smolrunners jobs list` should show the job terminal with GitHub's result, and
`smolrunners host --state-dir /var/lib/smolrunners/state --runtime-dir /opt/smolrunners/runtime --launcher /opt/smolrunners/bin/smolrunners-launcher list --json`
should print `[]`.

## Known limits of the pilot

- The API is plain HTTP with no operator authentication, which is why it must stay on loopback.
- Each guest pulls its image into its own private SmolVM state, so there is no image cache shared
  between guests.
- After `kill -9` of the coordinator, GitHub may hold the old message session for a while. The
  coordinator retries the adapter with backoff (5 s doubling to 120 s), and running jobs keep
  going under their host agents.
- The runner runs as root inside the guest (`RUNNER_ALLOW_RUNASROOT=1`). The guest is disposable
  and isolated by its own VM UID and cgroup.
- The VMM overhead allowance (512 MiB, 0.25 vCPU) is an estimate. If a guest that fills its memory
  is OOM-killed on the host, raise `vmm_overhead.memory_mib`.
